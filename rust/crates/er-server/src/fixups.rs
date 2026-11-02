//! Named, idempotent fixups: the operations the declarative reconciler
//! deliberately never performs.
//!
//! The entity framework has no migration files. The baseline replay only ever
//! runs `CREATE … IF NOT EXISTS`, and the reconciler only ever ADDs — it never
//! drops a superseded index, never rewrites a natural key's in-table UNIQUE,
//! never backfills data, never tightens a NOT NULL ([`er_entity_diff`]'s
//! `Severity::Ahead` is tolerance, not amnesia). Everything in that gap is a
//! **fixup**: a named Rust step, idempotent by construction, recorded by name
//! in `public.fixups` so it runs semantics-once, applied by
//! [`crate::pg::ensure_schema`] under the same advisory lock as the rest of
//! schema bootstrap.
//!
//! Two phases, because the reconciler sits between them and both orders are
//! load-bearing:
//!
//! - [`Phase::BeforeReconcile`] — constraint surgery the reconciler would
//!   otherwise refuse: a `SET NOT NULL` must land before the reconciler
//!   compares live nullability against a tightened declaration, or the boot
//!   stops on a "breaking" difference the very next statement would have
//!   resolved.
//! - [`Phase::AfterReconcile`] — data work that needs the reconciler's ADD
//!   COLUMNs to exist: backfills, sweeps, derived rows. On a fresh database
//!   the baseline already created everything, so the phase split costs
//!   nothing there.
//!
//! Names are append-only, `NNNN_` ordered, and never reused — the ledger row
//! is what "already ran" means, so renaming a shipped fixup would re-run it.

use tokio_postgres::Client;

use crate::pg::SchemaError;

/// Where a fixup runs relative to the reconciler pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    BeforeReconcile,
    AfterReconcile,
}

/// The ordered fixup list. Append only; a retired fixup keeps its name here
/// (with a no-op body) or its ledger row keeps it skipped forever.
const FIXUPS: &[(&str, Phase)] = &[("0001_default_datasets", Phase::AfterReconcile)];

/// Run every not-yet-applied fixup of `phase`, each in its own transaction
/// under the schema-bootstrap advisory lock (two boots racing: the loser waits,
/// re-reads the ledger, and skips).
///
/// # Errors
/// The first fixup that fails stops the boot, exactly as a breaking reconcile
/// does — a half-applied data shape is worse than a loud restart.
pub(crate) async fn apply_fixups(client: &mut Client, phase: Phase) -> Result<(), SchemaError> {
    for (name, declared_phase) in FIXUPS {
        if *declared_phase != phase {
            continue;
        }
        let tx = client.transaction().await?;
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)",
            &[],
        )
        .await?;
        if !er_entities::fixup::read::by_name(&tx, name).await?.is_empty() {
            tx.rollback().await?;
            continue;
        }
        match *name {
            "0001_default_datasets" => default_datasets(&tx).await?,
            other => unreachable!("fixup {other} is listed but has no body"),
        }
        er_entities::fixup::create::record(&tx, &er_ids::new_v7(), name, &applied_by()).await?;
        tx.commit().await?;
        eprintln!("er-server: fixup {name} applied");
    }
    Ok(())
}

fn applied_by() -> String {
    format!("er-server {}", env!("CARGO_PKG_VERSION"))
}

/// `0001_default_datasets` — give every org its one dataset, named `default`,
/// copying the org's lake env, config pointer, delivery root, auto-ingest
/// fields and active config version **verbatim**: nothing moves on disk or in
/// any catalog, the dataset row simply names what the org row already meant.
///
/// `entity_type` comes from the org's active published config; an org with no
/// published version defaults to `person` (the config's own default), and the
/// publish-time entity-type guard corrects any mismatch loudly rather than
/// this sweep guessing quietly. The insert is `ON CONFLICT (org_id, name) DO
/// NOTHING`, so an org that already has a `default` dataset — created by a
/// later provisioning path or an earlier run — is left exactly as it is.
async fn default_datasets(tx: &tokio_postgres::Transaction<'_>) -> Result<(), SchemaError> {
    let orgs = er_entities::org::read::list(tx).await?;
    for org in orgs {
        let entity_type = match org.active_config_version {
            Some(version) => er_entities::config_version::read::by_version(tx, &org.id, version)
                .await?
                .into_iter()
                .next()
                .and_then(|row| er_config::load_config_str(&row.yaml).ok())
                .map_or("person", |c| match c.entity_type {
                    er_config::EntityType::Person => "person",
                    er_config::EntityType::Account => "account",
                }),
            None => "person",
        };
        er_entities::dataset::create::ensure(
            tx,
            &er_ids::new_v7(),
            &org.id,
            "default",
            None,
            entity_type,
            &org.config_path,
            &org.env,
            org.drop_root.as_deref(),
            org.s3_ingest_bucket.as_deref(),
            org.s3_ingest_prefix.as_deref(),
            org.active_config_version,
            &org.state,
        )
        .await?;
    }
    Ok(())
}

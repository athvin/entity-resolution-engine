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
//!   COLUMNs to exist: backfills and derived rows. On a fresh database the
//!   baseline already created everything, so the phase split costs nothing
//!   there.
//!
//! Names are append-only, `NNNN_` ordered, and never reused — the ledger row
//! is what "already ran" means, so renaming a shipped fixup would re-run it.
//!
//! Not everything transition-shaped is a fixup. [`ensure_default_datasets`]
//! runs on **every** boot, because "each org has a `default` dataset" is a
//! convergence invariant, not an event: an org created after a one-shot sweep
//! ran would be missed forever, and one `ON CONFLICT DO NOTHING` per org per
//! boot is what idempotence is for.

use tokio_postgres::Client;

use crate::pg::SchemaError;

/// Where a fixup runs relative to the reconciler pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    BeforeReconcile,
    AfterReconcile,
}

/// The ordered fixup list. Append only; a retired fixup keeps its ledger row,
/// so its name is never reused.
const FIXUPS: &[(&str, Phase)] = &[
    ("0002_backfill_dataset_ids", Phase::AfterReconcile),
    ("0003_per_dataset_locks", Phase::AfterReconcile),
];

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
        if !er_entities::fixup::read::by_name(&tx, name)
            .await?
            .is_empty()
        {
            tx.rollback().await?;
            continue;
        }
        run_fixup(&tx, name).await?;
        er_entities::fixup::create::record(&tx, &er_ids::new_v7(), name, &applied_by()).await?;
        tx.commit().await?;
        eprintln!("er-server: fixup {name} applied");
    }
    Ok(())
}

/// One fixup's body, dispatched by its ledger name — a new fixup is one arm
/// here plus one line in [`FIXUPS`].
async fn run_fixup(tx: &tokio_postgres::Transaction<'_>, name: &str) -> Result<(), SchemaError> {
    match name {
        "0002_backfill_dataset_ids" => backfill_dataset_ids(tx).await,
        "0003_per_dataset_locks" => per_dataset_locks(tx).await,
        // Reaching this is a FIXUPS entry without a body — a bug in this
        // module, not in any database.
        other => unreachable!("fixup {other} is listed but has no body"),
    }
}

/// `0002_backfill_dataset_ids` — stamp every pre-dataset row with its org's
/// `default` dataset. One declared statement per table; each touches only
/// rows whose `dataset_id` is NULL, so the fixup is idempotent and rows
/// written after the column landed (which stamp themselves) are never
/// rewritten. Runs after [`ensure_default_datasets`] has ever converged?
/// No — ordering inside one boot is: reconcile adds the columns, this
/// backfill runs, then the convergence sweep runs. An org whose default
/// dataset does not exist yet simply has no match here and is picked up the
/// next boot; its children stay NULL until then, exactly as nullable means.
async fn backfill_dataset_ids(tx: &tokio_postgres::Transaction<'_>) -> Result<(), SchemaError> {
    er_entities::config_version::update::backfill_dataset(tx).await?;
    er_entities::matching_model::update::backfill_dataset(tx).await?;
    er_entities::job::update::backfill_dataset(tx).await?;
    er_entities::task_instance::update::backfill_dataset(tx).await?;
    er_entities::schedule::update::backfill_dataset(tx).await?;
    er_entities::s3_ingest_cursor::update::backfill_dataset(tx).await?;
    er_entities::pair_label::update::backfill_dataset(tx).await?;
    er_entities::tenant_schema_state::update::backfill_dataset(tx).await?;
    er_entities::entity_link::update::backfill_dataset(tx).await?;
    er_entities::tagging::update::backfill_dataset(tx).await?;
    er_entities::staged_steward_action::update::backfill_dataset(tx).await?;
    Ok(())
}

/// `0003_per_dataset_locks` — retire the per-org writer locks the per-dataset
/// ones supersede. The baseline replay only ever runs `CREATE … IF NOT
/// EXISTS`, so a superseded index persists on live databases forever unless
/// dropped here — and a stale `jobs_one_active_per_org` would silently keep
/// the whole org at one job, which is precisely the limit this program lifts.
/// The `tenant_schema_states` in-table `UNIQUE (org_id)` (generated from its
/// retired natural key) is looked up by column set in `pg_constraint` rather
/// than by name: Postgres named it, and names are the fragile part.
async fn per_dataset_locks(tx: &tokio_postgres::Transaction<'_>) -> Result<(), SchemaError> {
    tx.batch_execute(
        "DROP INDEX IF EXISTS jobs_one_active_per_org;\n\
         DROP INDEX IF EXISTS task_instances_one_pod_per_org;",
    )
    .await?;
    let constraints = tx
        .query(
            "SELECT con.conname FROM pg_constraint con
             JOIN pg_class rel ON rel.oid = con.conrelid
             JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
             WHERE nsp.nspname = 'public' AND rel.relname = 'tenant_schema_states'
               AND con.contype = 'u'",
            &[],
        )
        .await?;
    for row in constraints {
        let name: String = row.get(0);
        tx.batch_execute(&format!(
            "ALTER TABLE public.tenant_schema_states DROP CONSTRAINT IF EXISTS \"{name}\""
        ))
        .await?;
    }
    Ok(())
}

fn applied_by() -> String {
    format!("er-server {}", env!("CARGO_PKG_VERSION"))
}

/// Give every org its one dataset, named `default`, copying the org's lake
/// env, config pointer, delivery root, auto-ingest fields and active config
/// version **verbatim**: nothing moves on disk or in any catalog, the dataset
/// row simply names what the org row already meant. Runs on every boot
/// (convergence, not an event — see the module docs); the insert is
/// `ON CONFLICT (org_id, name) DO NOTHING`, so an org that already has a
/// `default` dataset is left exactly as it is.
///
/// `entity_type` comes from the org's active published config; an org with no
/// published version defaults to `person` (the config's own default), and the
/// publish-time entity-type guard corrects any mismatch loudly rather than
/// this sweep guessing quietly.
///
/// # Errors
/// Propagates the transaction / query errors (the boot stops, loudly).
pub(crate) async fn ensure_default_datasets(client: &mut Client) -> Result<(), SchemaError> {
    let tx = client.transaction().await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)",
        &[],
    )
    .await?;
    let orgs = er_entities::org::read::list(&tx).await?;
    for org in orgs {
        ensure_default_dataset_for(&tx, &org).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The per-org body of [`ensure_default_datasets`], also run inline where an
/// org is created so its dataset exists from the first moment rather than at
/// the next boot.
pub(crate) async fn ensure_default_dataset_for(
    client: &impl tokio_postgres::GenericClient,
    org: &er_entities::org::Org,
) -> Result<(), SchemaError> {
    let entity_type = match org.active_config_version {
        Some(version) => er_entities::config_version::read::by_version(client, &org.id, version)
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
        client,
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
    Ok(())
}

/// [`ensure_default_dataset_for`] by org name — the create-org handlers'
/// entry, which hold the name rather than the generated row.
pub(crate) async fn ensure_default_dataset_by_name(
    client: &Client,
    name: &str,
) -> Result<(), SchemaError> {
    if let Some(org) = er_entities::org::read::by_name(client, name)
        .await?
        .into_iter()
        .next()
    {
        ensure_default_dataset_for(client, &org).await?;
    }
    Ok(())
}

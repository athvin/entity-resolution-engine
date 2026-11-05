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

/// Whether a fixup removes something an older release still reads.
///
/// Every fixup must say. That is the whole mechanism: the registry below cannot
/// be appended to without classifying the new entry, and `Contract` cannot be
/// written without naming the catalog it became safe after. A reviewer sees the
/// claim; `contract_allowed` enforces it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Additive, or data-only. Safe whatever else is running.
    Safe,
    /// Removes or tightens something. `after` is the catalog hash of the
    /// release that STOPPED READING it — the expand half. This fixup is the
    /// contract half, and it may not run until the store has recorded that
    /// release, which is what makes "you cannot contract in the same release
    /// that expanded" mechanical rather than a convention.
    /// Unconstructed until the first contract fixup is written, and that order
    /// is the point: a guard added alongside the first removal could not have
    /// gated it.
    ///
    /// `allow` rather than `expect`, which was tried first and does not work
    /// here — an expectation must be FULFILLED in every compilation, and this
    /// variant is dead in the lib target while the unit tests below construct
    /// it, so `expect` errors with "unfulfilled" on the test target. Remove the
    /// attribute when a registered fixup uses the variant.
    #[allow(dead_code, reason = "ships before its first user; see above")]
    Contract { after: &'static str },
    /// Destructive, but shipped before this guard existed, so no `after` hash
    /// was ever recorded and there is nothing to check against. **Never use for
    /// a new fixup** — the variant exists so the two that predate the guard are
    /// labelled honestly rather than mis-declared `Safe`.
    ContractUngated { why: &'static str },
}

/// The ordered fixup list. Append only; a retired fixup keeps its ledger row,
/// so its name is never reused.
const FIXUPS: &[(&str, Phase, Kind)] = &[
    (
        "0002_backfill_dataset_ids",
        Phase::AfterReconcile,
        Kind::Safe,
    ),
    (
        "0003_per_dataset_locks",
        Phase::AfterReconcile,
        Kind::ContractUngated {
            why: "drops the superseded org-level locks and the retired \
                  tenant_schema_states UNIQUE. Shipped with the dataset \
                  switchover and already applied on every live store, so there \
                  is no release boundary left to gate on.",
        },
    ),
];

/// May a contract fixup run, given the catalog versions this store has recorded?
///
/// Pure, and separated from the query for that reason: the decision is the part
/// worth testing, and it needs no database to test.
///
/// The rule, and what it does and does not prove:
///
/// - **`after` is in the history** → allow. The store passed through the
///   release that stopped reading the thing, so at some point only code that
///   ignored it was serving.
/// - **The history holds nothing but the current catalog** → allow. The store
///   was born on this release; there is no older version of anything to
///   protect, and refusing here would break every fresh install.
/// - **Otherwise** → refuse. The store has history, but not the history this
///   fixup needs — it jumped from something older straight to here, so a
///   release that still read the thing may never have been retired.
///
/// What it proves is the store's HISTORY, not the absence of old pods. Keeping
/// one version serving at a time is the deploy's job — which, with the colour
/// dimension, is exactly what a cutover does.
#[must_use]
pub(crate) fn contract_allowed(after: &str, history: &[String], current: &str) -> bool {
    if history.iter().any(|h| h == after) {
        return true;
    }
    history.iter().all(|h| h == current)
}

/// Run every not-yet-applied fixup of `phase`, each in its own transaction
/// under the schema-bootstrap advisory lock (two boots racing: the loser waits,
/// re-reads the ledger, and skips).
///
/// # Errors
/// The first fixup that fails stops the boot, exactly as a breaking reconcile
/// does — a half-applied data shape is worse than a loud restart.
pub(crate) async fn apply_fixups(client: &mut Client, phase: Phase) -> Result<(), SchemaError> {
    // The ledger is read ONCE, outside the loop: it does not change under us
    // (the advisory lock serialises boots) and a per-fixup query would be the
    // same answer N times.
    let history: Vec<String> = er_entities::schema_migration::read::history(client)
        .await?
        .into_iter()
        .map(|row| row.catalog_hash)
        .collect();
    let current = crate::schema::control_catalog_hash();

    for (name, declared_phase, kind) in FIXUPS {
        if *declared_phase != phase {
            continue;
        }
        if let Kind::Contract { after } = kind
            && !contract_allowed(after, &history, &current)
        {
            // A refusal, not a skip. Skipping would leave the store in the
            // shape the fixup exists to leave behind, silently, and the next
            // boot would skip again — so the removal would never happen and
            // nothing would say so.
            return Err(SchemaError::ContractNotReady {
                fixup: (*name).to_string(),
                after: (*after).to_string(),
            });
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
        Some(version) => {
            er_entities::config_version::read::by_org_version(client, &org.id, version)
                .await?
                .into_iter()
                .next()
                .and_then(|row| er_config::load_config_str(&row.yaml).ok())
                .map_or("person", |c| match c.entity_type {
                    er_config::EntityType::Person => "person",
                    er_config::EntityType::Account => "account",
                })
        }
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

#[cfg(test)]
mod tests {
    use super::{Kind, contract_allowed};

    const EXPAND: &str = "aaaa1111";
    const OLDER: &str = "0000cccc";
    const CURRENT: &str = "bbbb2222";

    fn history(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn allowed_when_the_expand_release_is_in_the_history() {
        // The store passed through the release that stopped reading the thing,
        // so at some point only code that ignored it was serving.
        assert!(contract_allowed(
            EXPAND,
            &history(&[OLDER, EXPAND, CURRENT]),
            CURRENT
        ));
    }

    #[test]
    fn refused_when_the_store_skipped_the_expand_release() {
        // Jumped from something older straight to here: a release that still
        // read the thing may never have been retired. This is the case the
        // guard exists for.
        assert!(!contract_allowed(
            EXPAND,
            &history(&[OLDER, CURRENT]),
            CURRENT
        ));
    }

    #[test]
    fn allowed_on_a_store_born_on_this_release() {
        // Nothing older exists to protect, and refusing here would break every
        // fresh install — which is the way a guard like this usually goes
        // wrong.
        assert!(contract_allowed(EXPAND, &history(&[CURRENT]), CURRENT));
        assert!(contract_allowed(
            EXPAND,
            &history(&[CURRENT, CURRENT, CURRENT]),
            CURRENT
        ));
    }

    #[test]
    fn allowed_on_an_empty_ledger() {
        // The very first boot, before the reconciler has written anything.
        assert!(contract_allowed(EXPAND, &[], CURRENT));
    }

    #[test]
    fn refused_when_only_an_older_version_is_recorded() {
        // History exists but predates the expand release entirely.
        assert!(!contract_allowed(EXPAND, &history(&[OLDER]), CURRENT));
    }

    #[test]
    fn no_registered_contract_may_name_the_current_catalog() {
        // "You cannot contract in the same release that expanded" — if `after`
        // were this build's own catalog hash, the guard would pass trivially on
        // the very release that introduced the removal, which is the mistake it
        // exists to prevent. Checked over the real registry, so adding such a
        // fixup fails here rather than in a cluster.
        let current = crate::schema::control_catalog_hash();
        for (name, _, kind) in super::FIXUPS {
            if let Kind::Contract { after } = kind {
                assert_ne!(
                    *after,
                    current.as_str(),
                    "fixup {name} contracts against its own release's catalog"
                );
            }
        }
    }

    #[test]
    fn a_contract_kind_routes_to_the_guard() {
        // The dispatch `apply_fixups` performs, in isolation: only `Contract`
        // consults the history, and the other two run unconditionally. Written
        // as a construction rather than a match over the registry because no
        // registered fixup is a gated Contract yet — the guard ships before its
        // first user, which is the only order that makes the first user safe.
        let gated = |kind: Kind| match kind {
            Kind::Contract { after } => Some(after),
            Kind::Safe | Kind::ContractUngated { .. } => None,
        };
        assert_eq!(gated(Kind::Contract { after: EXPAND }), Some(EXPAND));
        assert_eq!(gated(Kind::Safe), None);
        assert_eq!(gated(Kind::ContractUngated { why: "historical" }), None);

        // And the branch it selects does refuse the case it exists for.
        let after = gated(Kind::Contract { after: EXPAND }).unwrap();
        assert!(!contract_allowed(
            after,
            &history(&[OLDER, CURRENT]),
            CURRENT
        ));
    }

    #[test]
    fn the_registry_classifies_every_fixup() {
        // The point of `Kind` is that appending to the registry forces a
        // decision. This asserts the decisions that exist are the ones
        // reviewed: two entries, neither of them a gated Contract yet.
        let kinds: Vec<&str> = super::FIXUPS
            .iter()
            .map(|(_, _, k)| match k {
                Kind::Safe => "safe",
                Kind::Contract { .. } => "contract",
                Kind::ContractUngated { .. } => "ungated",
            })
            .collect();
        assert_eq!(kinds, vec!["safe", "ungated"]);
    }

    #[test]
    fn an_ungated_contract_must_say_why() {
        // The variant is a confession, not a shortcut. An empty reason would
        // make it indistinguishable from `Safe`.
        for (name, _, kind) in super::FIXUPS {
            if let Kind::ContractUngated { why } = kind {
                assert!(
                    why.len() > 40,
                    "fixup {name} is ungated without a real reason"
                );
            }
        }
    }
}

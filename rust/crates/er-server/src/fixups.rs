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
    ///
    /// The guard shipped one release before its first user, which is the only
    /// order that makes a first user safe: a guard added alongside the first
    /// removal could not have gated it. `0004_dataset_id_required` is that
    /// first user.
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
    (
        "0004_dataset_id_required",
        Phase::BeforeReconcile,
        // The expand half is the release that stopped addressing these rows by
        // org: every statement re-scoped from `WHERE org_id = $1` to `WHERE
        // dataset_id = $1`, carrying catalog `bbb067f8…`. Until a store has
        // recorded that catalog, a pod may still be reading by org and relying
        // on the org-keyed uniques this fixup drops — two datasets both holding
        // a version 1 would hand such a reader the wrong row. That release
        // changed no entity, so the hash it names is shared with its
        // predecessors; it is the most specific marker the ledger carries, and
        // it still refuses the case that matters — a jump from an older catalog
        // straight to this one.
        Kind::Contract {
            after: "bbb067f8f6e59817fc972f51e6aa03379b9632bc268f189a7b0377fb06d00f87",
        },
    ),
    (
        "0005_tag_kind",
        Phase::BeforeReconcile,
        // `tags.kind` is NOT NULL, and the differ classifies a missing column
        // on nullability alone — so without this fixup every store that
        // already has `tags` would refuse to boot (ERR_SCHEMA_BREAKING) on
        // the release that introduced the column. Safe: it adds a column with
        // a default, which older releases never read — their INSERTs name
        // their columns, so the default fills, and their SELECTs enumerate
        // the columns they know.
        Kind::Safe,
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
        // The contract guard is consulted only for a fixup that has NOT run.
        // Order matters and this is the safe one: once the ledger says applied,
        // the removal has already happened, so refusing here could not undo it
        // — it would only take the store down permanently, on exactly the
        // stores that already migrated successfully.
        if let Kind::Contract { after } = kind
            && !contract_allowed(after, &history, &current)
        {
            // A refusal, not a skip. Skipping would leave the store in the
            // shape the fixup exists to leave behind, silently, and the next
            // boot would skip again — so the removal would never happen and
            // nothing would say so.
            tx.rollback().await?;
            return Err(SchemaError::ContractNotReady {
                fixup: (*name).to_string(),
                after: (*after).to_string(),
            });
        }
        run_fixup(&tx, name).await?;
        er_entities::fixup::create::record(&tx, &er_ids::new_v7(), name, &applied_by()).await?;
        tx.commit().await?;
        tracing::info!(er.fixup = %name, "the fixup was applied");
    }
    Ok(())
}

/// One fixup's body, dispatched by its ledger name — a new fixup is one arm
/// here plus one line in [`FIXUPS`].
async fn run_fixup(tx: &tokio_postgres::Transaction<'_>, name: &str) -> Result<(), SchemaError> {
    match name {
        "0002_backfill_dataset_ids" => backfill_dataset_ids(tx).await,
        "0003_per_dataset_locks" => per_dataset_locks(tx).await,
        "0004_dataset_id_required" => dataset_id_required(tx).await,
        "0005_tag_kind" => tag_kind(tx).await,
        // Reaching this is a FIXUPS entry without a body — a bug in this
        // module, not in any database.
        other => unreachable!("fixup {other} is listed but has no body"),
    }
}

/// Every relation whose rows belong to a dataset, with whether the column is
/// required there. `jobs` and `task_instances` stay nullable: an org-level
/// kind (`provision`, the migrate sweep) has no dataset by design.
const DATASET_SCOPED: &[(&str, bool)] = &[
    ("config_versions", true),
    ("matching_models", true),
    ("schedules", true),
    ("s3_ingest_cursors", true),
    // The Salesforce twin of the row above. No real store predates datasets
    // AND carries this table — it shipped after the re-key — but the aged
    // stores pg_schema_replay/pg_schema_race synthesize drop dataset_id from
    // EVERY table that has one, and a NOT NULL column only heals through this
    // fixup: the reconciler rightly refuses to add it (Breaking).
    ("sf_extract_cursors", true),
    ("pair_labels", true),
    // NULL is meaningful here: provisioning records an ORG-LEVEL lake
    // position before any dataset exists, and the deploy sweep falls back to
    // one for an org the convergence has not met yet.
    ("tenant_schema_states", false),
    ("entity_links", true),
    ("taggings", true),
    ("staged_steward_actions", true),
    ("jobs", false),
    ("task_instances", false),
];

/// Stamp every row that predates datasets with its org's `default` dataset.
///
/// Raw SQL rather than declared statements: `dataset_id` is part of the
/// natural key now, and the entity lint rightly refuses an UPDATE that
/// assigns one (E005 — natural identity is written once, at insert). A
/// migration is the exception that proves that rule, and this module is
/// where the repo already keeps migration SQL.
///
/// Touches only NULL rows, so it is idempotent and never rewrites a row that
/// stamped itself.
pub(crate) async fn backfill_dataset_ids(
    tx: &tokio_postgres::Transaction<'_>,
) -> Result<(), SchemaError> {
    for (relation, _) in DATASET_SCOPED {
        tx.batch_execute(&format!(
            "UPDATE public.{relation} c
             SET dataset_id = d.id, updated_at = now()
             FROM public.datasets d
             WHERE d.org_id = c.org_id AND d.name = 'default' AND c.dataset_id IS NULL"
        ))
        .await?;
    }
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
            // Raw, and addressed by ORG — the two reasons are the same one.
            // This runs before `0004_dataset_id_required` has filled
            // `dataset_id`, on a store where the column may be NULL or not
            // yet exist, so a generated reader would decode a now-required
            // field and panic. It is also the only read in the system that
            // legitimately has no dataset to address: it exists to decide
            // what the first dataset should be.
            client
                .query_opt(
                    "SELECT yaml FROM public.config_versions
                     WHERE org_id = $1 AND version = $2",
                    &[&org.id, &version],
                )
                .await?
                .and_then(|row| er_config::load_config_str(row.get::<_, &str>("yaml")).ok())
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

/// `0004_dataset_id_required` — make `dataset_id` what the catalog now says
/// it is, on a store that predates it.
///
/// Runs BEFORE the reconciler because the reconciler refuses a live-nullable
/// column against a NOT NULL declaration (`Severity::Breaking`, which stops
/// the boot). It therefore cannot wait for the reconciler to add the column
/// either, so it does that part itself — the one place this module stands in
/// for the differ rather than cleaning up after it.
///
/// Order within the fixup matters: add, fill, then tighten. The default
/// datasets the fill reads are converged by `ensure_schema` immediately
/// before this phase, which is why that call moved ahead of the reconciler.
///
/// Finally it drops the org-keyed uniques the dataset-keyed ones supersede.
/// Those are in-table constraints generated from the old natural keys, and
/// the reconciler never drops; left in place, `UNIQUE (org_id, version)`
/// would keep two datasets of one org from both having a version 1 — the
/// precise thing the re-key exists to allow. They are matched by COLUMN SET
/// rather than by name, because Postgres named them and names are the
/// fragile part.
async fn dataset_id_required(tx: &tokio_postgres::Transaction<'_>) -> Result<(), SchemaError> {
    for (relation, required) in DATASET_SCOPED {
        tx.batch_execute(&format!(
            "ALTER TABLE public.{relation}
             ADD COLUMN IF NOT EXISTS dataset_id text REFERENCES datasets(id)"
        ))
        .await?;
        if *required {
            tx.batch_execute(&format!(
                "UPDATE public.{relation} c
                 SET dataset_id = d.id
                 FROM public.datasets d
                 WHERE d.org_id = c.org_id AND d.name = 'default' AND c.dataset_id IS NULL"
            ))
            .await?;
            tx.batch_execute(&format!(
                "ALTER TABLE public.{relation} ALTER COLUMN dataset_id SET NOT NULL"
            ))
            .await?;
        }
    }
    for (relation, columns) in SUPERSEDED_ORG_UNIQUES {
        drop_unique_on(tx, relation, columns).await?;
    }
    Ok(())
}

/// 0005: `tags.kind` arrived NOT NULL, and the differ reads a missing NOT NULL
/// column as breaking regardless of its default — so the column is added here,
/// before the reconcile that would otherwise refuse it. Postgres fills the
/// default for existing rows without a rewrite (11+), and `IF NOT EXISTS`
/// keeps the replay idempotent on stores born after the column shipped.
///
/// The `kind IN ('user','system')` CHECK exists only on stores born fresh —
/// the reconciler's add-column path emits no constraints, and this fixup
/// matches that deliberately: enforcement is the route's (the create path
/// hardcodes the value), so old and new stores differ only in where a bug
/// would be caught, not in what is accepted. `apply_min_role` is nullable and
/// needs no fixup — the reconcile adds it on its own.
async fn tag_kind(tx: &tokio_postgres::Transaction<'_>) -> Result<(), SchemaError> {
    tx.batch_execute(
        "ALTER TABLE public.tags
         ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'user'",
    )
    .await?;
    Ok(())
}

/// The org-keyed uniques the dataset-keyed natural keys replace, by the
/// column set Postgres recorded for them.
const SUPERSEDED_ORG_UNIQUES: &[(&str, &[&str])] = &[
    ("config_versions", &["org_id", "version"]),
    ("matching_models", &["org_id", "name", "version"]),
    ("pair_labels", &["org_id", "rec_a_key", "rec_b_key"]),
    ("s3_ingest_cursors", &["org_id", "source"]),
    (
        "entity_links",
        &["org_id", "parent_entity_id", "child_entity_id", "kind"],
    ),
];

/// Drop the unique constraint on `relation` whose columns are exactly
/// `columns`, whatever Postgres called it.
///
/// Column-set precise, unlike `0003`'s sweep of every unique on one table:
/// here the NEW natural key is also a unique constraint on the same
/// relation, and dropping it would undo the re-key in the same breath.
async fn drop_unique_on(
    tx: &tokio_postgres::Transaction<'_>,
    relation: &str,
    columns: &[&str],
) -> Result<(), SchemaError> {
    let rows = tx
        .query(
            "SELECT con.conname,
                    array_agg(att.attname::text ORDER BY att.attname) AS cols
             FROM pg_constraint con
             JOIN pg_class rel ON rel.oid = con.conrelid
             JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
             JOIN unnest(con.conkey) AS k(attnum) ON true
             JOIN pg_attribute att ON att.attrelid = rel.oid AND att.attnum = k.attnum
             WHERE nsp.nspname = 'public' AND rel.relname = $1 AND con.contype = 'u'
             GROUP BY con.conname",
            &[&relation],
        )
        .await?;
    let mut want: Vec<&str> = columns.to_vec();
    want.sort_unstable();
    for row in rows {
        let name: String = row.get(0);
        let cols: Vec<String> = row.get(1);
        if cols == want {
            tx.batch_execute(&format!(
                "ALTER TABLE public.{relation} DROP CONSTRAINT IF EXISTS \"{name}\""
            ))
            .await?;
        }
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
        // as a construction rather than a match over the registry so it keeps
        // asserting the dispatch itself, whatever the registry happens to hold.
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
        // reviewed: 0005 adds a NOT NULL column with a default — Safe because
        // older releases never read it, and its DDL fills the default for
        // their INSERTs.
        let kinds: Vec<&str> = super::FIXUPS
            .iter()
            .map(|(_, _, k)| match k {
                Kind::Safe => "safe",
                Kind::Contract { .. } => "contract",
                Kind::ContractUngated { .. } => "ungated",
            })
            .collect();
        assert_eq!(kinds, vec!["safe", "ungated", "contract", "safe"]);
    }

    #[test]
    fn every_contract_names_a_catalog_hash() {
        // `after` is compared against `schema_migrations.catalog_hash`, so a
        // value that is not shaped like one of those can never match and the
        // fixup would refuse every store that has any history — a typo here is
        // a boot failure, not a weaker guard.
        for (name, _, kind) in super::FIXUPS {
            if let Kind::Contract { after } = kind {
                assert_eq!(after.len(), 64, "fixup {name}: not a sha256 hex digest");
                assert!(
                    after
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
                    "fixup {name}: not lowercase hex"
                );
            }
        }
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

//! Account golden generation shared by embedded execution and the remote pilot.
//! Intermediates live in the native temporary catalog. Publication replaces both
//! lake relations in one transaction, so a reader never sees half an assembly.

use anyhow::{Result, anyhow, ensure};
use er_config::Config;
use er_entities::execution::Executor;

/// Execute the existing account survivorship generators on one pinned session.
///
/// # Errors
/// Refuses person configuration and propagates execution failures. A failed or
/// uncertain commit is never replayed. The caller must close the session.
pub fn run_account<E: Executor>(session: &E, config: &Config, run_at: &str) -> Result<()> {
    run_account_guarded(session, config, run_at, || Ok(()))
}

/// As `run_account`, checking the attempt fence before opening and committing.
///
/// # Errors
/// The guard refuses lost ownership or cancellation; execution errors propagate.
pub fn run_account_guarded<E: Executor>(
    session: &E,
    config: &Config,
    run_at: &str,
    guard: impl Fn() -> Result<()>,
) -> Result<()> {
    guard()?;
    ensure!(
        !config.entity_type.is_person(),
        "remote golden pilot requires an account dataset"
    );
    let execute = |sql: &str| {
        session
            .query(sql, &[])
            .map(|_| ())
            .map_err(|e| anyhow!("{e}"))
    };
    let lineage = er_golden::acct_golden_lineage_sql(
        config,
        crate::ACCT_INT_STD_RECORDS,
        crate::ENTITY_MEMBERSHIP,
        crate::ENTITIES,
        run_at,
    )?;
    let records = er_golden::acct_golden_records_sql(
        config,
        "temp.main.er_golden_lineage",
        crate::ACCT_INT_STD_RECORDS,
        crate::ENTITY_MEMBERSHIP,
        crate::ENTITIES,
        run_at,
    );
    execute("BEGIN TRANSACTION")?;
    let result = (|| {
        execute(&format!(
            "CREATE OR REPLACE TEMP TABLE er_golden_lineage AS {lineage}"
        ))?;
        execute(&format!(
            "CREATE OR REPLACE TEMP TABLE er_golden_records AS {records}"
        ))?;
        for (target, source) in [
            (crate::ACCT_GOLDEN_LINEAGE, "temp.main.er_golden_lineage"),
            (crate::ACCT_GOLDEN_RECORDS, "temp.main.er_golden_records"),
        ] {
            execute(&format!("DROP TABLE IF EXISTS {target}"))?;
            execute(&format!("CREATE TABLE {target} AS SELECT * FROM {source}"))?;
        }
        guard()?;
        execute("COMMIT")
    })();
    if result.is_err() {
        // Remote transports refuse this if execution is uncertain; they must
        // never recreate the session or replay the publication to roll it back.
        let _ = execute("ROLLBACK");
    }
    result
}

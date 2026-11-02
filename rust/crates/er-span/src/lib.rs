//! The span vocabulary the generated query functions call.
//!
//! This is the leaf of the telemetry layer. It emits; it does not collect.
//! `tracing`'s model is that libraries emit spans and the *binary* installs a
//! subscriber, so the exporter, the subscriber and the metric instruments all
//! live in `er-obs` and only binaries depend on it. Nothing here allocates a
//! buffer, opens a socket or reads a clock: with no subscriber installed, a
//! span is a few atomic loads and a discard.
//!
//! ## Why a crate rather than `tracing` directly in the generated code
//!
//! `er-entities` is generated and committed — 42 of its 46 modules carry query
//! functions, ~19.7k lines. If the generated text called `tracing::info_span!`
//! with inline attribute literals, then renaming one attribute would regenerate
//! and churn all 19.7k lines, and `er-entity verify` byte-compares every one of
//! them. Behind this crate it is a one-file change. The same indirection is
//! what lets [`sql_text_enabled`] read its environment variable once instead of
//! 218 times, and what gives the [`attr`] vocabulary somewhere to be a value.
//!
//! ## The PII boundary, enforced by the signature
//!
//! [`sql_span`] takes no parameter argument. There is nowhere to put a bind
//! value, which is a stronger guarantee than a rule in a document: a caller
//! cannot record one by mistake because the function will not accept one. See
//! [`attr`] for the rest of the §10.4 reasoning.

pub mod attr;

use std::sync::OnceLock;

use tracing::field::Empty;

/// Which store a statement runs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// The tenant's DuckLake catalog.
    Lake,
    /// The Postgres control plane.
    Control,
}

impl Store {
    /// The OpenTelemetry `db.system.name` value.
    #[must_use]
    pub const fn system(self) -> &'static str {
        match self {
            Self::Lake => "duckdb",
            Self::Control => "postgresql",
        }
    }
}

/// How a spanned operation ended. The `er.outcome` dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
}

impl Outcome {
    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

/// Whether the statement template is exported as `db.query.text`.
///
/// Off unless `ER_TRACE_SQL_TEXT` is set to something other than `0`. Two
/// independent reasons, and only one of them is safety:
///
/// - **Volume.** `entities/job/update/claim.sql` is ~40 lines with comments. A
///   run executes hundreds of statements, so exporting every template would
///   push tens of megabytes of identical SQL per trace, against backends that
///   bound a span's size.
/// - **Blast radius.** The analyser proves declared statements bind their
///   values rather than embedding them, and a posterity test holds that line.
///   But the default should not be "ship the text" on the day someone writes
///   the first statement with an inline literal.
///
/// Read once — the generated code asks this on every call, and 218 getenv
/// syscalls per run is a measurable cost for a flag that cannot change inside
/// a process.
#[must_use]
pub fn sql_text_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("ER_TRACE_SQL_TEXT").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// The span for one execution of a **declared** statement.
///
/// `entity`, `folder` and `stem` are the triple that names the statement's
/// `.sql` file, which is its stable identity: `entities/<entity>/<folder>/<stem>.sql`
/// becomes `<entity>.<folder>.<stem>`. The emitter has all three in scope, so
/// threading them costs nothing.
///
/// `sql` is the statement **template**, with bound `$n` placeholders. It is
/// recorded only when [`sql_text_enabled`]; the identity is always recorded,
/// and the identity is enough to `cat` the file.
///
/// There is deliberately no parameter argument. Bind values are this engine's
/// payload and never belong in telemetry (§10.4).
#[must_use]
pub fn sql_span(
    store: Store,
    entity: &'static str,
    folder: &'static str,
    stem: &'static str,
    relation: &'static str,
    sql: &'static str,
) -> tracing::Span {
    // `tracing` span names must be literals, so the constant name is "db.query"
    // and the per-statement name rides `otel.name`, which tracing-opentelemetry
    // reads when it builds the exported span.
    let span = tracing::info_span!(
        "db.query",
        otel.name = Empty,
        otel.kind = "client",
        { attr::STATEMENT } = Empty,
        { attr::DB_SYSTEM } = store.system(),
        { attr::RELATION } = relation,
        { attr::DB_QUERY_TEXT } = Empty,
        { attr::ROWS } = Empty,
        { attr::OUTCOME } = Empty,
    );
    // Formatting the identity is only worth it when something is listening.
    if !span.is_disabled() {
        let statement = format!("{entity}.{folder}.{stem}");
        span.record(attr::STATEMENT, statement.as_str());
        span.record("otel.name", statement.as_str());
        if sql_text_enabled() {
            span.record(attr::DB_QUERY_TEXT, sql);
        }
    }
    span
}

/// The span for one execution of a statement whose SQL is **assembled at
/// runtime** — the engine's CTE pipelines and the `er-golden` / `er-compile`
/// generators.
///
/// Carries the generator's identity and a bounded summary, never the text.
/// Generated SQL can embed config-derived literals: `er_golden`'s survivorship
/// ranking interpolates each source-system name as a SQL literal, and those are
/// tenant metadata. The rule holds on shape rather than on a per-call-site
/// audit, because an audit is a thing that goes stale.
#[must_use]
pub fn generator_span(
    store: Store,
    generator: &'static str,
    summary: &'static str,
) -> tracing::Span {
    let span = tracing::info_span!(
        "db.query",
        otel.name = generator,
        otel.kind = "client",
        { attr::GENERATOR } = generator,
        { attr::DB_SYSTEM } = store.system(),
        { attr::DB_QUERY_SUMMARY } = summary,
        { attr::ROWS } = Empty,
        { attr::OUTCOME } = Empty,
    );
    span
}

/// Record the row count on a span.
pub fn rows(span: &tracing::Span, n: u64) {
    span.record(attr::ROWS, n);
}

/// Mark a span's outcome from a `Result` and hand the `Result` straight back,
/// so a `?` site becomes one wrapping call rather than a match.
///
/// The error's *text* is recorded, not the error itself: the message belongs on
/// the span for debugging, while the chain stays with the propagated error for
/// the caller to flatten in the workspace's usual way. Nothing here interpolates
/// a source — that rule governs `Display` on error types and is enforced by
/// `er-errors/tests/display_no_source.rs`.
pub fn check<T, E: core::fmt::Display>(span: &tracing::Span, result: Result<T, E>) -> Result<T, E> {
    match &result {
        Ok(_) => {
            span.record(attr::OUTCOME, Outcome::Ok.as_str());
        }
        Err(e) => {
            span.record(attr::OUTCOME, Outcome::Error.as_str());
            span.record("otel.status_code", "ERROR");
            span.record("error.type", tracing::field::display(e));
        }
    }
    result
}

/// Attach a span to a future.
///
/// The async path **must** use this rather than `Span::enter`. A
/// `tracing::span::Entered` guard is thread-local; held across an `.await` it
/// attributes whatever else that thread runs to this span, which is both wrong
/// and invisible. The generated Postgres functions are async, so without this
/// the mistake would be generated into 110 call sites at once.
pub fn instrument<F: core::future::Future>(
    span: &tracing::Span,
    future: F,
) -> tracing::instrument::Instrumented<F> {
    use tracing::Instrument;
    future.instrument(span.clone())
}

#[cfg(test)]
mod tests {
    use super::{Outcome, Store, check, generator_span, rows, sql_span};

    #[test]
    fn a_store_maps_to_its_opentelemetry_system_name() {
        assert_eq!(Store::Lake.system(), "duckdb");
        assert_eq!(Store::Control.system(), "postgresql");
        assert_eq!(Outcome::Ok.as_str(), "ok");
        assert_eq!(Outcome::Error.as_str(), "error");
    }

    #[test]
    fn a_span_is_inert_without_a_subscriber() {
        // No subscriber is installed in this test binary, which is the state
        // every library call is in until a binary opts in. The point is that
        // the whole surface is callable and records nothing — a library that
        // panicked or allocated here would tax every `er` CLI invocation.
        let span = sql_span(
            Store::Lake,
            "run_stage",
            "create",
            "start",
            "run_stages",
            "INSERT INTO lake.main.run_stages VALUES ($1)",
        );
        assert!(span.is_disabled());
        rows(&span, 7);
        let ok: Result<u8, std::fmt::Error> = check(&span, Ok(1));
        assert!(ok.is_ok());
        let err: Result<u8, std::fmt::Error> = check(&span, Err(std::fmt::Error));
        assert!(err.is_err());

        let generated = generator_span(
            Store::Lake,
            "er_golden::merge_plans_sql",
            "merge_plans(...)",
        );
        assert!(generated.is_disabled());
    }

    #[test]
    fn check_returns_the_result_unchanged() {
        // `check` is an observation, and observation must never change a run —
        // the discipline `er-server`'s attempt reporter states outright. The
        // value and the error both pass through untouched.
        let span = tracing::Span::none();
        assert_eq!(check(&span, Ok::<_, std::fmt::Error>(42)).unwrap(), 42);
        assert!(check(&span, Err::<u8, _>(std::fmt::Error)).is_err());
    }
}

//! Every metric this system emits, declared as a value.
//!
//! ## Why a registry and not a list in a doc
//!
//! `er-entity-lint/src/lib.rs` opens with a confession worth copying: hand-typed
//! rule references are *"how three documented codes ended up never
//! implemented."* A metrics table in Markdown has the same failure mode and a
//! worse one — a metric that exists but is undocumented is merely untidy, while
//! a metric that is documented but never emitted reads as **coverage**. Its
//! dashboard panel renders an honest-looking empty chart rather than an error,
//! and the gap is invisible precisely to the person relying on it.
//!
//! So the declarations here are the source of truth, the observability skill's
//! `references/telemetry-reference.md` is generated from them and byte-compared,
//! and `tests/cardinality.rs` checks
//! the one property that costs money if it slips.
//!
//! ## The cardinality rule, with the arithmetic that motivates it
//!
//! Under `awsemf` every distinct dimension set becomes a CloudWatch custom
//! metric at roughly $0.30/month. The statement name space has 247 members, so
//! `statement_name` as a dimension would be 247 × 2 outcomes = **494 series ≈
//! $148/month for one histogram**, in the dev namespace, for one developer —
//! and it would grow silently with every new `.sql` file. That is D-OTEL's
//! stated failure mode for `tenant`, reached by a different route.
//!
//! Hence [`FORBIDDEN_DIMENSIONS`]: statement identity, tenancy and record
//! identity are **span attributes only**. A span carries them for free and the
//! span store can group by them; a metric cannot afford them. If a per-statement
//! aggregate is ever wanted, it is a `spanmetrics` connector in the collector's
//! values file — a YAML edit, revertible, with no Rust change.

/// What a metric measures, which decides its OTel instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Monotonic count of things that happened.
    Counter,
    /// A distribution — latency, bytes, rows.
    Histogram,
    /// A value sampled at a point in time.
    Gauge,
}

impl Kind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Counter => "counter",
            Kind::Histogram => "histogram",
            Kind::Gauge => "gauge",
        }
    }
}

/// One declared metric.
pub struct Metric {
    /// Dotted OTel name, `er.`-prefixed.
    pub name: &'static str,
    pub kind: Kind,
    /// The unit, in OTel's UCUM spelling (`ms`, `By`, `1`).
    pub unit: &'static str,
    /// What it measures, and what a reader should conclude from a change in it.
    pub description: &'static str,
    /// Every dimension this metric carries. Each must be drawn from a closed
    /// vocabulary that exists in code, and the product of their cardinalities
    /// is the series count this metric costs.
    pub dimensions: &'static [&'static str],
    /// The series count, stated rather than computed, so a reader can see what
    /// the author believed and a reviewer can disagree with it.
    ///
    /// This is the **OTel-side** cardinality: the product of the dimensions
    /// below. What CloudWatch BILLS equals it only because `awsemf` is
    /// configured `dimension_rollup_option: NoDimensionRollup`.
    ///
    /// With the exporter's default rollup it does not. Measured in ER/Dev on
    /// 2026-11-05, this one histogram with two dimensions had become four
    /// dimension SETS — `{db.system.name, OTelLib, outcome}`,
    /// `{OTelLib, outcome}`, `{OTelLib}`, `{db.system.name, OTelLib}` — for
    /// **nine** billable series against the four declared here, because rollup
    /// emits one series per subset and injects `OTelLib`.
    ///
    /// So if that exporter setting is ever removed, every number in this
    /// registry understates the bill, and `tests/cardinality.rs` cannot see it:
    /// the inflation happens in the collector, after the process that declared
    /// it. That is the one part of this calculation a Rust test cannot hold.
    pub series: usize,
}

/// Dimension names that may never appear on a metric.
///
/// Not a style rule. Each of these is unbounded or tenant-scoped, and each
/// would turn one metric into hundreds of billable series that grow without
/// anyone choosing to grow them. They are all available on spans, which is
/// where high-cardinality identity belongs.
pub const FORBIDDEN_DIMENSIONS: &[&str] = &[
    // 247 members and counting — one per declared statement.
    "statement",
    "statement_name",
    "er.statement",
    // Tenancy. D-OTEL names this one explicitly.
    "org",
    "er.org",
    "org_id",
    "dataset",
    "er.dataset",
    "dataset_id",
    // Unbounded per run.
    "run_id",
    "er.run_id",
    "job_id",
    "er.job_id",
    "task_instance_id",
    // Record identity: unbounded by construction, and the payload's neighbour.
    "record_key",
    "er.record_key",
    "entity_id",
    "er.entity_id",
    // Grows with the UI, and the whole point of §10's span-only decision.
    "er.action",
    // URL shapes: `url.query` is payload here, and `url.path` is unbounded
    // because it carries identifiers. `http.route` is the bounded form.
    "url.path",
    "url.query",
];

/// The declared metrics.
///
/// Deliberately small. The plan sketches ~320 series across API RED, queue
/// depth, dispatch latency, stage duration, spill bytes and pod outcomes; this
/// is the substrate plus the one metric whose emission path — a leaf crate
/// reaching a provider without depending on it — had to be designed rather than
/// typed. Each later metric is an entry here and a call site, not new
/// machinery.
pub const METRICS: &[Metric] = &[Metric {
    name: "er.db.query.duration",
    kind: Kind::Histogram,
    unit: "ms",
    description: "How long a SQL statement took, aggregated across every statement. \
                  Statement identity is deliberately NOT a dimension — see the module \
                  docs for the arithmetic — so this answers 'is the database slow' and \
                  not 'which statement is slow'. The second question is a span query, \
                  where `er.statement` is recorded and costs nothing.",
    dimensions: &["db.system.name", "outcome"],
    // 2 stores (duckdb, postgresql) x 2 outcomes (ok, error).
    series: 4,
}];

/// Render the reference document.
///
/// Byte-compared by `tests/reference.rs`, so this function's output IS the
/// committed doc. Formatting is pinned here rather than left to a formatter,
/// for the reason the generated entity-lint reference already learned: a generator whose
/// output depends on an external tool's version fails the comparison for a
/// reason that has nothing to do with the content.
#[must_use]
pub fn reference() -> String {
    // Written with `write!` rather than by pushing formatted temporaries:
    // clippy's `format_push_string` rejects the latter, which allocates a
    // String per fragment only to copy and drop it. The writes cannot fail —
    // the target is a String — so the results are discarded.
    use core::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Telemetry reference\n\n");
    out.push_str(
        "GENERATED from `rust/crates/er-obs/src/registry.rs` by its own test. Do not\n\
         hand-edit: `cargo test -p er-obs reference` byte-compares this file, and\n\
         `UPDATE_EXPECT=1 cargo test -p er-obs reference` rewrites it.\n\n",
    );
    out.push_str(
        "A metrics table written by hand is how a metric ends up documented and never\n\
         emitted, which reads as coverage rather than as a gap — its dashboard panel\n\
         renders an honest-looking empty chart. The registry is therefore the source\n\
         of truth and this file is its output.\n\n",
    );

    out.push_str("## Metrics\n\n");
    out.push_str("| Metric | Kind | Unit | Dimensions | Series |\n");
    out.push_str("|---|---|---|---|---|\n");
    let mut total = 0;
    for m in METRICS {
        let _ = writeln!(
            out,
            "| `{}` | {} | `{}` | {} | {} |",
            m.name,
            m.kind.as_str(),
            m.unit,
            m.dimensions
                .iter()
                .map(|d| format!("`{d}`"))
                .collect::<Vec<_>>()
                .join(", "),
            m.series,
        );
        total += m.series;
    }
    let _ = write!(out, "\n**{total} series in total.**\n\n");

    for m in METRICS {
        let _ = write!(out, "### `{}`\n\n{}\n\n", m.name, m.description);
    }

    out.push_str("## Dimensions a metric may never carry\n\n");
    out.push_str(
        "Each is unbounded or tenant-scoped, and each would turn one metric into\n\
         hundreds of billable series that grow without anyone choosing to grow them.\n\
         Under `awsemf` a distinct dimension set is a CloudWatch custom metric at\n\
         roughly $0.30/month: `statement_name` alone is 247 statements x 2 outcomes =\n\
         494 series, about $148/month, for one histogram in one developer's dev\n\
         namespace.\n\n\
         All of them are available on spans, which is where high-cardinality identity\n\
         belongs. `tests/cardinality.rs` enforces this.\n\n",
    );
    for d in FORBIDDEN_DIMENSIONS {
        let _ = writeln!(out, "- `{d}`");
    }
    out.push('\n');
    out
}

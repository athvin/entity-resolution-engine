//! No metric may carry a dimension that costs money without anyone choosing to.
//!
//! ## The arithmetic this protects
//!
//! Under `awsemf` every distinct dimension set becomes a CloudWatch custom
//! metric at roughly $0.30/month. So a dimension's cost is the product of its
//! cardinality with every other dimension on the same metric, and a dimension
//! drawn from an *open* set has no bound at all:
//!
//! - `statement_name`: 247 declared statements × 2 outcomes = **494 series ≈
//!   $148/month** for one histogram, in the dev namespace, for one developer —
//!   growing silently with every new `.sql` file.
//! - `org` / `dataset`: D-OTEL names tenancy as the stated failure mode, and
//!   the datasets work makes it worse by multiplying one org into several.
//! - `run_id`, `record_key`, `entity_id`: unbounded by construction.
//!
//! None of this is hypothetical arithmetic about a mistake nobody would make.
//! Every one of those names is in scope at the emission site, which is exactly
//! why a gate is cheaper than a convention.
//!
//! ## What this test can and cannot prove
//!
//! It checks the **registry's own declarations**, which is where a dimension
//! would be added deliberately. It cannot see a `KeyValue` added directly at a
//! call site — that is what `metrics.rs`'s own
//! `the_emission_site_matches_what_the_registry_declares` covers, by asserting
//! the declared dimension list against the code's.
//!
//! The pair is the guarantee: the registry cannot declare a forbidden
//! dimension, and the code cannot drift from the registry.

use er_obs::registry::{FORBIDDEN_DIMENSIONS, METRICS};

#[test]
fn no_declared_metric_carries_a_forbidden_dimension() {
    let mut offenders = Vec::new();
    for metric in METRICS {
        for dimension in metric.dimensions {
            if FORBIDDEN_DIMENSIONS.contains(dimension) {
                offenders.push(format!("`{}` carries `{dimension}`", metric.name));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a metric may not carry an unbounded or tenant-scoped dimension:\n\n{}\n\n\
         Under `awsemf` each distinct dimension set is a billable CloudWatch custom \
         metric at ~$0.30/month, so `statement_name` alone is 247 statements x 2 \
         outcomes = 494 series, about $148/month, for ONE histogram.\n\n\
         All of these are available on SPANS, where they cost nothing and the store \
         can group by them. If a per-dimension aggregate is genuinely wanted, add a \
         `spanmetrics` connector to the collector's values file — a YAML edit, \
         revertible, with no Rust change.",
        offenders.join("\n")
    );
}

#[test]
fn every_metric_states_a_series_count_that_matches_its_dimensions() {
    // A `series` field nobody checks is decoration. It cannot be computed from
    // the names alone — the cardinality of `db.system.name` is domain knowledge
    // — so the check is that it is plausible: at least one series per metric,
    // and never more than a few hundred for the whole set.
    for metric in METRICS {
        assert!(
            metric.series >= 1,
            "`{}` claims {} series",
            metric.name,
            metric.series
        );
        assert!(
            metric.series <= 64,
            "`{}` claims {} series on its own. Nothing in this system needs that \
             many on one metric; check the dimensions are all closed vocabularies.",
            metric.name,
            metric.series
        );
    }

    let total: usize = METRICS.iter().map(|m| m.series).sum();
    assert!(
        total <= 400,
        "{total} series declared in total. The plan budgets ~320 across every \
         signal; past that, the next metric should replace one rather than add to \
         the bill."
    );
}

#[test]
fn metric_names_are_prefixed_and_dotted() {
    // Bounds what can reach a dashboard and keeps our names distinguishable
    // from the OTel semantic conventions they sit beside.
    for metric in METRICS {
        assert!(
            metric.name.starts_with("er."),
            "`{}` is not `er.`-prefixed",
            metric.name
        );
        assert!(
            metric
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
            "`{}` has characters outside the lowercase dotted form",
            metric.name
        );
    }
}

#[test]
fn the_forbidden_list_has_no_duplicates() {
    // A duplicate is harmless at runtime and a sign the list was appended to
    // without being read, which is how the entry that matters gets missed.
    let mut seen = std::collections::BTreeSet::new();
    for dimension in FORBIDDEN_DIMENSIONS {
        assert!(
            seen.insert(dimension),
            "`{dimension}` appears twice in FORBIDDEN_DIMENSIONS"
        );
    }
    assert!(
        seen.len() > 15,
        "the forbidden list looks truncated ({} entries)",
        seen.len()
    );
}

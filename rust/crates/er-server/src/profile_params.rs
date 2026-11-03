//! Turning a `profile` job's params into a profile request.
//!
//! There are two paths that run a profile — the dispatcher's in-process arm and
//! the DAG task the Kubernetes executor will call — and they must agree on what
//! a job's params mean. One parser is the only way to guarantee that: two would
//! drift the first time a default changed, and a scheduled profile that quietly
//! ran at a different tier than an operator-triggered one is the kind of
//! disagreement nobody notices until the numbers do not match.

use dedupe_core::profile::{ProfileOptions, ProfileSample};
use dedupe_core::session::Session;
use er_cli::profile::{ProfileReport, Subject};
use serde_json::Value;

/// What a `profile` job asks for.
#[derive(Debug, Clone)]
pub struct ProfileRequest {
    pub subject: Subject,
    pub source: Option<String>,
    pub tier: i64,
    pub options: ProfileOptions,
}

/// The tier a job with no `tier` param gets.
const DEFAULT_TIER: i64 = 1;

impl ProfileRequest {
    /// Read a request out of `jobs.params`.
    ///
    /// Defaults are chosen for the unattended caller, because that is who omits
    /// things: a schedule fires with `{}`, and the useful profile to take
    /// without being asked is the standardized corpus with EXACT distinct
    /// counts. Exactness is the default here and not in the library, because
    /// the findings that matter most — a column that is not the unique
    /// identifier it looks like, values that collapse under normalization —
    /// decline to speak at all without it, and a scheduled profile that stays
    /// silent on them is worse than no schedule.
    ///
    /// # Errors
    /// Names the accepted values for an unknown subject, and refuses a `raw`
    /// request with no source rather than silently profiling one arbitrary
    /// source's rows.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let subject = params
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("standardized");
        let subject = Subject::parse(subject).map_err(|e| format!("{e:#}"))?;
        let source = params
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string);
        if subject == Subject::Raw && source.is_none() {
            return Err(
                "a raw profile needs a \"source\" param: two sources' delivered payloads share \
                 no schema, so a profile merging them would describe neither"
                    .to_string(),
            );
        }
        let tier = params
            .get("tier")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_TIER);
        if tier < 1 {
            return Err(format!("tier must be at least 1, got {tier}"));
        }
        let exact_distinct = params
            .get("exact_distinct")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        // The out-of-range arm comes FIRST. Ordered the other way round, a
        // `p >= 1.0` arm also swallows 1.5 and silently reports "all rows" for
        // a nonsense request — which is what it did until this module's own
        // test said so.
        let sample = match params.get("sample").and_then(Value::as_f64) {
            None => None,
            Some(p) if p <= 0.0 || p > 1.0 => {
                return Err(format!("sample must be in (0, 1], got {p}"));
            }
            // Exactly 1.0 is "all rows": no sample at all rather than an error,
            // so a caller computing a proportion need not special-case its own
            // upper bound.
            Some(p) if p >= 1.0 => None,
            Some(p) => Some(ProfileSample {
                // `rowid` is DuckDB's own and stable within a snapshot, which is
                // all a deterministic sample needs; the three subjects do not
                // share a key column to use instead.
                id_expr: "cast(rowid as varchar)".to_string(),
                proportion: p,
                seed: params.get("sample_seed").and_then(Value::as_i64),
            }),
        };
        Ok(ProfileRequest {
            subject,
            source,
            tier,
            options: ProfileOptions {
                exact_distinct,
                sample,
            },
        })
    }

    /// Run the request against an open lake.
    ///
    /// # Errors
    /// The profile's failure, flattened with the alternate formatter so the
    /// whole chain survives — `er-server` keeps job failures as strings, and a
    /// bare `to_string()` there would discard the cause.
    pub fn run(
        &self,
        session: &Session,
        account: bool,
        captured_at: &str,
    ) -> Result<ProfileReport, String> {
        er_cli::profile::run_profile(
            session,
            &er_cli::profile::ProfileInputs {
                subject: self.subject,
                source: self.source.as_deref(),
                tier: self.tier,
                options: &self.options,
                account,
                captured_at,
                // A requested or scheduled profile describes the corpus as it
                // stands, not one landing — the auto-profile on the ingest
                // stage is the only caller that can name a delivery.
                ingest_batch_id: None,
            },
        )
        .map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_params_object_is_the_unattended_default() {
        // What a schedule fires with.
        let r = ProfileRequest::from_params(&json!({})).unwrap();
        assert_eq!(r.subject, Subject::Standardized);
        assert_eq!(r.tier, 1);
        assert!(
            r.options.exact_distinct,
            "a scheduled profile must be the trustworthy one, or the findings that need an \
             exact count stay silent and the schedule reports less than it appears to"
        );
        assert!(r.options.sample.is_none());
        assert!(r.source.is_none());
    }

    #[test]
    fn a_raw_profile_without_a_source_is_refused() {
        let err = ProfileRequest::from_params(&json!({"subject": "raw"})).unwrap_err();
        assert!(err.contains("needs a \"source\""), "{err}");
        // With one, it is fine.
        let r = ProfileRequest::from_params(&json!({"subject": "raw", "source": "crm"})).unwrap();
        assert_eq!(r.subject, Subject::Raw);
        assert_eq!(r.source.as_deref(), Some("crm"));
    }

    #[test]
    fn an_unknown_subject_names_the_accepted_values() {
        let err = ProfileRequest::from_params(&json!({"subject": "nonsense"})).unwrap_err();
        assert!(err.contains("standardized"), "{err}");
        assert!(err.contains("golden"), "{err}");
    }

    #[test]
    fn sampling_is_parsed_with_its_bounds() {
        let r = ProfileRequest::from_params(&json!({"sample": 0.25, "sample_seed": 7})).unwrap();
        let s = r.options.sample.expect("sampled");
        assert!((s.proportion - 0.25).abs() < f64::EPSILON);
        assert_eq!(s.seed, Some(7));

        // 1.0 is "all rows" — no sample, not an error.
        assert!(
            ProfileRequest::from_params(&json!({"sample": 1.0}))
                .unwrap()
                .options
                .sample
                .is_none()
        );
        for bad in [0.0, -0.5, 1.5] {
            assert!(
                ProfileRequest::from_params(&json!({"sample": bad})).is_err(),
                "sample {bad} should be refused"
            );
        }
    }

    #[test]
    fn a_tier_below_one_is_refused() {
        assert!(ProfileRequest::from_params(&json!({"tier": 0})).is_err());
        assert_eq!(
            ProfileRequest::from_params(&json!({"tier": 2}))
                .unwrap()
                .tier,
            2
        );
    }
}

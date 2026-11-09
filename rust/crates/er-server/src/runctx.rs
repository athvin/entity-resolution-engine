//! What a pipeline run needs, resolved once from what the work order carries
//! (batch-orchestration-design §6.3 step 1 / §11 Phase 3).
//!
//! The pod holds **no control-plane credential** (§6.4), so it cannot read the
//! model registry: the server resolves the registry — its own data — and the
//! work order carries the result. Everything else a stage needs is derivable
//! from the tenant's lake env and the job's params, which the order already
//! carries.
//!
//! This module exists so there is exactly ONE answer to "what does this job
//! mean": the legacy in-process arm and the executor's task bodies both build
//! a [`RunContext`] from the same inputs through the same code. A second
//! resolution path would be the class of bug where a run behaves differently
//! depending on which executor claimed it — invisible until the two disagree
//! in production.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One ACTIVE model version, as the control plane resolved it from the
/// registry and the work order carries it.
///
/// `model_json` is the settings document, not a path: the pod cannot read the
/// registry, and a tenant's model is not a file on the shared filesystem for
/// orgs created since the registry existed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveModelSpec {
    pub name: String,
    pub version: i64,
    pub model_json: String,
    pub model_version_label: String,
    #[serde(default)]
    pub tf_snapshot_id: Option<String>,
}

/// A resolution refusal. Every variant is a `config`- or `precondition`-class
/// failure: nothing here is retryable, because re-running the same job against
/// the same tenant state would refuse again.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("org env missing {0}")]
    MissingEnv(&'static str),
    #[error("reading {what} at {path}")]
    Read {
        what: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the published config does not parse")]
    Config(#[source] er_config::ConfigError),
    #[error("{0}")]
    Refused(String),
}

/// Everything a pipeline stage needs, owned so it can cross a process
/// boundary's worth of lifetimes.
///
/// `Debug` is derived rather than hand-written so a refusal assertion can name
/// what it got; the settings documents it prints are tenant config, never a
/// credential (§6.4 keeps every credential out of this struct).
#[derive(Debug)]
pub struct RunContext {
    pub config: er_config::Config,
    pub lexicon: Vec<(String, String)>,
    /// `(word, dictionary)` rows from the org's `ER_IGNORE_WORDS_PATH` CSV —
    /// empty when unset, exactly as the lexicon tolerates absence.
    pub ignore_words: Vec<(String, String)>,
    /// `(source, csv)` pairs; empty under `skip_ingest` or for a kind that
    /// does not ingest.
    pub deliveries: Vec<(String, String)>,
    pub tf_path: PathBuf,
    /// The sole model's settings document — the registry's when one active
    /// version answers, the env file's otherwise.
    pub settings_json: String,
    pub model_version: String,
    pub tf_snapshot_id: String,
    /// The declared ensemble, in CONFIG order; empty means the single-model
    /// path (which `run_all_observed` builds its own one-member slice for).
    pub members: Vec<ResolvedMember>,
    /// `(name, predicate)` — the predicate comes from the org's own parsed
    /// config, never from a request (a job carrying SQL would be an injection
    /// surface).
    pub segment: Option<(String, String)>,
    pub skip_ingest: bool,
}

/// One resolved ensemble member, owned.
#[derive(Debug)]
pub struct ResolvedMember {
    pub name: String,
    pub settings_json: String,
    pub model_version: String,
    pub tf_snapshot_id: String,
}

/// What the caller wants resolved. `deliveries` is the one part that depends on
/// the caller's filesystem view rather than on the job, which is why it is a
/// flag rather than a value: the drop root is a tenant env/org field in both
/// worlds, but only an ingesting kind reads it.
pub struct ResolveRequest<'a> {
    pub lake_env: &'a Value,
    pub params: &'a Value,
    pub models: &'a [ActiveModelSpec],
    /// `None` refuses an ingesting run with no drop root, exactly as before.
    pub drop_root: Option<&'a str>,
    /// Scope from the work order, not from client-supplied params.
    pub dataset_id: Option<&'a str>,
    /// `false` for a kind that never ingests (`correct`, `lake_maintain`),
    /// which then needs no drop root at all.
    pub wants_deliveries: bool,
}

fn env_str<'a>(env: &'a Value, key: &str) -> Option<&'a str> {
    env.get(key).and_then(Value::as_str)
}

/// Load a lexicon file into `(term, canonical)` pairs, tolerating absence —
/// the same shape the dispatcher's loader has always had.
fn load_lexicon(path: &str) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, ',');
            let term = parts.next()?.trim();
            let canonical = parts.next()?.trim();
            (!term.is_empty() && !canonical.is_empty())
                .then(|| (term.to_string(), canonical.to_string()))
        })
        .collect()
}

/// The MATCH labels a training pass learns from, as the control plane filtered
/// them and the work order carries them.
///
/// Filtered server-side, and the filter is the point: only `verdict = "match"`
/// rows, and never `origin = "llm"` — a model must not be trained on its own
/// suggestions, and the exclusion is by ORIGIN rather than by `labeled_by` so
/// that a future second LLM producer is caught by it too.
///
/// A pod cannot read `pair_labels` (§6.4), and an empty list is not a failure:
/// EM is a complete training method on its own and is what every train did
/// before labels existed.
#[must_use]
pub fn labels_from_order(labels: &Value) -> Vec<(String, String)> {
    serde_json::from_value(labels.clone()).unwrap_or_default()
}

/// The work order's `models` field as the resolver takes it.
///
/// A malformed or absent list resolves to empty rather than failing: empty is
/// the documented "no active version" state that falls back to the env model
/// file, and an order minted before the field existed must keep working.
#[must_use]
pub fn models_from_order(models: &Value) -> Vec<ActiveModelSpec> {
    serde_json::from_value(models.clone()).unwrap_or_default()
}

impl RunContext {
    /// Resolve one job's inputs.
    ///
    /// # Errors
    /// A missing env key, an unreadable config/model file, a config that does
    /// not parse, an ambiguous active-model set, a declared member with no
    /// active version, a named segment the config does not declare, or an
    /// ingesting run with no deliveries.
    pub fn resolve(req: &ResolveRequest<'_>) -> Result<Self, ResolveError> {
        let config = Self::load_config(req.lake_env)?;

        let skip_ingest = req
            .params
            .get("skip_ingest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let deliveries = if req.wants_deliveries && !skip_ingest {
            Self::find_deliveries(&config, req)?
        } else {
            Vec::new()
        };

        // The TF snapshot is still a file on disk: the registry stores the
        // settings document and the identity it stamps, not the frozen term
        // frequencies, so `ER_TF_PATH` stays the source for those.
        let tf_path = PathBuf::from(
            env_str(req.lake_env, "ER_TF_PATH").ok_or(ResolveError::MissingEnv("ER_TF_PATH"))?,
        );
        let lexicon = env_str(req.lake_env, "ER_LEXICON_PATH")
            .map(load_lexicon)
            .unwrap_or_default();
        let ignore_words = env_str(req.lake_env, "ER_IGNORE_WORDS_PATH")
            .map(load_lexicon)
            .unwrap_or_default();

        let (settings_json, model_version, tf_snapshot_id) =
            Self::sole_model(req.lake_env, req.models, &config)?;
        let members = Self::resolve_members(&config, req.models)?;
        let segment = Self::resolve_segment(req.params, &config)?;

        Ok(RunContext {
            config,
            lexicon,
            ignore_words,
            deliveries,
            tf_path,
            settings_json,
            model_version,
            tf_snapshot_id,
            members,
            segment,
            skip_ingest,
        })
    }

    fn load_config(lake_env: &Value) -> Result<er_config::Config, ResolveError> {
        let path = env_str(lake_env, "ER_CONFIG_PATH")
            .ok_or(ResolveError::MissingEnv("ER_CONFIG_PATH"))?;
        let text = std::fs::read_to_string(path).map_err(|source| ResolveError::Read {
            what: "the published config",
            path: path.to_string(),
            source,
        })?;
        er_config::load_config_str(&text).map_err(ResolveError::Config)
    }

    /// Every declared source that has a delivery waiting under the drop root.
    ///
    /// `{"skip_ingest": true}` — set by every publish-triggered rebuild —
    /// means "re-run the pipeline over the corpus already landed", so the
    /// caller skips this entirely rather than failing on a cleaned-up drop
    /// directory.
    fn find_deliveries(
        config: &er_config::Config,
        req: &ResolveRequest<'_>,
    ) -> Result<Vec<(String, String)>, ResolveError> {
        let drop_root = req
            .drop_root
            .ok_or_else(|| ResolveError::Refused("org has no drop_root for a run_all".into()))?;
        if let Some(value) = req.params.get("delivery") {
            let pin: crate::delivery::DeliveryPin = serde_json::from_value(value.clone())
                .map_err(|_| ResolveError::Refused("invalid pinned delivery".into()))?;
            if req.params.get("source").and_then(Value::as_str) != Some(pin.source.as_str())
                || req.params.get("delivery_id").and_then(Value::as_str)
                    != Some(pin.delivery_id.as_str())
            {
                return Err(ResolveError::Refused(
                    "delivery identity differs from job params".into(),
                ));
            }
            let spec = config.sources.get(&pin.source).ok_or_else(|| {
                ResolveError::Refused("delivery source no longer declared".into())
            })?;
            let dataset_id = req.dataset_id.ok_or_else(|| {
                ResolveError::Refused("pinned delivery requires a dataset".into())
            })?;
            let csv = pin
                .read(Path::new(drop_root), dataset_id, spec)
                .map_err(|e| ResolveError::Refused(crate::errfmt::chain(&e)))?;
            return Ok(vec![(pin.source, csv)]);
        }
        if req.params.get("source").is_some() || req.params.get("delivery_id").is_some() {
            return Err(ResolveError::Refused(
                "legacy delivery job has no immutable pin; re-deliver the original input".into(),
            ));
        }
        // Explicit operator run-all without a delivery identity retains the
        // legacy drop-directory convention. Accepted inputs are never aliases
        // here; use skip_ingest to rebuild the already-landed corpus.
        let found: Vec<(String, String)> = config
            .sources
            .keys()
            .filter(|source| crate::delivery::safe_source(source))
            .filter_map(|source| {
                std::fs::read_to_string(format!("{drop_root}/{source}.csv"))
                    .ok()
                    .map(|csv| (source.clone(), csv))
            })
            .collect();
        if found.is_empty() {
            return Err(ResolveError::Refused(format!(
                "no source deliveries found under {drop_root}"
            )));
        }
        Ok(found)
    }

    /// The sole model's settings: registry first, env file second.
    ///
    /// The fallback is what keeps an org created before the registry existed
    /// scoring exactly as it did. With no `models:` block the org scores
    /// against its ONE active version; several active names and no block is
    /// not a default anyone may pick for the tenant — activation became
    /// per-name in E5b-1, so this state is reachable, and silently choosing
    /// one model is the class of bug this program keeps closing.
    fn sole_model(
        lake_env: &Value,
        models: &[ActiveModelSpec],
        config: &er_config::Config,
    ) -> Result<(String, String, String), ResolveError> {
        let active = if config.models.is_empty() {
            if models.len() > 1 {
                let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
                return Err(ResolveError::Refused(format!(
                    "the registry has {} active models ({}) but the published config declares no \
                     `models:` list, so which one scores is undefined; declare the ensemble or \
                     archive the versions you do not want",
                    models.len(),
                    names.join(", ")
                )));
            }
            models.first()
        } else {
            None
        };
        if let Some(m) = active {
            return Ok((
                m.model_json.clone(),
                m.model_version_label.clone(),
                m.tf_snapshot_id.clone().unwrap_or_else(|| "tf".into()),
            ));
        }
        let path =
            env_str(lake_env, "ER_MODEL_PATH").ok_or(ResolveError::MissingEnv("ER_MODEL_PATH"))?;
        let settings = std::fs::read_to_string(path).map_err(|source| ResolveError::Read {
            what: "the model settings",
            path: path.to_string(),
            source,
        })?;
        Ok((
            settings,
            env_str(lake_env, "ER_MODEL_VERSION")
                .unwrap_or("v0001")
                .to_string(),
            env_str(lake_env, "ER_TF_SNAPSHOT_ID")
                .unwrap_or("tf")
                .to_string(),
        ))
    }

    /// The published config's `models:` list resolved against the active set,
    /// in CONFIG order — the config is the selector, the registry supplies the
    /// versions. So retraining a member is an activate, not a publish.
    fn resolve_members(
        config: &er_config::Config,
        models: &[ActiveModelSpec],
    ) -> Result<Vec<ResolvedMember>, ResolveError> {
        config
            .models
            .iter()
            .map(|spec| {
                let found = models.iter().find(|m| m.name == spec.name).ok_or_else(|| {
                    let active: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
                    ResolveError::Refused(format!(
                        "the config declares model '{}' but the registry has no ACTIVE version of \
                         that name (active: {})",
                        spec.name,
                        if active.is_empty() {
                            "none".to_string()
                        } else {
                            active.join(", ")
                        }
                    ))
                })?;
                Ok(ResolvedMember {
                    name: found.name.clone(),
                    settings_json: found.model_json.clone(),
                    model_version: found.model_version_label.clone(),
                    tf_snapshot_id: found.tf_snapshot_id.clone().unwrap_or_else(|| "tf".into()),
                })
            })
            .collect()
    }

    /// `{"segment": "<name>"}` restricts the match pass to a slice of the
    /// corpus (#54). The job carries the NAME; the predicate is looked up in
    /// the org's own config, which was parsed and column-checked at publish.
    /// The enqueueing endpoint validates the name too — this is the second
    /// check, because a schedule stores its params and the config can be
    /// republished without it.
    fn resolve_segment(
        params: &Value,
        config: &er_config::Config,
    ) -> Result<Option<(String, String)>, ResolveError> {
        let Some(name) = params.get("segment").and_then(Value::as_str) else {
            return Ok(None);
        };
        let predicate = config.segments.get(name).ok_or_else(|| {
            let known: Vec<&str> = config.segments.keys().map(String::as_str).collect();
            ResolveError::Refused(format!(
                "segment '{name}' is not in the active config (known: {})",
                if known.is_empty() {
                    "none declared".to_string()
                } else {
                    known.join(", ")
                }
            ))
        })?;
        Ok(Some((name.to_string(), predicate.clone())))
    }

    /// The borrowed engine view of the sole model.
    #[must_use]
    pub fn model(&self) -> er_cli::MatchModel<'_> {
        er_cli::MatchModel {
            settings_json: &self.settings_json,
            tf_csv_path: Path::new(&self.tf_path),
            model_version: &self.model_version,
            tf_snapshot_id: &self.tf_snapshot_id,
        }
    }

    /// The borrowed engine view of the declared ensemble (empty for the
    /// single-model path).
    #[must_use]
    pub fn ensemble(&self) -> Vec<er_cli::EnsembleMember<'_>> {
        self.members
            .iter()
            .map(|m| er_cli::EnsembleMember {
                model: er_cli::MatchModel {
                    settings_json: &m.settings_json,
                    tf_csv_path: Path::new(&self.tf_path),
                    model_version: &m.model_version,
                    tf_snapshot_id: &m.tf_snapshot_id,
                },
                name: &m.name,
            })
            .collect()
    }

    /// The segment as the engine takes it.
    #[must_use]
    pub fn segment_ref(&self) -> Option<(&str, &str)> {
        self.segment
            .as_ref()
            .map(|(name, predicate)| (name.as_str(), predicate.as_str()))
    }

    /// The identity the ledger records: the ENSEMBLE's when one is declared,
    /// the sole model's otherwise. That string is what the drift guard compares
    /// across runs.
    #[must_use]
    pub fn identity(&self) -> (String, String) {
        if self.members.is_empty() {
            (self.model_version.clone(), self.tf_snapshot_id.clone())
        } else {
            er_cli::ensemble_identity(&self.ensemble())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONFIG: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("er-runctx-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// A lake env pointing at a real config, with the files a resolve needs.
    fn env_for(dir: &Path) -> Value {
        let config = dir.join("acme.yaml");
        std::fs::write(&config, CONFIG).unwrap();
        std::fs::write(dir.join("tf.csv"), "term,freq\n").unwrap();
        std::fs::write(dir.join("model.json"), r#"{"from":"file"}"#).unwrap();
        json!({
            "ER_CONFIG_PATH": config.to_string_lossy(),
            "ER_TF_PATH": dir.join("tf.csv").to_string_lossy(),
            "ER_MODEL_PATH": dir.join("model.json").to_string_lossy(),
        })
    }

    fn spec(name: &str) -> ActiveModelSpec {
        ActiveModelSpec {
            name: name.to_string(),
            version: 1,
            model_json: format!(r#"{{"from":"registry","name":"{name}"}}"#),
            model_version_label: format!("{name}-v3"),
            tf_snapshot_id: Some("tf-7".into()),
        }
    }

    fn request<'a>(
        lake_env: &'a Value,
        params: &'a Value,
        models: &'a [ActiveModelSpec],
    ) -> ResolveRequest<'a> {
        ResolveRequest {
            lake_env,
            params,
            models,
            drop_root: None,
            dataset_id: None,
            wants_deliveries: false,
        }
    }

    #[test]
    fn one_active_version_and_no_models_block_scores_from_the_registry() {
        let dir = scratch("registry");
        let env = env_for(&dir);
        let models = [spec("people")];
        let ctx = RunContext::resolve(&request(&env, &json!({}), &models)).expect("resolves");
        assert_eq!(ctx.settings_json, models[0].model_json);
        assert_eq!(ctx.model_version, "people-v3");
        assert_eq!(ctx.tf_snapshot_id, "tf-7");
        assert!(
            ctx.members.is_empty(),
            "no `models:` block means the single-model path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_active_version_falls_back_to_the_env_model_file() {
        let dir = scratch("fallback");
        let env = env_for(&dir);
        let ctx = RunContext::resolve(&request(&env, &json!({}), &[])).expect("resolves");
        assert_eq!(ctx.settings_json, r#"{"from":"file"}"#);
        assert_eq!(ctx.model_version, "v0001", "the documented env default");
        assert_eq!(ctx.tf_snapshot_id, "tf");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_active_versions_with_no_models_block_is_refused() {
        let dir = scratch("ambiguous");
        let env = env_for(&dir);
        let models = [spec("people"), spec("firms")];
        let err = RunContext::resolve(&request(&env, &json!({}), &models)).expect_err("refused");
        let text = crate::errfmt::chain(&err);
        assert!(text.contains("which one scores is undefined"), "{text}");
        assert!(text.contains("people") && text.contains("firms"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_segment_names_the_declared_ones() {
        let dir = scratch("segment");
        let env = env_for(&dir);
        let err = RunContext::resolve(&request(&env, &json!({"segment": "nope"}), &[]))
            .expect_err("refused");
        let text = crate::errfmt::chain(&err);
        assert!(
            text.contains("segment 'nope' is not in the active config"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_ingest_needs_no_drop_root() {
        let dir = scratch("skip");
        let env = env_for(&dir);
        let params = json!({"skip_ingest": true});
        let ctx = RunContext::resolve(&ResolveRequest {
            lake_env: &env,
            params: &params,
            models: &[],
            drop_root: None,
            dataset_id: None,
            wants_deliveries: true,
        })
        .expect("resolves");
        assert!(ctx.skip_ingest);
        assert_eq!(ctx.deliveries, Vec::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ingesting_run_with_no_drop_root_is_refused() {
        let dir = scratch("nodrop");
        let env = env_for(&dir);
        let err = RunContext::resolve(&ResolveRequest {
            lake_env: &env,
            params: &json!({}),
            models: &[],
            drop_root: None,
            dataset_id: None,
            wants_deliveries: true,
        })
        .expect_err("refused");
        assert!(crate::errfmt::chain(&err).contains("no drop_root"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deliveries_are_the_declared_sources_present_under_the_drop_root() {
        let dir = scratch("deliveries");
        let env = env_for(&dir);
        let drop = dir.join("drop");
        std::fs::create_dir_all(&drop).unwrap();
        // The fixture config's first source, plus a file no source declares.
        let config = RunContext::load_config(&env).expect("config");
        let source = config.sources.keys().next().expect("a source").clone();
        std::fs::write(drop.join(format!("{source}.csv")), "id\n1\n").unwrap();
        std::fs::write(drop.join("undeclared.csv"), "id\n2\n").unwrap();
        let ctx = RunContext::resolve(&ResolveRequest {
            lake_env: &env,
            params: &json!({}),
            models: &[],
            drop_root: drop.to_str(),
            dataset_id: None,
            wants_deliveries: true,
        })
        .expect("resolves");
        assert_eq!(ctx.deliveries.len(), 1, "only declared sources land");
        assert_eq!(ctx.deliveries[0].0, source);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_or_broken_delivery_jobs_never_read_the_latest_file() {
        let dir = scratch("legacy-pin");
        let env = env_for(&dir);
        std::fs::write(dir.join("crm.csv"), "id\nlatest\n").unwrap();
        for params in [
            json!({"source":"crm", "delivery_id":"old-job"}),
            json!({"delivery":null}),
            json!({"delivery":{"version":99}}),
        ] {
            let result = RunContext::resolve(&ResolveRequest {
                lake_env: &env,
                params: &params,
                models: &[],
                drop_root: dir.to_str(),
                dataset_id: Some("dataset-a"),
                wants_deliveries: true,
            });
            assert!(result.is_err(), "broken pins must fail explicitly");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_env_key_names_itself() {
        let err = RunContext::resolve(&request(&json!({}), &json!({}), &[])).expect_err("refused");
        assert!(
            crate::errfmt::chain(&err).contains("ER_CONFIG_PATH"),
            "{err}"
        );
    }

    #[test]
    fn an_absent_or_malformed_models_field_resolves_to_empty() {
        assert!(models_from_order(&Value::Null).is_empty());
        assert!(models_from_order(&json!({})).is_empty());
        assert!(models_from_order(&json!([{"name": "no-other-fields"}])).is_empty());
        let one = models_from_order(&json!([{
            "name": "people", "version": 2, "model_json": "{}",
            "model_version_label": "v2", "tf_snapshot_id": null,
        }]));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].tf_snapshot_id, None);
    }

    #[test]
    fn the_active_model_spec_round_trips_through_the_work_order() {
        let spec = spec("people");
        let wire = serde_json::to_string(&spec).expect("serialize");
        let back: ActiveModelSpec = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(back.name, spec.name);
        assert_eq!(back.model_json, spec.model_json);
        assert_eq!(back.model_version_label, spec.model_version_label);
        assert_eq!(back.tf_snapshot_id, spec.tf_snapshot_id);
    }
}

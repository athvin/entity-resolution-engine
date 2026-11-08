//! Turning an `extract` job's params into an extract request.
//!
//! Two paths read them — the enqueue endpoint's 422 validation and the DAG
//! task the executor runs — and they must agree on what a job's params mean.
//! One parser is the only way to guarantee that, the same reasoning
//! `profile_params` wrote down first.

use serde_json::Value;

/// The two pull shapes an extract can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractMode {
    /// Changed rows only: `updated_at > watermark`. The unattended default —
    /// a schedule fires with the same params forever, and the watermark is
    /// what makes that cheap.
    Incremental,
    /// Everything the query matches, watermark ignored. The first pull, and
    /// the recovery move when Salesforce-side deletes must be reflected.
    Full,
}

/// What an `extract` job asks for.
#[derive(Debug, Clone)]
pub struct ExtractRequest {
    /// Which `salesforce_connections` row authenticates the pull.
    pub connection_id: String,
    /// The sObject API name to extract.
    pub object: String,
    /// The config source the delivery lands as — names the drop-root CSV and
    /// the column mapping.
    pub source: String,
    pub mode: ExtractMode,
    /// A raw SOQL `WHERE` fragment ANDed into the query, steward-supplied.
    pub where_clause: Option<String>,
    /// Spec unit 1.26: converted leads are excluded by default — but only for
    /// `Lead`, because `IsConverted` is a Lead field and naming it on any
    /// other object is a SOQL error.
    pub exclude_converted_leads: bool,
}

impl ExtractRequest {
    /// Read a request out of `jobs.params`.
    ///
    /// # Errors
    /// Messages start `param:` — the shape `engine_error_class` classifies as
    /// a precondition, because retrying an unchanged request cannot help.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let required = |key: &str| -> Result<String, String> {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("param: {key} is required and must be a non-empty string"))
        };
        let connection_id = required("connection_id")?;
        let object = required("object")?;
        let source = required("source")?;
        let mode = match params
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("incremental")
        {
            "incremental" => ExtractMode::Incremental,
            "full" => ExtractMode::Full,
            other => {
                return Err(format!(
                    "param: mode {other:?} is neither \"incremental\" nor \"full\""
                ));
            }
        };
        let where_clause = params
            .get("where")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let exclude_converted_leads = match params.get("exclude_converted_leads") {
            None => object.eq_ignore_ascii_case("Lead"),
            Some(Value::Bool(b)) => {
                if *b && !object.eq_ignore_ascii_case("Lead") {
                    return Err(format!(
                        "param: exclude_converted_leads is a Lead rule; {object} has no \
                         IsConverted field"
                    ));
                }
                *b
            }
            Some(_) => {
                return Err("param: exclude_converted_leads must be a boolean".to_string());
            }
        };
        Ok(ExtractRequest {
            connection_id,
            object,
            source,
            mode,
            where_clause,
            exclude_converted_leads,
        })
    }
}

/// The columns a source's CSV delivery carries, in a stable order: its record
/// id, its updated-at stamp, then every mapped column. This is the order
/// `push_records` renders and the order the extract queries, factored here so
/// the two delivery producers cannot drift — a column shift between them would
/// land values in the wrong fields with no error anywhere.
#[must_use]
pub fn source_csv_columns(spec: &er_config::SourceSpec) -> Vec<String> {
    let mut columns: Vec<String> = vec![spec.record_id_column.clone()];
    if !spec.updated_at_column.is_empty() && spec.updated_at_column != spec.record_id_column {
        columns.push(spec.updated_at_column.clone());
    }
    for raw in spec.columns.values() {
        if !columns.contains(raw) {
            columns.push(raw.clone());
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_minimal_request_defaults_to_incremental() {
        let r = ExtractRequest::from_params(&json!({
            "connection_id": "c1", "object": "Contact", "source": "sf_contact",
        }))
        .unwrap();
        assert_eq!(r.mode, ExtractMode::Incremental);
        assert!(r.where_clause.is_none());
        assert!(
            !r.exclude_converted_leads,
            "IsConverted is a Lead field; defaulting it on for Contact would be a SOQL error"
        );
    }

    /// Spec unit 1.26: converted leads are excluded BY DEFAULT — the org-level
    /// behaviour DataGroomr documents — and the default is scoped to Lead.
    #[test]
    fn a_lead_extract_excludes_converted_leads_unless_told_otherwise() {
        let params = json!({ "connection_id": "c1", "object": "Lead", "source": "sf_lead" });
        assert!(
            ExtractRequest::from_params(&params)
                .unwrap()
                .exclude_converted_leads
        );

        let keep = json!({
            "connection_id": "c1", "object": "Lead", "source": "sf_lead",
            "exclude_converted_leads": false,
        });
        assert!(
            !ExtractRequest::from_params(&keep)
                .unwrap()
                .exclude_converted_leads
        );

        // Asking for the Lead rule on a non-Lead object is refused rather than
        // deferred to a Salesforce error an hour later in the queue.
        let wrong = json!({
            "connection_id": "c1", "object": "Account", "source": "sf_account",
            "exclude_converted_leads": true,
        });
        let err = ExtractRequest::from_params(&wrong).unwrap_err();
        assert!(err.starts_with("param:"), "{err}");
    }

    #[test]
    fn each_missing_required_param_is_named() {
        for key in ["connection_id", "object", "source"] {
            let mut p = json!({
                "connection_id": "c1", "object": "Lead", "source": "sf_lead",
            });
            p.as_object_mut().unwrap().remove(key);
            let err = ExtractRequest::from_params(&p).unwrap_err();
            assert!(err.contains(key), "{err}");
            assert!(err.starts_with("param:"), "{err}");
        }
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        let p = json!({
            "connection_id": "c1", "object": "Lead", "source": "sf_lead", "mode": "delta",
        });
        let err = ExtractRequest::from_params(&p).unwrap_err();
        assert!(err.contains("delta"), "{err}");
    }
}

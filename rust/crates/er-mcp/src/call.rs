//! Turning a tool call into an er-server request.
//!
//! The whole security surface of this crate is here: an argument from an MCP
//! client becomes a path segment in a URL that is then sent with the tenant's
//! API key. Everything below exists because that sentence is true.

use crate::manifest::Tool;

/// Why a call was refused before any request was made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// A required path parameter was absent.
    Missing(String),
    /// A parameter the tool does not declare.
    Unknown(String),
    /// A parameter whose value cannot be a single path segment.
    NotASegment { param: String, value: String },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "`{p}` is required"),
            Self::Unknown(p) => write!(f, "`{p}` is not a parameter of this tool"),
            Self::NotASegment { param, value } => write!(
                f,
                "`{param}` must be a single path segment; {value:?} is not"
            ),
        }
    }
}

/// Is this value safe to substitute into a path?
///
/// A `/` would let a caller reach a different route entirely —
/// `org = "acme/jobs/../../orgs"` — and `.` segments traverse. Refusing is
/// better than escaping: percent-encoding a slash produces a path that is
/// *valid* and addresses something nobody intended, whereas a refusal is a
/// sentence the caller can act on.
fn is_single_segment(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('?')
        && !value.contains('#')
        && value != "."
        && value != ".."
}

/// The path to request, with every `{param}` substituted.
///
/// # Errors
/// [`Refused`] when an argument is missing, unknown, or cannot be a segment.
pub fn build_path(
    tool: &Tool,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, Refused> {
    // Unknown arguments are refused rather than ignored. Ignoring them would
    // let a caller believe a filter was applied that never was — and the
    // schema already says `additionalProperties: false`, so accepting one here
    // would make this server disagree with the contract it published.
    for key in args.keys() {
        if !tool.params.contains(key) {
            return Err(Refused::Unknown(key.clone()));
        }
    }

    let mut path = tool.path.clone();
    for param in &tool.params {
        let value = args
            .get(param)
            .and_then(|v| v.as_str())
            .ok_or_else(|| Refused::Missing(param.clone()))?;
        if !is_single_segment(value) {
            return Err(Refused::NotASegment {
                param: param.clone(),
                value: value.to_string(),
            });
        }
        path = path.replace(&format!("{{{param}}}"), value);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> Tool {
        Tool {
            name: "get_orgs_jobs_one".into(),
            method: "GET".into(),
            path: "/v1/orgs/{org}/jobs/{job_id}".into(),
            role: "viewer".into(),
            params: vec!["org".into(), "job_id".into()],
            needs_body: false,
            summary: "one job".into(),
        }
    }

    fn args(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), serde_json::json!(v)))
            .collect()
    }

    #[test]
    fn substitutes_every_parameter() {
        assert_eq!(
            build_path(&tool(), &args(&[("org", "acme"), ("job_id", "01jm")])).unwrap(),
            "/v1/orgs/acme/jobs/01jm"
        );
    }

    #[test]
    fn refuses_a_value_that_would_reach_another_route() {
        // THE one that matters. The argument comes from an MCP client and the
        // request is sent with the tenant's API key, so a `/` is a way to
        // address a route the tool never named.
        let err = build_path(
            &tool(),
            &args(&[("org", "acme/jobs/../../orgs"), ("job_id", "x")]),
        )
        .unwrap_err();
        assert!(matches!(err, Refused::NotASegment { .. }), "{err:?}");
        // Refused, not escaped: percent-encoding would produce a VALID path
        // addressing something nobody intended, which is worse than an error.
        assert!(err.to_string().contains("single path segment"));
    }

    #[test]
    fn refuses_traversal_segments() {
        for bad in ["..", ".", "", "a?b", "a#b", "a\\b"] {
            assert!(
                build_path(&tool(), &args(&[("org", bad), ("job_id", "x")])).is_err(),
                "{bad:?} was accepted as a path segment"
            );
        }
    }

    #[test]
    fn refuses_a_missing_parameter() {
        let err = build_path(&tool(), &args(&[("org", "acme")])).unwrap_err();
        assert_eq!(err, Refused::Missing("job_id".into()));
    }

    #[test]
    fn refuses_an_unknown_parameter() {
        // Ignoring it would let a caller believe a filter applied that never
        // did — and the published schema says `additionalProperties: false`,
        // so accepting one would make this server contradict its own contract.
        let err = build_path(
            &tool(),
            &args(&[("org", "acme"), ("job_id", "x"), ("limit", "5")]),
        )
        .unwrap_err();
        assert_eq!(err, Refused::Unknown("limit".into()));
    }

    #[test]
    fn a_tool_with_no_parameters_needs_no_arguments() {
        let listing = Tool {
            name: "get_healthz".into(),
            method: "GET".into(),
            path: "/healthz".into(),
            role: "public".into(),
            params: vec![],
            needs_body: false,
            summary: "liveness".into(),
        };
        assert_eq!(build_path(&listing, &args(&[])).unwrap(), "/healthz");
    }
}

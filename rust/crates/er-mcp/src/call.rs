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

/// What a validated tool call becomes: a path, and a body when the route
/// takes one.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub path: String,
    /// Forwarded verbatim. This server does NOT validate the body against the
    /// derived schema — er-server does, and it is the authority. Re-checking
    /// here would mean two validators that can disagree, and the one a tenant
    /// relies on is the server's.
    pub body: Option<serde_json::Value>,
}

/// The request to make, with every `{param}` substituted.
///
/// # Errors
/// [`Refused`] when an argument is missing, unknown, or cannot be a segment.
pub fn build_request(
    tool: &Tool,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<Request, Refused> {
    // Unknown arguments are refused rather than ignored. Ignoring them would
    // let a caller believe a field was applied that never was — and the schema
    // says `additionalProperties: false`, so accepting one here would make
    // this server disagree with the contract it published.
    let takes_body = tool.body.is_some();
    for key in args.keys() {
        if tool.params.contains(key) {
            continue;
        }
        if takes_body && key == "body" {
            continue;
        }
        return Err(Refused::Unknown(key.clone()));
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

    let body = if takes_body {
        Some(
            args.get("body")
                .ok_or_else(|| Refused::Missing("body".to_string()))?
                .clone(),
        )
    } else {
        None
    };
    Ok(Request { path, body })
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
            body: None,
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
            build_request(&tool(), &args(&[("org", "acme"), ("job_id", "01jm")]))
                .unwrap()
                .path,
            "/v1/orgs/acme/jobs/01jm"
        );
    }

    #[test]
    fn refuses_a_value_that_would_reach_another_route() {
        // THE one that matters. The argument comes from an MCP client and the
        // request is sent with the tenant's API key, so a `/` is a way to
        // address a route the tool never named.
        let err = build_request(
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
                build_request(&tool(), &args(&[("org", bad), ("job_id", "x")])).is_err(),
                "{bad:?} was accepted as a path segment"
            );
        }
    }

    #[test]
    fn refuses_a_missing_parameter() {
        let err = build_request(&tool(), &args(&[("org", "acme")])).unwrap_err();
        assert_eq!(err, Refused::Missing("job_id".into()));
    }

    #[test]
    fn refuses_an_unknown_parameter() {
        // Ignoring it would let a caller believe a filter applied that never
        // did — and the published schema says `additionalProperties: false`,
        // so accepting one would make this server contradict its own contract.
        let err = build_request(
            &tool(),
            &args(&[("org", "acme"), ("job_id", "x"), ("limit", "5")]),
        )
        .unwrap_err();
        assert_eq!(err, Refused::Unknown("limit".into()));
    }

    fn with_body() -> Tool {
        Tool {
            name: "post_orgs_by_org_tags".into(),
            method: "POST".into(),
            path: "/v1/orgs/{org}/tags".into(),
            role: "steward".into(),
            params: vec!["org".into()],
            needs_body: false,
            body: Some(serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            })),
            summary: "add a label to the vocabulary".into(),
        }
    }

    #[test]
    fn a_body_rides_under_its_own_key() {
        // Under `body`, not flattened alongside the path parameters. A body
        // field named `org` — and `OrgIn` has one — would otherwise collide
        // with the path parameter, and whichever won would silently not be
        // what the caller meant.
        let mut args = args(&[("org", "acme")]);
        args.insert(
            "body".to_string(),
            serde_json::json!({ "name": "needs-review" }),
        );
        let request = build_request(&with_body(), &args).unwrap();
        assert_eq!(request.path, "/v1/orgs/acme/tags");
        assert_eq!(
            request.body,
            Some(serde_json::json!({ "name": "needs-review" }))
        );
    }

    #[test]
    fn a_route_that_takes_a_body_refuses_a_call_without_one() {
        let err = build_request(&with_body(), &args(&[("org", "acme")])).unwrap_err();
        assert_eq!(err, Refused::Missing("body".into()));
    }

    #[test]
    fn a_route_that_takes_no_body_refuses_one() {
        // `body` is only a legal argument where the manifest says the route
        // has one. Accepting it elsewhere would let a caller attach a payload
        // to a GET, which the published schema forbids.
        let mut args = args(&[("org", "acme"), ("job_id", "x")]);
        args.insert("body".to_string(), serde_json::json!({}));
        assert_eq!(
            build_request(&tool(), &args).unwrap_err(),
            Refused::Unknown("body".into())
        );
    }

    #[test]
    fn a_path_parameter_is_still_guarded_when_a_body_is_present() {
        // The body must not distract from the segment check — that guard is
        // what stops an argument reaching a route the tool never named.
        let mut args = args(&[("org", "a/b")]);
        args.insert("body".to_string(), serde_json::json!({ "name": "x" }));
        assert!(matches!(
            build_request(&with_body(), &args).unwrap_err(),
            Refused::NotASegment { .. }
        ));
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
            body: None,
            summary: "liveness".into(),
        };
        assert_eq!(
            build_request(&listing, &args(&[])).unwrap().path,
            "/healthz"
        );
    }
}

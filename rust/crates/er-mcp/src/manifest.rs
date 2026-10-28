//! The tool manifest, and the rules for which tools an MCP client is offered.
//!
//! The manifest is **derived from er-server's handlers**, never written here —
//! `er-server/tests/mcp_manifest.rs` builds it from their doc comments and
//! fails if the committed copy drifts. That choice is the whole design, and it
//! is the repo's own: `response_envelopes.rs` already recorded that "a
//! `#[utoipa::path]` response body, or any hand-maintained table, is one more
//! claim that can drift from the code — the same shape as the defect it would
//! be built to prevent." A hand-written tool list would be 81 such claims.

use serde::Deserialize;

/// One routed handler, as the manifest records it.
#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    pub name: String,
    pub method: String,
    pub path: String,
    /// The minimum role er-server requires. Carried so this server can refuse
    /// a call its credential cannot satisfy *before* making it — a 403 from
    /// upstream is a correct answer, but an agent that learns the boundary
    /// from the tool list wastes fewer turns discovering it.
    pub role: String,
    pub params: Vec<String>,
    /// A request body this manifest cannot describe. See [`offered`].
    pub needs_body: bool,
    /// The request body's JSON Schema, derived from the handler's `Json<T>`
    /// (O10b). `None` when the route takes no body.
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    tools: Vec<Tool>,
}

/// The committed manifest, gated by `er-server/tests/mcp_manifest.rs`.
const TOOLS_JSON: &str = include_str!("../tools.json");

/// Every tool this server will offer.
///
/// # What is deliberately left out, and why
///
/// **Routes whose body is not JSON.** Exactly one: the CSV upload takes
/// `Multipart`, and a file is not something an MCP client can put in a tool
/// call. The JSON push (`records:ingest`) is the route an agent should use,
/// and it IS offered.
///
/// Every other body is described (O10b). The schema comes from the handler's
/// `Json<T>` struct — field names, types, and which are optional — derived
/// from the same source as everything else here, so it cannot claim a shape
/// the server will reject.
///
/// **Operator-role routes.** They are fleet-wide — create an org, list every
/// org, send mail — and this server authenticates with a single org's API key.
/// Offering them would mean either handing an agent an operator credential, or
/// advertising tools that always 403.
///
/// # Errors
/// Panics at startup if the committed manifest does not parse, which is a
/// build-time fact rather than a runtime one.
#[must_use]
pub fn offered() -> Vec<Tool> {
    let manifest: Manifest =
        serde_json::from_str(TOOLS_JSON).expect("the committed tool manifest parses");
    manifest
        .tools
        .into_iter()
        .filter(|t| !t.needs_body && t.role != "operator")
        .collect()
}

/// Whether a tool only reads.
///
/// Used to label the mutating ones in their description rather than to hide
/// them: an agent should be able to cancel a runaway job, and refusing every
/// mutation would make this a strictly worse interface than the CLI it sits
/// beside. What it must not do is let one look like a read.
#[must_use]
pub fn is_read_only(tool: &Tool) -> bool {
    tool.method == "GET"
}

/// The JSON Schema an MCP client needs to call `tool`.
///
/// Every path parameter is required — a URL cannot be built without one — and
/// nothing else is accepted. `additionalProperties: false` is load-bearing: it
/// is what stops a caller smuggling a field this server has not checked into a
/// request it then signs with the org's credential.
///
/// A route with a request body takes it under one `body` property carrying the
/// derived schema, rather than flattening its fields alongside the path
/// parameters. Flattening would be friendlier to read and wrong: a body field
/// named `org` — and `OrgIn` has one — would collide with the path parameter,
/// and whichever won would be silently not what the caller meant.
#[must_use]
pub fn input_schema(tool: &Tool) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    for param in &tool.params {
        properties.insert(
            param.clone(),
            serde_json::json!({
                "type": "string",
                "description": format!("the `{param}` path segment"),
            }),
        );
    }
    let mut required: Vec<String> = tool.params.clone();
    if let Some(body) = &tool.body {
        properties.insert("body".to_string(), body.clone());
        required.push("body".to_string());
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The description an MCP client shows, with the mutation warning first.
///
/// First, not appended: a description is often truncated in a client's tool
/// picker, and "irreversible" read at the end of a long sentence is a warning
/// nobody saw.
#[must_use]
pub fn description(tool: &Tool) -> String {
    if is_read_only(tool) {
        tool.summary.clone()
    } else {
        format!("[MUTATES — {} {}] {}", tool.method, tool.path, tool.summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_parses_and_is_not_empty() {
        // A manifest that failed to parse, or shrank to nothing, would make
        // every assertion below vacuous.
        let tools = offered();
        assert!(
            tools.len() > 30,
            "only {} tools offered; the manifest is stale or the filter is \
             wrong",
            tools.len()
        );
    }

    #[test]
    fn no_offered_tool_needs_a_body_or_operator_role() {
        // Both exclusions, asserted rather than trusted: a body-needing tool
        // cannot be called correctly, and an operator tool cannot be called at
        // all with an org credential.
        for tool in offered() {
            assert!(!tool.needs_body, "{tool:?} needs a body it cannot describe");
            assert_ne!(tool.role, "operator", "{tool:?} is fleet-wide");
        }
    }

    #[test]
    fn a_mutating_tool_says_so_before_anything_else() {
        let mutating = Tool {
            name: "post_orgs_jobs_cancel".into(),
            method: "POST".into(),
            path: "/v1/orgs/{org}/jobs/{job_id}/cancel".into(),
            role: "steward".into(),
            params: vec!["org".into(), "job_id".into()],
            needs_body: false,
            body: None,
            summary: "cancel a queued job".into(),
        };
        let text = description(&mutating);
        assert!(
            text.starts_with("[MUTATES"),
            "the warning must come FIRST — a client truncating the description \
             would otherwise drop it: {text}"
        );
        assert!(!is_read_only(&mutating));
    }

    #[test]
    fn a_read_tool_carries_no_warning() {
        let read = Tool {
            name: "get_orgs_jobs".into(),
            method: "GET".into(),
            path: "/v1/orgs/{org}/jobs".into(),
            role: "viewer".into(),
            params: vec!["org".into()],
            needs_body: false,
            body: None,
            summary: "the org's jobs".into(),
        };
        assert_eq!(description(&read), "the org's jobs");
        assert!(is_read_only(&read));
    }

    #[test]
    fn the_input_schema_requires_every_path_parameter_and_nothing_else() {
        let tool = Tool {
            name: "get_orgs_jobs_one".into(),
            method: "GET".into(),
            path: "/v1/orgs/{org}/jobs/{job_id}".into(),
            role: "viewer".into(),
            params: vec!["org".into(), "job_id".into()],
            needs_body: false,
            body: None,
            summary: "one job".into(),
        };
        let schema = input_schema(&tool);
        assert_eq!(schema["required"], serde_json::json!(["org", "job_id"]));
        // THE one that matters: without this a caller could add fields this
        // server never validated to a request it signs with the org's key.
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn every_offered_tool_has_a_schema_covering_its_path() {
        // A tool whose schema omits a path segment cannot be called at all,
        // and the failure would be a malformed URL rather than a clear refusal.
        for tool in offered() {
            let schema = input_schema(&tool);
            let required = schema["required"].as_array().expect("required is a list");
            for segment in tool.path.split('/') {
                if let Some(param) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                    assert!(
                        required.iter().any(|r| r == param),
                        "{} does not require `{param}`",
                        tool.name
                    );
                }
            }
        }
    }
}

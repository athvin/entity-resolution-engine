//! The JSON-RPC 2.0 half of MCP: `initialize`, `tools/list`, `tools/call`.
//!
//! Kept apart from the transport and from the HTTP client so the protocol can
//! be tested without a socket in either direction — the dispatch below is a
//! pure function of a request and a tool list.

use crate::call::build_path;
use crate::manifest::{self, Tool};

/// The MCP revision this server speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// What a dispatched request wants done.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Answer with this JSON-RPC result.
    Reply(serde_json::Value),
    /// Fetch `path` from er-server, then reply with the body.
    Fetch { id: serde_json::Value, path: String },
    /// A notification: there is nothing to answer.
    Silent,
}

fn result(id: &serde_json::Value, value: serde_json::Value) -> Action {
    Action::Reply(serde_json::json!({
        "jsonrpc": "2.0", "id": id, "result": value
    }))
}

/// A JSON-RPC error. `code` follows the spec's reserved range so a client can
/// tell a protocol fault from a tool that ran and refused.
fn error(id: &serde_json::Value, code: i64, message: String) -> Action {
    Action::Reply(serde_json::json!({
        "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
    }))
}

/// A tool-level failure, reported as a RESULT with `isError`, not a JSON-RPC
/// error.
///
/// The distinction is in the MCP spec and it matters: a protocol error means
/// the client did something malformed and the model never sees it, while
/// `isError` is content the model reads and can correct from. A refused
/// argument is the model's to fix, so it must arrive as content.
fn tool_error(id: &serde_json::Value, message: String) -> Action {
    result(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    )
}

/// Route one JSON-RPC request.
#[must_use]
pub fn dispatch(request: &serde_json::Value, tools: &[Tool]) -> Action {
    let method = request.get("method").and_then(serde_json::Value::as_str);
    let id = request.get("id").cloned();

    // No `id` means a notification. The spec forbids answering one, and
    // `notifications/initialized` is sent by every client right after the
    // handshake — replying to it is a protocol violation clients report as a
    // hard error.
    let Some(id) = id else {
        return Action::Silent;
    };

    match method {
        Some("initialize") => result(
            &id,
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                // Tools only. No `resources` or `prompts`: declaring a
                // capability this server does not implement makes a client
                // call it and get a method-not-found it cannot distinguish
                // from a bug.
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "er-mcp", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        Some("tools/list") => result(
            &id,
            serde_json::json!({
                "tools": tools.iter().map(|t| serde_json::json!({
                    "name": t.name,
                    "description": manifest::description(t),
                    "inputSchema": manifest::input_schema(t),
                })).collect::<Vec<_>>()
            }),
        ),
        Some("tools/call") => {
            let params = request.get("params");
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let Some(tool) = tools.iter().find(|t| t.name == name) else {
                // A tool error, not a protocol error: the model chose the name
                // and can choose another.
                return tool_error(&id, format!("no tool named `{name}`"));
            };
            let empty = serde_json::Map::new();
            let args = params
                .and_then(|p| p.get("arguments"))
                .and_then(serde_json::Value::as_object)
                .unwrap_or(&empty);
            match build_path(tool, args) {
                Ok(path) => Action::Fetch { id, path },
                Err(refused) => tool_error(&id, refused.to_string()),
            }
        }
        Some(other) => error(&id, -32601, format!("method not found: {other}")),
        None => error(&id, -32600, "invalid request: no method".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<Tool> {
        vec![
            Tool {
                name: "get_orgs_jobs".into(),
                method: "GET".into(),
                path: "/v1/orgs/{org}/jobs".into(),
                role: "viewer".into(),
                params: vec!["org".into()],
                needs_body: false,
                summary: "the org's jobs".into(),
            },
            Tool {
                name: "post_orgs_suspend".into(),
                method: "POST".into(),
                path: "/v1/orgs/{org}/suspend".into(),
                role: "operator".into(),
                params: vec!["org".into()],
                needs_body: false,
                summary: "suspend the tenant".into(),
            },
        ]
    }

    fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    #[test]
    fn initialize_declares_only_what_it_implements() {
        let Action::Reply(reply) = dispatch(&req("initialize", serde_json::json!({})), &tools())
        else {
            panic!("initialize must reply");
        };
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        // Declaring `resources` would make a client call a method this server
        // answers with -32601, which it cannot tell from a bug.
        assert!(reply["result"]["capabilities"]["resources"].is_null());
        assert!(reply["result"]["capabilities"]["prompts"].is_null());
    }

    #[test]
    fn a_notification_is_never_answered() {
        // `notifications/initialized` arrives from every client right after the
        // handshake. Replying to a request with no `id` is a protocol
        // violation clients report as a hard error.
        let notification = serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        });
        assert_eq!(dispatch(&notification, &tools()), Action::Silent);
    }

    #[test]
    fn tools_list_carries_a_schema_and_a_mutation_warning() {
        let Action::Reply(reply) = dispatch(&req("tools/list", serde_json::json!({})), &tools())
        else {
            panic!("tools/list must reply");
        };
        let listed = reply["result"]["tools"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "get_orgs_jobs");
        assert_eq!(listed[0]["description"], "the org's jobs");
        assert_eq!(listed[0]["inputSchema"]["additionalProperties"], false);
        assert!(
            listed[1]["description"]
                .as_str()
                .unwrap()
                .starts_with("[MUTATES"),
            "a mutating tool must say so first"
        );
    }

    #[test]
    fn a_call_becomes_a_fetch() {
        let request = req(
            "tools/call",
            serde_json::json!({ "name": "get_orgs_jobs", "arguments": { "org": "acme" } }),
        );
        assert_eq!(
            dispatch(&request, &tools()),
            Action::Fetch {
                id: serde_json::json!(1),
                path: "/v1/orgs/acme/jobs".into()
            }
        );
    }

    #[test]
    fn a_refused_argument_is_tool_content_not_a_protocol_error() {
        // The distinction is in the spec and it matters: a protocol error never
        // reaches the model, so it cannot correct from it. A bad argument is
        // the model's to fix.
        let request = req(
            "tools/call",
            serde_json::json!({ "name": "get_orgs_jobs", "arguments": { "org": "a/b" } }),
        );
        let Action::Reply(reply) = dispatch(&request, &tools()) else {
            panic!("a refusal must reply");
        };
        assert!(reply["error"].is_null(), "must not be a JSON-RPC error");
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("single path segment")
        );
    }

    #[test]
    fn an_unknown_tool_is_also_tool_content() {
        let request = req("tools/call", serde_json::json!({ "name": "nope" }));
        let Action::Reply(reply) = dispatch(&request, &tools()) else {
            panic!("must reply");
        };
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply["error"].is_null());
    }

    #[test]
    fn an_unknown_method_is_a_protocol_error() {
        let Action::Reply(reply) =
            dispatch(&req("resources/list", serde_json::json!({})), &tools())
        else {
            panic!("must reply");
        };
        assert_eq!(reply["error"]["code"], -32601);
    }
}

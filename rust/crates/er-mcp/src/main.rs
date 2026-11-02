//! `er-mcp` — an MCP server over the control API, for one tenant (O10).
//!
//! An agent gets the product's reads as tools: what jobs ran, which duplicates
//! are open, what a golden record says and where each value came from. It
//! speaks JSON-RPC 2.0 over stdio, which is what a local MCP client expects.
//!
//! # The credential is one org's, deliberately
//!
//! `ER_MCP_API_KEY` is an org-scoped key issued by the normal mint path, and
//! `ER_MCP_BASE_URL` points at er-server. There is no operator mode: the
//! operator role is fleet-wide — create an org, list every org, send mail — and
//! an agent holding that key could reach every tenant. The manifest filters
//! operator routes out of the tool list for the same reason, so an agent is not
//! even told they exist.
//!
//! # What it will not do yet
//!
//! Routes that take a request body are not offered. The manifest is derived
//! from the handlers' doc comments, which give method, path and role but not
//! the shape of a `Json<CreateOrgIn>` — and a tool whose input schema is "an
//! object, good luck" is worse than no tool, because the model cannot call it
//! correctly and the failures land as 422s a human reads. Body-less mutations
//! (suspend, resume, cancel, retry, activate) ARE offered, labelled, because
//! their whole input is their path.

mod call;
mod manifest;
mod rpc;

use std::io::{BufRead, Write};

/// Read JSON-RPC requests from stdin, answer on stdout, one line each.
///
/// Line-delimited because that is what stdio MCP clients send, and because a
/// framing bug is far easier to see in a log of whole lines than in a stream.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Stdout is the JSON-RPC transport, so the subscriber must write to stderr
    // and nothing else. `er_obs::init` does; a `fmt()` default would not, and
    // one stray line here corrupts the protocol for the whole session.
    let _telemetry = er_obs::init(er_obs::Role::Mcp);
    let Ok(base) = std::env::var("ER_MCP_BASE_URL") else {
        eprintln!("er-mcp: ER_MCP_BASE_URL is required (where er-server listens)");
        return std::process::ExitCode::FAILURE;
    };
    let Ok(key) = std::env::var("ER_MCP_API_KEY") else {
        eprintln!("er-mcp: ER_MCP_API_KEY is required (an ORG-scoped key, never an operator one)");
        return std::process::ExitCode::FAILURE;
    };
    let base = base.trim_end_matches('/').to_string();

    let tools = manifest::offered();
    eprintln!(
        "er-mcp: {} tools ({} read, {} mutating) against {base}",
        tools.len(),
        tools.iter().filter(|t| manifest::is_read_only(t)).count(),
        tools.iter().filter(|t| !manifest::is_read_only(t)).count(),
    );

    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("er-mcp: building the HTTP client: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            // A parse failure has no id to answer against, so there is nothing
            // well-formed to reply with. Logged to stderr, which stdio clients
            // surface, rather than written to stdout where it would corrupt the
            // stream.
            eprintln!("er-mcp: ignoring a line that is not JSON");
            continue;
        };

        let reply = match rpc::dispatch(&request, &tools) {
            rpc::Action::Silent => continue,
            rpc::Action::Reply(value) => value,
            rpc::Action::Fetch {
                id,
                method,
                path,
                body,
            } => call(&http, &base, &key, &id, &method, &path, body).await,
        };
        if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
    std::process::ExitCode::SUCCESS
}

/// Call er-server and wrap the response as MCP tool content.
///
/// An upstream failure comes back as `isError` content rather than a JSON-RPC
/// error, so the model can read "403 requires steward role" and change what it
/// is doing. The status is included for the same reason — "it failed" is not
/// something a model can act on.
///
/// The body is forwarded verbatim. This server does not re-validate it against
/// the derived schema: er-server is the authority, and two validators that can
/// disagree is worse than one, because the one a tenant relies on is the
/// server's.
#[allow(clippy::too_many_arguments)]
async fn call(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    id: &serde_json::Value,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let url = format!("{base}{path}");
    let mut request = match method {
        "POST" => http.post(&url),
        "PUT" => http.put(&url),
        "PATCH" => http.patch(&url),
        "DELETE" => http.delete(&url),
        _ => http.get(&url),
    }
    .bearer_auth(key);
    if let Some(body) = body {
        // Serialized here rather than through `.json()`, which needs reqwest's
        // `json` feature: this crate should not widen a workspace dependency
        // to save one line, and the explicit form shows exactly what is sent.
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_string(&body).unwrap_or_else(|_| "null".to_string()));
    }
    let response = request.send().await;
    let (text, failed) = match response {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            if status.is_success() {
                (body, false)
            } else {
                (format!("{status}: {body}"), true)
            }
        }
        // The URL is not echoed: it carries the tenant's own identifiers and
        // this string goes to a model's context.
        Err(e) => (format!("could not reach er-server: {e}"), true),
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": failed,
        }
    })
}

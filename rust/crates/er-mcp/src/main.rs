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
        tracing::error!("ER_MCP_BASE_URL is required (where er-server listens)");
        return std::process::ExitCode::FAILURE;
    };
    let Ok(key) = std::env::var("ER_MCP_API_KEY") else {
        tracing::error!("ER_MCP_API_KEY is required (an ORG-scoped key, never an operator one)");
        return std::process::ExitCode::FAILURE;
    };
    let base = base.trim_end_matches('/').to_string();

    let tools = manifest::offered();
    tracing::info!(
        tools = tools.len(),
        read_only = tools.iter().filter(|t| manifest::is_read_only(t)).count(),
        mutating = tools.iter().filter(|t| !manifest::is_read_only(t)).count(),
        upstream = %base,
        "serving MCP tools"
    );

    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "building the HTTP client");
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
            // well-formed to reply with. It goes to the subscriber — which
            // writes to stderr, where stdio clients surface it — rather than to
            // stdout, where it would corrupt the protocol for the session.
            //
            // The line's CONTENT is deliberately not recorded. This is the one
            // site here that fires on untrusted input at runtime, and an MCP
            // client's malformed line can carry whatever it was about to ask
            // for — a record key, a search term, an address. §10.4's rule is
            // identifiers and counts, never values, so the length is logged and
            // the bytes are not.
            tracing::warn!(bytes = line.len(), "ignoring a line that is not JSON");
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
            match r.text().await {
                Ok(body) if status.is_success() => (body, false),
                Ok(body) => (format!("{status}: {body}"), true),
                // The body error can include the request URL and its tenant
                // identifiers, so only the status and failure class go to MCP.
                Err(_) => (
                    format!("{status}: could not read er-server response body"),
                    true,
                ),
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

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::call;

    fn mock_response(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 512];
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0, "client closed before sending request headers");
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            stream.write_all(response).unwrap();
        });
        (base, server)
    }

    #[tokio::test]
    async fn incomplete_upstream_body_is_a_tool_error() {
        let (base, server) = mock_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nshort",
        );
        let reply = call(
            &reqwest::Client::new(),
            &base,
            "test-key",
            &serde_json::json!(1),
            "GET",
            "/v1/test",
            None,
        )
        .await;
        server.join().unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("could not read er-server response body")
        );
    }

    #[tokio::test]
    async fn complete_upstream_body_is_successful_content() {
        let (base, server) =
            mock_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        let reply = call(
            &reqwest::Client::new(),
            &base,
            "test-key",
            &serde_json::json!(1),
            "GET",
            "/v1/test",
            None,
        )
        .await;
        server.join().unwrap();
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(reply["result"]["content"][0]["text"], "ok");
    }
}

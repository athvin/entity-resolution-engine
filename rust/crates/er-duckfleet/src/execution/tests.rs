use super::*;
use std::io::Write;
use std::net::TcpListener;
use std::thread;

struct Reply {
    status: u16,
    headers: &'static str,
    body: Vec<u8>,
}

fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        headers: "",
        body: serde_json::to_vec(&body).unwrap(),
    }
}

fn server(replies: Vec<Reply>) -> (Transport, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let join = thread::spawn(move || {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let end = loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break request.len();
                }
            };
            let headers = String::from_utf8(request.clone()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            request.resize(end + length, 0);
            stream.read_exact(&mut request[end..]).unwrap();
            requests.push(String::from_utf8(request).unwrap());
            if reply.status == 0 {
                continue;
            }
            write!(
                stream,
                "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
                reply.status,
                reply.body.len(),
                reply.headers
            )
            .unwrap();
            // A bounded client may close immediately after the length header.
            let _ = stream.write_all(&reply.body);
        }
        requests
    });
    (
        Transport::new(
            format!("http://{address}"),
            "private-fixture-token".to_owned(),
        )
        .unwrap(),
        join,
    )
}

fn opened() -> Reply {
    reply(
        200,
        json!({"session_id": "00000000-0000-0000-0000-000000000001"}),
    )
}

#[test]
fn redirects_never_forward_credentials() {
    let (transport, join) = server(vec![Reply {
        status: 307,
        headers: "Location: http://127.0.0.1:9/stolen\r\n",
        body: Vec::new(),
    }]);
    let error = RemoteSession::open(transport).err().unwrap();
    assert_eq!(error.kind, Failure::Protocol);
    assert!(!format!("{error:?}").contains("private-fixture-token"));
    assert_eq!(join.join().unwrap().len(), 1);
}

#[test]
fn lost_session_is_never_replaced_and_explicit_close_is_idempotent() {
    let (transport, join) = server(vec![
        opened(),
        reply(
            404,
            json!({"failure_kind":"unknown_session", "error":"sensitive SQL"}),
        ),
        reply(404, json!({"failure_kind":"unknown_session"})),
    ]);
    let mut session = RemoteSession::open(transport).unwrap();
    let error = session.query("select 1", &[]).unwrap_err();
    assert_eq!(error.kind, Failure::SessionLost);
    assert!(error.request_id.is_some());
    assert!(!format!("{error:?}").contains("sensitive"));
    assert_eq!(
        session.query("select 2", &[]).unwrap_err().kind,
        Failure::SessionLost
    );
    session.close(STATEMENT_TIMEOUT).unwrap();
    session.close(STATEMENT_TIMEOUT).unwrap();
    assert_eq!(
        session.query("select 3", &[]).unwrap_err().kind,
        Failure::Closed
    );
    let requests = join.join().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /v1/sessions "));
    assert!(
        requests[1].starts_with("POST /v1/sessions/00000000-0000-0000-0000-000000000001/queries ")
    );
    assert!(requests[2].starts_with("DELETE /v1/sessions/00000000-0000-0000-0000-000000000001 "));
}

#[test]
fn cleanup_failure_is_reported_and_can_be_retried_without_reopening() {
    let (transport, join) = server(vec![opened(), reply(503, json!({})), reply(200, json!({}))]);
    let mut session = RemoteSession::open(transport).unwrap();
    assert!(session.close(STATEMENT_TIMEOUT).is_err());
    assert_eq!(
        session.query("select 1", &[]).unwrap_err().kind,
        Failure::Closed
    );
    session.close(STATEMENT_TIMEOUT).unwrap();
    let requests = join.join().unwrap();
    assert_eq!(requests[1].lines().next(), requests[2].lines().next());
}

#[test]
fn drop_attempts_cleanup_and_invalid_parameters_do_not_reach_server() {
    let (transport, join) = server(vec![opened(), reply(200, json!({}))]);
    {
        let session = RemoteSession::open(transport).unwrap();
        for parameter in [
            Parameter::Double(f64::NAN),
            Parameter::Double(f64::INFINITY),
            Parameter::ListVarchar(vec!["a".to_owned()]),
        ] {
            assert_eq!(
                session.query("select ?", &[parameter]).unwrap_err().kind,
                Failure::Parameter
            );
        }
    }
    let requests = join.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("DELETE "));
}

#[test]
fn cancellation_uses_the_owned_session_and_stops_after_close() {
    let (transport, join) = server(vec![opened(), reply(200, json!({})), reply(200, json!({}))]);
    let mut session = Session::Remote(RemoteSession::open(transport).unwrap());
    let control = session.cancellation().unwrap();
    control.cancel().unwrap();
    session.close().unwrap();
    assert_eq!(control.cancel().unwrap_err().kind, Failure::Closed);
    let requests = join.join().unwrap();
    assert!(
        requests[1].starts_with("POST /v1/sessions/00000000-0000-0000-0000-000000000001/cancel ")
    );
}

#[test]
fn embedded_bindings_and_transaction_affinity_survive_calls() {
    use arrow::array::{Array, Float64Array, Int64Array, StringArray};
    let mut session = Session::open_embedded().unwrap();
    session
        .query("CREATE TEMP TABLE fixture (n BIGINT)", &[])
        .unwrap();
    session.query("BEGIN", &[]).unwrap();
    session
        .query(
            "INSERT INTO fixture VALUES (?)",
            &[Parameter::Bigint(i64::MAX)],
        )
        .unwrap();
    let batches = session
        .query(
            "SELECT n, ?::DOUBLE AS score, ?::VARCHAR AS missing FROM fixture",
            &[Parameter::Double(0.0012033694344163659), Parameter::Null],
        )
        .unwrap();
    assert_eq!(
        batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0),
        i64::MAX
    );
    assert_eq!(
        batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0)
            .to_bits(),
        0.0012033694344163659_f64.to_bits()
    );
    assert!(
        batches[0]
            .column(2)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .is_null(0)
    );
    session.query("ROLLBACK", &[]).unwrap();
    let batches = session.query("SELECT n FROM fixture", &[]).unwrap();
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 0);
    session.close().unwrap();
    assert_eq!(
        session.query("SELECT 1", &[]).unwrap_err().kind,
        Failure::Closed
    );
}

#[test]
fn transport_loss_has_a_request_identity_and_never_replays() {
    let (transport, join) = server(vec![
        opened(),
        Reply {
            status: 0,
            headers: "",
            body: Vec::new(),
        },
        reply(200, json!({})),
    ]);
    let mut session = RemoteSession::open(transport).unwrap();
    let error = session.query("SELECT 1", &[]).unwrap_err();
    assert_eq!(error.kind, Failure::UnknownOutcome);
    assert!(error.request_id.is_some());
    assert_eq!(
        session.query("SELECT 2", &[]).unwrap_err().kind,
        Failure::SessionLost
    );
    session.close(STATEMENT_TIMEOUT).unwrap();
    assert_eq!(join.join().unwrap().len(), 3);
}

#[test]
fn oversized_control_reply_is_refused() {
    let (transport, join) = server(vec![Reply {
        status: 200,
        headers: "",
        body: vec![b' '; usize::try_from(MAX_CONTROL_BYTES + 1).unwrap()],
    }]);
    assert_eq!(
        RemoteSession::open(transport).err().unwrap().kind,
        Failure::Limit
    );
    assert_eq!(join.join().unwrap().len(), 1);
}

#[test]
fn malformed_arrow_poisons_the_session_without_exposing_the_body() {
    let (transport, join) = server(vec![
        opened(),
        reply(200, json!({"secret":"upstream-value"})),
        reply(200, json!({})),
    ]);
    let mut session = RemoteSession::open(transport).unwrap();
    let error = session.query("SELECT 1", &[]).unwrap_err();
    assert_eq!(error.kind, Failure::Protocol);
    assert!(!format!("{error:?}").contains("upstream-value"));
    assert_eq!(
        session.query("SELECT 2", &[]).unwrap_err().kind,
        Failure::SessionLost
    );
    session.close(STATEMENT_TIMEOUT).unwrap();
    assert_eq!(join.join().unwrap().len(), 3);
}

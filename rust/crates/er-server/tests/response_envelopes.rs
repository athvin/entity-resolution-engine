//! The response-envelope ledger.
//!
//! `mock_contract.rs` compares route and method; `wire-contract.test.ts` and
//! `mock-fabrication.test.ts` compare *field names*. None of them can see an
//! **envelope** mismatch — a list read as a bare array when the server wraps it
//! in `{key: […]}`. That is W0.5b: the BFF declared
//! `forward<unknown[]>("/v1/orgs/{org}/jobs")`, er-server sends `{jobs: […]}`,
//! and the dashboard crashed against the real backend while every mock-backed
//! tier was green, because the mock answered with the shape the frontend
//! believed in.
//!
//! The reason that was an easy mistake to make is worth stating, because it is a
//! property of this API rather than of that developer: **the two conventions are
//! genuinely mixed.** `/audit`, `/runs` and `/webhooks` return bare arrays;
//! `/jobs`, `/reviews` and `/webhooks/deliveries` wrap. Within one resource
//! family, both spellings occur, so there is no convention to infer and guessing
//! is a coin flip. Unifying them is a breaking change for every tenant reading
//! the API directly, so this test does not ask for that — it writes the fact
//! down, per route, where a consumer can check it.
//!
//! The ledger is **derived from the handlers**, never declared beside them. That
//! distinction is the point: a `#[utoipa::path]` response body, or any
//! hand-maintained table, is one more claim that can drift from the code — the
//! same shape as the defect it would be built to prevent. Here the source is the
//! authority, so the ledger cannot disagree with what the server returns without
//! the classifier failing.
//!
//! An envelope this test **cannot** classify is a hard failure naming the
//! handler, not a skip. A gate that silently drops what it does not understand
//! reports "all clear" over exactly the novel case most likely to be wrong.

use er_api_spec::{envelope_ledger, get_routes};

/// This server's router and handlers, as source.
const SERVER_SRC: &str = include_str!("../src/lib.rs");

/// The committed ledger the frontend reads.
const COMMITTED: &str = include_str!("../../../../frontend/src/lib/api/envelopes.json");

#[test]
fn every_routed_get_has_a_classified_envelope() {
    let routes = get_routes(SERVER_SRC);
    // A parser that matched nothing would make the ledger trivially correct.
    assert!(
        routes.len() > 30,
        "parsed only {} GET routes from the router; the parser is broken",
        routes.len()
    );
    match envelope_ledger(SERVER_SRC) {
        Ok(_) => {}
        Err(failures) => panic!(
            "{} routed GET handler(s) could not be classified:\n{}",
            failures.len(),
            failures.join("\n")
        ),
    }
}

#[test]
fn the_committed_ledger_is_current() {
    let built = envelope_ledger(SERVER_SRC).unwrap_or_else(|f| panic!("{}", f.join("\n")));
    assert_eq!(
        built, COMMITTED,
        "frontend/src/lib/api/envelopes.json is stale. It is generated from the handlers, so a \
         diff here means a response envelope changed — check every BFF route and mock fixture \
         that reads the affected path before regenerating with \
         `cargo test -p er-server --test response_envelopes -- --ignored regenerate`"
    );
}

#[test]
fn both_conventions_really_are_in_use() {
    // The mixed convention is the premise of this test's existence. If the API
    // is ever unified, this fails and the doc comment above needs rewriting
    // rather than quietly becoming false.
    let built = envelope_ledger(SERVER_SRC).unwrap_or_else(|f| panic!("{}", f.join("\n")));
    assert!(
        built.contains("\"envelope\": \"bare\""),
        "no un-enveloped GET remains"
    );
    assert!(
        built.contains("\"envelope\": \"wrapped\""),
        "no wrapped GET remains"
    );
}

/// Rewrite the committed ledger. Run with
/// `cargo test -p er-server --test response_envelopes -- --ignored regenerate`.
#[test]
#[ignore = "writes frontend/src/lib/api/envelopes.json"]
fn regenerate() {
    let built = envelope_ledger(SERVER_SRC).unwrap_or_else(|f| panic!("{}", f.join("\n")));
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../frontend/src/lib/api/envelopes.json"
    );
    std::fs::write(path, built).expect("write the ledger");
}

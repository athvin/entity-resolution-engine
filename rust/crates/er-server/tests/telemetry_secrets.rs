//! #869: no telemetry call in er-server's product source names a secret.
//!
//! The key-minting path is the place a credential passes through handler code
//! with a span open: `create_key` holds the plaintext in a local named `key`
//! between `pg::issue_key` and the response body. `pg_key_telemetry.rs` proves
//! the exported spans of a real mint carry no plaintext and no hash, but that
//! is a runtime proof over the fields the request span DECLARES — and
//! `span.record("key", plaintext)` on an undeclared field is a silent no-op
//! (the observability skill's failure 1), so it would pass that test today and
//! leak the day someone declared the field. This is the shape gate for it.
//!
//! Every tracing macro and every `.record(` call is cut out by paren matching
//! and tokenized into identifier words; a word from [`SECRET_WORDS`] is an
//! offender. Whole words, so `key_id`, `hash_secret` and `token_hash` do not
//! match `key`, `secret` or `hash` — those are what spans are supposed to
//! carry, or functions rather than values.

/// Every product `.rs` file under er-server's `src/`, test modules stripped.
/// Walked rather than listed, so a new module is covered the day it lands.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_stem().is_some_and(|s| s != "tests")
            {
                let src = std::fs::read_to_string(&path).expect("readable source");
                out.push((
                    path.display().to_string(),
                    crate::wire_serialization::product_source(&src),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out
}

/// Names that, as a whole word in a telemetry call's code (string literals and
/// comments stripped), mean a credential is being recorded — anywhere in the
/// crate.
const SECRET_WORDS: &[&str] = &["plaintext", "key_hash", "secret", "token", "password"];

/// Stricter words for the functions that hold a key's plaintext in a local.
/// `key` is a record key or an S3 object key everywhere else in this crate, so
/// it is only a credential here: `create_key` binds the minted plaintext to
/// `key`, `issue_key` builds it, and the extractor splits the presented one.
const KEY_SURFACE: &[&str] = &[
    "fn create_key(",
    "fn revoke_key(",
    "fn list_keys(",
    "fn issue_key(",
    "fn from_request_parts(",
];
const KEY_SURFACE_WORDS: &[&str] = &["key", "hash", "full_key", "parts3"];

/// The call shapes that put a value into telemetry.
const CALLS: &[&str] = &[
    "trace!(", "debug!(", "info!(", "warn!(", "error!(", "_span!(", ".record(",
];

/// The argument text of every telemetry call in `src`.
fn calls(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for shape in CALLS {
        for (at, _) in src.match_indices(shape) {
            let open = at + shape.len() - 1;
            let mut depth = 0usize;
            for (i, c) in src[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            out.push(src[open + 1..open + i].to_string());
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// `args` with string literals and `//` comments removed: a log message saying
/// "api key issued" records no key.
fn code_only(args: &str) -> String {
    let mut out = String::with_capacity(args.len());
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                out.push(' ');
            }
            '/' if chars.peek() == Some(&'/') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
                out.push('\n');
            }
            _ => out.push(c),
        }
    }
    out
}

/// The words from `banned` a call's code names.
fn secrets_in(args: &str, banned: &[&'static str]) -> Vec<&'static str> {
    let code = code_only(args);
    let words: Vec<&str> = code
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    banned
        .iter()
        .copied()
        .filter(|s| words.contains(s))
        .collect()
}

/// Every offending call in `src` against `banned`, as `<words>: <call>`.
fn offenders_against(src: &str, banned: &[&'static str]) -> Vec<String> {
    calls(src)
        .into_iter()
        .filter_map(|args| {
            let found = secrets_in(&args, banned);
            (!found.is_empty()).then(|| format!("{found:?}: ({})", args.trim()))
        })
        .collect()
}

/// Crate-wide offenders.
fn offenders(src: &str) -> Vec<String> {
    offenders_against(src, SECRET_WORDS)
}

/// The body of `fn name(` in `src`, by brace matching from its first `{`.
fn body_of<'a>(src: &'a str, signature: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    for (at, _) in src.match_indices(signature) {
        let Some(open) = src[at..].find('{').map(|o| at + o) else {
            continue;
        };
        let mut depth = 0usize;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push(&src[open..=open + i]);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Offenders inside the key surface: the crate-wide words plus the stricter
/// ones.
fn key_surface_offenders(body: &str) -> Vec<String> {
    let banned: Vec<&'static str> = SECRET_WORDS
        .iter()
        .chain(KEY_SURFACE_WORDS)
        .copied()
        .collect();
    offenders_against(body, &banned)
}

#[test]
fn the_detector_catches_a_recorded_secret_and_passes_an_identifier() {
    // The ticket's own fixture: a plaintext recorded onto the span.
    assert_eq!(
        offenders(r#"span.record("key", plaintext);"#).len(),
        1,
        "the #869 fixture is not caught"
    );
    assert_eq!(
        offenders("let s = tracing::info_span!(\"hook\", er.url = %hook.secret);").len(),
        1
    );
    assert_eq!(offenders("tracing::debug!(h = %key_hash);").len(), 1);
    // On the key surface, the plaintext's own binding is caught too.
    assert_eq!(
        key_surface_offenders(r#"{ tracing::info!(k = %key, "minted"); }"#).len(),
        1
    );
    assert_eq!(
        key_surface_offenders("{ span.record(er_span::attr::KEY_ID, full_key.as_str()); }").len(),
        1
    );
    // What the handlers do record: ids and roles. The message text and a
    // comment are not code.
    let issued = r#"tracing::info!(org = %org, key_id = %key_id, role = role.as_str(), "api key issued"); // the key stays in the body"#;
    assert_eq!(offenders(issued), Vec::<String>::new());
    assert_eq!(key_surface_offenders(issued), Vec::<String>::new());
    assert_eq!(
        key_surface_offenders("span.record(er_span::attr::KEY_ID, key_id.as_str());"),
        Vec::<String>::new()
    );
    // Elsewhere `key` is a record or object key, which spans may carry.
    assert_eq!(
        offenders("tracing::warn!(er.record_key = %key, \"lookup failed\");"),
        Vec::<String>::new()
    );
}

#[test]
fn no_telemetry_call_names_a_secret() {
    let mut seen = 0;
    let mut found = Vec::new();
    let files = sources();
    assert!(files.len() > 30, "only {} source files found", files.len());
    for (path, src) in &files {
        seen += calls(src).len();
        for offender in offenders(src) {
            found.push(format!("{path}: {offender}"));
        }
    }
    // The key surface, with the stricter words.
    let lib = crate::wire_serialization::product_source(include_str!("../src/lib.rs"));
    let pg = crate::wire_serialization::product_source(include_str!("../src/pg.rs"));
    let mut surface = 0;
    for signature in KEY_SURFACE {
        for body in body_of(&lib, signature)
            .into_iter()
            .chain(body_of(&pg, signature))
        {
            surface += 1;
            for offender in key_surface_offenders(body) {
                found.push(format!("{signature}…: {offender}"));
            }
        }
    }
    assert!(
        surface >= KEY_SURFACE.len(),
        "found {surface} key-surface bodies for {} signatures — a function was renamed and the stricter scan no longer reads it",
        KEY_SURFACE.len()
    );
    // Anti-vacuity: the handlers and the request span between them make many
    // telemetry calls, so a scan that found a handful is not reading them.
    assert!(
        seen > 20,
        "only {seen} telemetry calls found; the scan is broken"
    );
    assert!(
        found.is_empty(),
        "a telemetry call names a credential. A span is exportable, and \
         `er.api_key` / `er.api_key_hash` are Pii::Never in er-span: record \
         `er.key_id` instead, and leave the plaintext in the mint response \
         body.\n{}",
        found.join("\n")
    );
}

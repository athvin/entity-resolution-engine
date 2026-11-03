//! The PII boundary on spans, enforced rather than documented.
//!
//! This is a deduplication engine, so names, emails, phones and addresses ARE
//! the payload. §10.4's rule is that telemetry carries identifiers and counts
//! and never attribute values — and spans now leave the process over OTLP, so
//! the boundary is an exportable store rather than in-account Postgres.
//!
//! The real guarantee is structural and lives in `er-span`: `sql_span` takes no
//! parameter argument, so there is nowhere to put a bind value. These are the
//! shape gates around that, and they exist because the structural property is
//! only a property while the call stays where it is:
//!
//! 1. only GENERATED code may call `sql_span` — product code calls
//!    `generator_span`, which has no `sql` parameter at all;
//! 2. every emitted `sql_span` argument is a literal, so none of them can carry
//!    a runtime value;
//! 3. the names `er-span` declares `Pii::Never` appear nowhere as recorded
//!    fields.
//!
//! The third reads the forbidden list out of `er-span/src/attr.rs` rather than
//! restating it, because a hardcoded copy is how a registry and its enforcement
//! drift apart — `er-entity-lint`'s own header records three documented codes
//! that were never implemented for exactly that reason.

use std::fs;
use std::path::{Path, PathBuf};

/// The `rust/crates` directory.
fn crates_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

/// Every `.rs` file under the workspace's crates, excluding build output and
/// test trees. Mirrors `freeform_sql.rs`'s walker for the same reason: a gate
/// that scans a hand-listed subtree silently stops covering what moves.
fn product_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable workspace dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name == "target" || name == "tests" || name == "benches" || name.starts_with('.') {
                continue;
            }
            product_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn all_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    product_sources(&crates_dir(), &mut files);
    assert!(
        files.len() > 60,
        "the workspace scan looks wrong ({} files) — a moved crate tree must fail here \
         rather than pass having examined nothing",
        files.len()
    );
    files
}

/// `er-span` itself, which defines `sql_span`, and the emitter that WRITES the
/// generated calls as text. Each is here with the reason it is not a violation.
fn is_allowed_to_mention_sql_span(path: &Path) -> Option<&'static str> {
    let p = path.to_string_lossy().replace('\\', "/");
    if p.contains("/er-span/src/") {
        return Some("defines sql_span");
    }
    if p.contains("/er-sql-check/src/emit.rs") {
        return Some("emits the generated calls as string literals; it does not call sql_span");
    }
    None
}

/// A generated file says so on its first line.
fn is_generated(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| t.lines().take(3).any(|l| l.contains("@generated")))
}

#[test]
fn only_generated_code_calls_sql_span() {
    let mut offenders = Vec::new();
    let mut generated_callers = 0usize;

    for file in all_sources() {
        let text = fs::read_to_string(&file).expect("readable source");
        if !text.contains("sql_span(") {
            continue;
        }
        if is_allowed_to_mention_sql_span(&file).is_some() {
            continue;
        }
        if is_generated(&file) {
            generated_callers += 1;
            continue;
        }
        offenders.push(file.to_string_lossy().into_owned());
    }

    assert!(
        offenders.is_empty(),
        "sql_span is for GENERATED code only — product code must call generator_span, \
         which has no `sql` parameter and therefore cannot carry a statement's text:\n{}",
        offenders.join("\n")
    );
    // A rename or a regenerate that emptied er-entities would otherwise leave
    // this test passing while enforcing nothing.
    assert!(
        generated_callers > 20,
        "expected many generated callers, found {generated_callers} — the generated \
         crate must have moved, and this gate is no longer looking at it"
    );
}

#[test]
fn every_emitted_sql_span_argument_is_a_literal() {
    // The shape half of the rule. The REAL guarantee is that `sql_span` has no
    // parameter argument at all, which is stated here because a reader could
    // otherwise believe this scan is what keeps bind values out.
    let generated: Vec<PathBuf> = all_sources()
        .into_iter()
        .filter(|p| is_generated(p))
        .collect();
    assert!(
        !generated.is_empty(),
        "no generated sources found; the gate is examining nothing"
    );

    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for file in &generated {
        let text = fs::read_to_string(file).expect("readable source");
        for (index, _) in text.match_indices("sql_span(") {
            let tail = &text[index + "sql_span(".len()..];
            // Arguments run to the matching close paren; the emitted form never
            // nests a call inside them, which is itself part of the shape.
            let Some(end) = tail.find(')') else {
                offenders.push(format!("{}: unterminated sql_span(", file.display()));
                continue;
            };
            checked += 1;
            for arg in tail[..end].split(',') {
                let arg = arg.trim();
                if arg.is_empty() {
                    continue;
                }
                let literal = arg.starts_with('"') && arg.ends_with('"');
                // `SQL` is the const holding the statement template, and the
                // `Store` discriminants are compile-time values.
                let known_const = arg == "SQL" || arg.starts_with("::er_span::Store::");
                if !literal && !known_const {
                    offenders.push(format!("{}: non-literal argument `{arg}`", file.display()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "every sql_span argument must be a string literal, SQL, or a Store discriminant — \
         a runtime value here would put caller data on a span:\n{}",
        offenders.join("\n")
    );
    assert!(
        checked > 200,
        "expected the full generated statement surface, checked only {checked}"
    );
}

/// Source with comments removed.
///
/// Needed because the first version of this gate failed on
/// `er-server/src/telemetry.rs`, whose comment says `url.query` is deliberately
/// NOT recorded — a file explaining the rule was being reported as breaking it.
/// Writing down WHY an attribute is forbidden is exactly the behaviour to
/// encourage, so prose is exempt and only code counts.
///
/// Deliberately crude, and the limitation runs the UNSAFE way, so it is stated
/// rather than glossed: this does not understand string literals, so a `//`
/// inside one (a URL, say) strips the rest of that line and a forbidden name
/// after it on the same line would be MISSED. A false negative, not a false
/// positive.
///
/// Accepted because the alternative is a Rust lexer in a test, and because the
/// structural guarantee — `sql_span` has no parameter argument — does not
/// depend on this scan at all. If that ever stops being true, this wants to
/// become a real lint rather than a slightly better regex.
fn code_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // Block comments first, so a `//` inside one cannot confuse the line pass.
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("*/") {
            rest = &rest[start + end + 2..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);

    out.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn forbidden_attribute_names_are_never_recorded() {
    // Read the list out of the registry rather than restating it: a hardcoded
    // copy is how a registry and its enforcement drift apart.
    let attr_rs = crates_dir().join("er-span/src/attr.rs");
    let registry = fs::read_to_string(&attr_rs).expect("er-span/src/attr.rs is readable");

    let forbidden: Vec<String> = registry
        .split("Attr {")
        .filter(|entry| entry.contains("Pii::Never"))
        .filter_map(|entry| {
            let start = entry.find("name: \"")? + "name: \"".len();
            let rest = &entry[start..];
            Some(rest[..rest.find('"')?].to_string())
        })
        .collect();

    assert!(
        forbidden.len() >= 3,
        "expected at least the three declared Pii::Never names, parsed {forbidden:?} \
         from {} — the registry's shape must have changed, and this gate is no longer \
         reading it",
        attr_rs.display()
    );

    let mut offenders = Vec::new();
    for file in all_sources() {
        // The registry declares these names; it is the one place they belong.
        if file.to_string_lossy().contains("/er-span/src/attr.rs") {
            continue;
        }
        let text = fs::read_to_string(&file).expect("readable source");
        for name in &forbidden {
            if code_only(&text).contains(name) {
                offenders.push(format!("{}: records {name}", file.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these attribute names are declared Pii::Never and must appear nowhere else — \
         `er.record_value` is a record's own payload, `url.query` is the \
         contains-search over record values, and a plaintext email belongs in \
         audit_log rather than on an exportable span:\n{}",
        offenders.join("\n")
    );
}

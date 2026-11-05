//! The collector's attribute allowlist still covers what the code emits.
//!
//! ## The failure this prevents
//!
//! `otel-collector-values.yaml` carries a `transform/pii_allowlist` processor
//! whose `keep_keys` drops every span attribute not named in it. That is
//! deliberately fail-closed: for a deduplication engine whose payload is names
//! and addresses, losing a field is the right failure and leaking one is not.
//!
//! The cost of that choice is a second failure mode, and it is silent in the
//! other direction. Add a span attribute in Rust, forget the YAML, and it is
//! stripped in flight — the code is right, the deploy is green, the span store
//! simply never shows it, and the only symptom is a field nobody can find. This
//! test is what turns that into a build failure.
//!
//! ## What it cannot cover
//!
//! Attributes the SDK adds on its own — `busy_ns`, `idle_ns`, `thread.*`,
//! `code.*`, `target` — appear in no span macro, so nothing here can derive
//! them. They are in the allowlist because they were found by reading a **real
//! exported Parquet file** from the dev collector, and a `tracing-opentelemetry`
//! bump that renames one will drop it again with no test failing.
//!
//! That is recorded in the values file as the thing to re-measure after a
//! dependency bump, and it is the honest limit of this gate rather than a gap
//! to paper over.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."))
}

fn values_yaml() -> String {
    let path = repo_root().join("infra/k8s/observability/otel-collector-values.yaml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The `keep_keys` list, read out of the YAML as plain text.
///
/// Textual rather than parsed: this crate has no YAML dependency and adding one
/// to read a list of quoted strings would be the larger change. The shape is
/// asserted — a parse that silently found nothing would otherwise pass.
fn allowlisted() -> Vec<String> {
    let yaml = values_yaml();
    let start = yaml
        .find("keep_keys(span.attributes, [")
        .expect("the values file declares keep_keys(span.attributes, [...]) — has it been renamed?");
    let rest = &yaml[start..];
    let end = rest.find("])").expect("the keep_keys list is unterminated");
    let names: Vec<String> = rest[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(
        names.len() > 25,
        "parsed only {} names from keep_keys, which cannot be right — the list's \
         shape must have changed and this gate is no longer reading it",
        names.len()
    );
    names
}

/// Resolve `er_span::attr`'s `pub const NAME: &str = "…"` pairs.
fn attr_constants() -> Vec<(String, String)> {
    let path = repo_root().join("rust/crates/er-span/src/attr.rs");
    let source = fs::read_to_string(&path).expect("er-span/src/attr.rs is readable");
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pub const ")
            && let Some((name, value)) = rest.split_once(": &str = ")
        {
            out.push((
                name.to_string(),
                value.trim_matches(|c| c == '"' || c == ';').to_string(),
            ));
        }
    }
    assert!(out.len() > 8, "parsed {} attr constants", out.len());
    out
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every field name declared in a span macro across the workspace.
///
/// Both spellings: a bare `er.foo = …` key, and `{ attr::FOO } = …`, which is
/// how `sql_span` names the constants. Missing the second form would make this
/// gate blind to exactly the attributes that matter most.
fn span_attributes() -> Vec<String> {
    let consts = attr_constants();
    let mut files = Vec::new();
    rust_sources(&repo_root().join("rust/crates"), &mut files);

    let mut found = std::collections::BTreeSet::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("readable source");
        for (index, _) in source.match_indices("_span!(") {
            // Take the macro's argument list by brace matching, so a nested
            // call does not truncate it.
            let open = index + "_span!".len();
            let mut depth = 0usize;
            let mut close = open;
            for (offset, ch) in source[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let block = &source[open..close];
            for line in block.lines() {
                let line = line.trim();
                // `{ attr::FOO } = …`
                if let Some(rest) = line.strip_prefix("{ attr::")
                    && let Some((name, _)) = rest.split_once(" }")
                    && let Some((_, value)) = consts.iter().find(|(c, _)| c == name)
                {
                    found.insert(value.clone());
                    continue;
                }
                // `er.foo = …`, excluding `tracing::field::Empty` RHS noise.
                if let Some((key, _)) = line.split_once(" = ") {
                    let key = key.trim();
                    if !key.is_empty()
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                        && key.contains('.')
                    {
                        found.insert(key.to_string());
                    }
                }
            }
        }
    }
    assert!(
        found.len() > 20,
        "found only {} span attributes in the workspace, which cannot be right",
        found.len()
    );
    found.into_iter().collect()
}

#[test]
fn every_emitted_span_attribute_survives_the_collector() {
    let allowed = allowlisted();
    // `otel.*` are consumed by `tracing-opentelemetry` to set the exported
    // span's name, kind and status — they become fields of the span, not
    // attributes on it, which a real exported Parquet file confirms. Allowing
    // them would be harmless and listing them would be misleading.
    const CONSUMED_BY_THE_BRIDGE: &[&str] = &["otel.name", "otel.kind", "otel.status_code"];

    let missing: Vec<String> = span_attributes()
        .into_iter()
        .filter(|a| !CONSUMED_BY_THE_BRIDGE.contains(&a.as_str()))
        .filter(|a| !allowed.contains(a))
        .collect();

    assert!(
        missing.is_empty(),
        "these span attributes are emitted by Rust but NOT in the collector's \
         keep_keys allowlist, so they are stripped in flight — the code is right, \
         the deploy is green, and the field simply never appears in the span \
         store:\n\n{}\n\nAdd them to `transform/pii_allowlist` in \
         infra/k8s/observability/otel-collector-values.yaml.",
        missing
            .iter()
            .map(|a| format!("  {a}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_ottl_statement_carries_no_comment() {
    // This test exists because its absence cost a crashlooping collector.
    //
    // The `keep_keys(...)` list is an OTTL expression inside a YAML `>-` block,
    // and **OTTL has no comment syntax**. A `#` line added for readability folds
    // into the expression, and the collector then refuses the ENTIRE config at
    // startup with `lexer: invalid input text "#..."`.
    //
    // The failure mode is the nasty part: the ConfigMap updates, the Deployment
    // rolls, the new pod crashloops, and the OLD pod keeps serving. So the
    // config looks deployed, telemetry keeps flowing, and nothing works — the
    // change simply never takes effect. Verified by reading pod health, which
    // is the only thing that showed it.
    let yaml = values_yaml();
    let start = yaml
        .find("keep_keys(span.attributes, [")
        .expect("the values file declares keep_keys(span.attributes, [...])");
    let rest = &yaml[start..];
    let end = rest.find("])").expect("the keep_keys list is unterminated");
    let statement = &rest[..end];

    assert!(
        !statement.contains('#'),
        "the OTTL statement contains a `#`, which OTTL does not accept as a \
         comment — the collector will refuse the whole config at startup and \
         crashloop while the old pod keeps serving:\n\n{statement}\n\n\
         Put commentary in a YAML comment above `transform/pii_allowlist` instead."
    );
}

#[test]
fn no_forbidden_attribute_is_allowlisted() {
    // The allowlist is the last thing between a `Pii::Never` name and an
    // exportable store, so it must not be the thing that admits one.
    let path = repo_root().join("rust/crates/er-span/src/attr.rs");
    let source = fs::read_to_string(&path).expect("readable");
    let consts = attr_constants();

    let forbidden: Vec<String> = source
        .split("Attr {")
        .filter(|entry| entry.contains("Pii::Never"))
        .filter_map(|entry| {
            let start = entry.find("name: ")? + "name: ".len();
            let rest = entry[start..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                Some(stripped[..stripped.find('"')?].to_string())
            } else {
                let ident: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                    .collect();
                consts
                    .iter()
                    .find(|(c, _)| *c == ident)
                    .map(|(_, v)| v.clone())
            }
        })
        .collect();

    assert!(
        forbidden.len() >= 3,
        "parsed {forbidden:?} as the Pii::Never set — expected at least three"
    );

    let allowed = allowlisted();
    let admitted: Vec<&String> = forbidden.iter().filter(|f| allowed.contains(f)).collect();
    assert!(
        admitted.is_empty(),
        "the collector allowlist admits attribute names declared `Pii::Never`: \
         {admitted:?}. `er.record_value` is a record's own payload, `url.query` is \
         the contains-search over record values, and a plaintext email belongs in \
         audit_log rather than on an exportable span."
    );
}

#[test]
fn the_allowlist_runs_before_sampling() {
    // Order matters twice. A tail-sampling policy reads span attributes — the
    // `ui-interactions` policy keys on `er.action` — so stripping after it
    // would be stripping too late to matter. And both must precede `batch`,
    // which hands the sampler trace fragments if it runs first.
    let yaml = values_yaml();
    let line = yaml
        .lines()
        .find(|l| l.contains("processors: [memory_limiter, transform/pii_allowlist"))
        .expect(
            "the traces pipeline must list transform/pii_allowlist — if the pipeline \
             was reordered, check it still precedes tail_sampling",
        );
    let allowlist = line.find("transform/pii_allowlist").expect("present");
    let sampling = line.find("tail_sampling").expect("tail_sampling present");
    let batch = line.find("batch").expect("batch present");
    assert!(
        allowlist < sampling,
        "the allowlist must precede tail_sampling, which reads attributes: {line}"
    );
    assert!(
        sampling < batch,
        "tail_sampling must precede batch, or the sampler sees trace fragments: {line}"
    );
}

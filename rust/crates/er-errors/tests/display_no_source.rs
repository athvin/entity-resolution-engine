//! The posterity gate for error `Display`: no `#[error("…")]` message may
//! interpolate its own source.
//!
//! Error reporters walk `.source()` separately — `anyhow`'s `{:#}` and
//! `er_server::errfmt::chain` both do — so a message that also interpolates its
//! cause prints that cause twice:
//!
//! ```text
//! parsing "x.yaml": bad type at line 3: bad type at line 3
//! ```
//!
//! The rule is therefore: name what was attempted, and let the chain supply
//! why it failed.
//!
//! ```ignore
//! #[error("reading {path:?}: {source}")]   // wrong — double-prints
//! #[error("reading {path:?}")]             // right
//! ```
//!
//! The subtle half is that thiserror treats a field *named* `source` as the
//! source whether or not it carries `#[source]`, so `{source}` in the message
//! is a violation even with no attribute in sight. This scan covers all three
//! spellings: `#[from]`, `#[source]`, and the bare `source:` field.
//!
//! The counterpart to this gate is that `e.to_string()` alone now yields only
//! the outermost message. Anywhere an error is flattened into a `String`
//! instead of propagated, the chain has to be walked at that point — see
//! `er_server::errfmt`.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, skipping `target/`, dotted dirs, `tests/` and
/// `benches/` (a test fixture's error type is scratch by definition).
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

/// The field names a format string interpolates: `{path:?}` → `path`, `{0}` →
/// `0`. `{{` and `}}` are escapes and contribute nothing.
fn placeholders(fmt: &str) -> Vec<String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'{') {
            i += 2; // escaped brace, not a placeholder
            continue;
        }
        let mut name = String::new();
        let mut j = i + 1;
        while j < chars.len() && chars[j] != '}' && chars[j] != ':' {
            name.push(chars[j]);
            j += 1;
        }
        while j < chars.len() && chars[j] != '}' {
            j += 1;
        }
        let name = name.trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
        i = j + 1;
    }
    out
}

/// True when `body` declares a field literally named `source` (thiserror's
/// implicit source). Guards against `foo::source` and a `.source()` call.
fn has_bare_source_field(body: &str) -> bool {
    let mut offset = 0;
    while let Some(i) = body[offset..].find("source") {
        let at = offset + i;
        let prev = body[..at].chars().next_back();
        let preceded_by_ident =
            prev.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.');
        let after = &body[at + "source".len()..];
        let trimmed = after.trim_start();
        // A field declaration is `source:` — not `source::path`, not `source()`.
        if !preceded_by_ident
            && !after.starts_with('_')
            && trimmed.starts_with(':')
            && !trimmed.starts_with("::")
        {
            return true;
        }
        offset = at + "source".len();
    }
    false
}

/// Drop a leading `pub`, `pub(crate)`, `pub(super)` … from a field
/// declaration, leaving the identifier. `public_id` is an identifier, not a
/// visibility keyword, so the `pub` prefix only counts when a space or `(`
/// follows it.
fn strip_visibility(decl: &str) -> &str {
    let decl = decl.trim_start();
    let Some(after_pub) = decl.strip_prefix("pub") else {
        return decl;
    };
    if let Some(restricted) = after_pub.strip_prefix('(') {
        return match restricted.find(')') {
            Some(close) => restricted[close + 1..].trim_start(),
            None => decl,
        };
    }
    if after_pub.starts_with(char::is_whitespace) {
        return after_pub.trim_start();
    }
    decl
}

/// The field thiserror treats as this variant's source, if any: `"0"` for a
/// marked tuple field, otherwise the named field's identifier.
fn source_field(body: &str) -> Option<String> {
    // Tuple field: `Variant(#[from] io::Error)`, addressable as `{0}`.
    if let Some(i) = body.find('(') {
        let after = body[i + 1..].trim_start();
        if after.starts_with("#[from]") || after.starts_with("#[source]") {
            return Some("0".to_string());
        }
    }
    // Named field carrying the marker: `#[source] pub inner: io::Error`.
    for marker in ["#[source]", "#[from]"] {
        if let Some(i) = body.find(marker) {
            let after = strip_visibility(&body[i + marker.len()..]);
            let ident: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() && after[ident.len()..].trim_start().starts_with(':') {
                return Some(ident);
            }
        }
    }
    // thiserror's implicit case: a field named `source` needs no attribute.
    if has_bare_source_field(body) {
        return Some("source".to_string());
    }
    None
}

#[test]
fn no_error_display_interpolates_its_own_source() {
    let crates = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    product_sources(&crates, &mut files);
    assert!(
        files.len() > 60,
        "the workspace scan looks wrong ({} files found under {})",
        files.len(),
        crates.display()
    );

    const ATTR: &str = "#[error(\"";
    let mut offenders: Vec<String> = Vec::new();
    let mut attrs_seen = 0usize;

    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };

        // Every `#[error("…")]` in the file, with the declaration that follows
        // it — i.e. up to the next such attribute, which is where this
        // variant's fields end.
        let starts: Vec<usize> = text.match_indices(ATTR).map(|(i, _)| i).collect();
        for (n, &start) in starts.iter().enumerate() {
            let open = start + ATTR.len();
            // The format string, honouring `\"` escapes. `fmt_end` is the byte
            // just past its closing quote — where the variant's own
            // declaration starts, so the field scan never reads the message.
            let mut fmt = String::new();
            let mut fmt_end = text.len();
            let mut escaped = false;
            for (offset, c) in text[open..].char_indices() {
                if escaped {
                    fmt.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    fmt_end = open + offset + c.len_utf8();
                    break;
                } else {
                    fmt.push(c);
                }
            }
            attrs_seen += 1;

            let referenced = placeholders(&fmt);
            if referenced.is_empty() {
                continue;
            }
            let end = starts.get(n + 1).copied().unwrap_or(text.len());
            if end <= fmt_end {
                continue;
            }
            let Some(source) = source_field(&text[fmt_end..end]) else {
                continue;
            };
            if referenced.contains(&source) {
                let line = text[..start].matches('\n').count() + 1;
                offenders.push(format!(
                    "{}:{}  #[error(\"{}\")] interpolates its source `{{{}}}`",
                    file.display(),
                    line,
                    fmt,
                    source
                ));
            }
        }
    }

    // A parser that silently matches nothing would pass this gate forever.
    assert!(
        attrs_seen > 30,
        "the #[error(\"…\")] scan looks wrong (only {attrs_seen} attributes found)"
    );

    assert!(
        offenders.is_empty(),
        "an error message interpolates its own source, which double-prints \
         under any reporter that walks `.source()` — drop the placeholder and \
         let the chain supply the cause:\n{}",
        offenders.join("\n")
    );
}

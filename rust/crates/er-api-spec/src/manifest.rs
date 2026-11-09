//! Handler documentation and JSON request-body derivation, lifted from the MCP gate.

/// One routed handler, as the manifest records it.
#[derive(Debug, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub method: String,
    pub path: String,
    pub role: String,
    pub summary: String,
    /// Path parameters, in path order — every `{...}` segment.
    pub params: Vec<String>,
    /// Whether a call needs a request body this manifest cannot describe.
    pub needs_body: bool,
    /// The request body's JSON Schema, derived from the handler's `Json<T>`
    /// extractor (O10b). Empty when the route takes no body.
    pub body: String,
}

/// The role a doc comment's prefix names.
pub fn role_of(line: &str) -> Option<&'static str> {
    // The role is the first word before the colon, which may be QUALIFIED —
    // `Admin+ (operator for structural blocks):` is a real one. Matching the
    // whole literal `Admin+:` misses it, and a missed role means a routed
    // handler silently absent from the tool list.
    let head = line.split(':').next()?;
    let word = head.split_whitespace().next()?.trim_end_matches('+');
    match word {
        "Operator" => Some("operator"),
        "Admin" => Some("admin"),
        "Steward" => Some("steward"),
        "Viewer" => Some("viewer"),
        "Public" => Some("public"),
        _ => None,
    }
}

/// `{org}` / `{job_id}` segments, in path order.
pub fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| {
            seg.strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(str::to_string)
        })
        .collect()
}

/// A stable tool name: the method plus the path's literal segments.
///
/// Derived rather than hand-named so two routes cannot collide silently and a
/// renamed route shows up as a changed tool rather than a quietly repointed
/// one.
pub fn tool_name(method: &str, path: &str) -> String {
    let mut parts = vec![method.to_ascii_lowercase()];
    for seg in path.split('/') {
        if seg.is_empty() || seg == "v1" {
            continue;
        }
        // A `{param}` becomes `by_param`, NOT nothing. Dropping it collapses
        // `/v1/orgs` and `/v1/orgs/{org}` onto one name — two different tools
        // under one label, where a caller gets whichever the lookup finds
        // first.
        if let Some(param) = seg.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            parts.push(format!("by_{}", param.replace('-', "_")));
        } else {
            parts.push(seg.replace(['-', '.', ':'], "_"));
        }
    }
    parts.join("_")
}

/// Every handler doc comment of the form `Role: summary (`METHOD /path`)`.
pub fn derive_tools(src: &str) -> Vec<Tool> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<Tool> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(doc) = line.trim_start().strip_prefix("/// ") else {
            continue;
        };
        let Some(role) = role_of(doc) else { continue };

        // The summary runs to the end of the sentence; the method and path sit
        // in backticks, possibly on the NEXT doc line when the first wraps.
        let mut block = doc.to_string();
        for next in lines.iter().skip(i + 1).take(3) {
            let Some(cont) = next.trim_start().strip_prefix("///") else {
                break;
            };
            block.push(' ');
            block.push_str(cont.trim());
        }

        let Some((method, path)) = method_and_path(&block) else {
            continue;
        };
        // One sentence. The doc comments are written for a human reading the
        // source and some run for a paragraph; an MCP client shows this in a
        // tool picker, where a paragraph is noise that crowds out the other
        // tools' names.
        let summary = summary(&block);

        let params = path_params(&path);
        // O10b: a body is DESCRIBED when the handler's `Json<T>` resolves to a
        // schema. `needs_body` now means "takes a body this manifest could not
        // describe" — which, with the derivation below, is only a type the
        // mapping does not know, and that is a hard failure rather than a
        // silently untooled route.
        // Only a method that CAN carry a body gets one looked for. Without
        // this the forward scan wanders into the next handler's signature and
        // hands a GET a request body it cannot have — which `GET /healthz`
        // demonstrated.
        let can_have_body = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
        let body = if can_have_body {
            body_type_after(&lines, i)
                .and_then(|t| body_schema(src, &t))
                .unwrap_or_default()
        } else {
            String::new()
        };
        // Whether a route takes a body is a property of its SIGNATURE, not a
        // list someone maintains: `body_type_after` finds the `Json<T>`
        // extractor or does not. The hand-kept list this replaced was already
        // wrong about four routes — publish, resume, activate and compile all
        // take their whole input from the path, and were being withheld from
        // the tool list for a body they never had.
        let non_json = can_have_body && NON_JSON_BODIES.contains(&path.as_str());
        let takes_body = !body.is_empty() || non_json;
        let needs_body = non_json;

        out.push(Tool {
            name: tool_name(&method, &path),
            method,
            path,
            role: role.to_string(),
            summary,
            params,
            needs_body,
            body: if takes_body { body } else { String::new() },
        });
    }
    out.sort_by(|a, b| (&a.method, &a.path).cmp(&(&b.method, &b.path)));
    out.dedup_by(|a, b| a.method == b.method && a.path == b.path);
    out
}

/// One field of a request-body struct.
pub struct Field {
    pub name: String,
    pub ty: String,
    /// `#[serde(default)]` — absent is legal, so the field is not required.
    pub optional: bool,
}

/// Parse `struct <name> { … }` out of the server source.
///
/// Returns `None` for a type that is not a local struct — `serde_json::Value`
/// is the real case, a passthrough body with no shape to describe.
pub fn struct_fields(src: &str, name: &str) -> Option<Vec<Field>> {
    let header = format!("\nstruct {name} {{\n");
    let start = src.find(&header)? + header.len();
    let end = start + src[start..].find("\n}\n")?;
    let mut fields = Vec::new();
    let mut defaulted = false;
    for raw in src[start..end].lines() {
        let line = raw.trim();
        if line.starts_with("///") || line.starts_with("//") || line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            // `#[serde(default)]` and `#[serde(default, …)]` both mean the
            // field may be absent. Anything else on the attribute is about
            // serialisation, not presence.
            defaulted |= line.contains("default");
            continue;
        }
        let (field, ty) = line.trim_end_matches(',').split_once(": ")?;
        fields.push(Field {
            name: field.trim().trim_start_matches("pub ").to_string(),
            ty: ty.trim().to_string(),
            optional: defaulted || ty.trim().starts_with("Option<"),
        });
        defaulted = false;
    }
    Some(fields)
}

/// A Rust field type as JSON Schema.
///
/// Returns `None` for a type this mapping does not know. That is a HARD
/// FAILURE at the call site rather than a permissive `{}`: a schema that says
/// "anything" for a field the server will reject is worse than no tool, since
/// the model calls it confidently and reads a 422 it cannot act on.
pub fn type_schema(ty: &str) -> Option<String> {
    let inner = ty
        .strip_prefix("Option<")
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(ty);
    Some(match inner {
        "String" => "{\"type\":\"string\"}".to_string(),
        "bool" => "{\"type\":\"boolean\"}".to_string(),
        "i64" | "usize" | "u32" | "u64" => "{\"type\":\"integer\"}".to_string(),
        "f64" => "{\"type\":\"number\"}".to_string(),
        // A passthrough body or free-form map: no shape to assert, and
        // asserting one would be a guess.
        "Value" | "serde_json::Value" => "{}".to_string(),
        "std::collections::BTreeMap<String, String>" => {
            "{\"type\":\"object\",\"additionalProperties\":{\"type\":\"string\"}}".to_string()
        }
        other => {
            let item = other
                .strip_prefix("Vec<")
                .and_then(|t| t.strip_suffix('>'))?;
            format!("{{\"type\":\"array\",\"items\":{}}}", type_schema(item)?)
        }
    })
}

/// The JSON Schema for a `Json<T>` request body.
///
/// Recurses one level into nested structs (`Vec<BulkResolveItem>`), because a
/// tool that says "an array of something" is one the model cannot fill in.
pub fn body_schema(src: &str, type_name: &str) -> Option<String> {
    let Some(fields) = struct_fields(src, type_name) else {
        // Not a local struct — `Json<Value>`, a passthrough. Describable as
        // "an object", which is honest rather than a guess.
        return Some("{\"type\":\"object\"}".to_string());
    };
    let mut props: Vec<String> = Vec::new();
    let mut required: Vec<String> = Vec::new();
    for f in &fields {
        // A nested local struct recurses rather than giving up: a tool that
        // says "an array of something" is one the model cannot fill in.
        let schema = if let Some(s) = type_schema(&f.ty) {
            s
        } else {
            let item =
                f.ty.trim_start_matches("Option<")
                    .trim_end_matches('>')
                    .strip_prefix("Vec<")
                    .map(|t| t.trim_end_matches('>'))?;
            format!(
                "{{\"type\":\"array\",\"items\":{}}}",
                body_schema(src, item)?
            )
        };
        props.push(format!("\"{}\":{schema}", f.name));
        if !f.optional {
            required.push(format!("\"{}\"", f.name));
        }
    }
    Some(format!(
        "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}],\"additionalProperties\":false}}",
        props.join(","),
        required.join(",")
    ))
}

/// The `Json<T>` body type a handler takes, if any.
///
/// Found by walking forward from the doc comment to the handler's signature —
/// the same adjacency the summary relies on.
pub fn body_type_after(lines: &[&str], doc_index: usize) -> Option<String> {
    let mut seen_fn = false;
    for line in lines.iter().skip(doc_index).take(40) {
        if line.starts_with("async fn ") || line.starts_with("fn ") {
            seen_fn = true;
        }
        if seen_fn && line.contains("Json<") {
            let after = line.split("Json<").nth(1)?;
            return Some(after.split('>').next()?.trim().to_string());
        }
        // The signature ends at its return type.
        if seen_fn && line.contains("-> ") {
            return None;
        }
    }
    None
}

/// Routes whose body is not JSON and so cannot become tool arguments.
///
/// One entry, and it is honest rather than an oversight: the CSV upload takes
/// `Multipart`, and a file is not something an MCP client can put in a tool
/// call. The JSON push (`records:ingest`) is the route an agent should use.
const NON_JSON_BODIES: [&str; 1] = ["/v1/orgs/{org}/imports"];

/// `(`GET /v1/orgs/{org}/jobs`)` → `("GET", "/v1/orgs/{org}/jobs")`.
pub fn method_and_path(block: &str) -> Option<(String, String)> {
    for chunk in block.split('`') {
        let mut it = chunk.split_whitespace();
        let method = it.next()?;
        if !matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            continue;
        }
        let path = it.next()?;
        if path.starts_with('/') && it.next().is_none() {
            // Doc comments illustrate query parameters (`?key=…`); the ROUTE
            // is the path alone, and a tool named after the illustration would
            // address nothing.
            let path = path.split('?').next().unwrap_or(path);
            return Some((method.to_string(), path.to_string()));
        }
    }
    None
}

pub fn manifest_json(tools: &[Tool]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("{\n  \"tools\": [\n");
    for (i, t) in tools.iter().enumerate() {
        let params = t
            .params
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "    {{ \"name\": \"{}\", \"method\": \"{}\", \"path\": \"{}\", \
             \"role\": \"{}\", \"params\": [{params}], \"needs_body\": {}, \
             \"body\": {}, \"summary\": {} }}{}",
            t.name,
            t.method,
            t.path,
            t.role,
            t.needs_body,
            if t.body.is_empty() {
                "null".to_string()
            } else {
                t.body.clone()
            },
            serde_json::to_string(&t.summary).expect("summary serialises"),
            if i + 1 == tools.len() { "" } else { "," }
        );
    }
    out.push_str("  ]\n}\n");
    out
}

/// The first prose sentence after the role prefix, without the route signature.
pub fn summary(block: &str) -> String {
    block
        .split_once(':')
        .map_or(block, |(_, rest)| rest.trim())
        .split(" (`")
        .next()
        .unwrap_or("")
        .split(". ")
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('.')
        .to_string()
}

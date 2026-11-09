//! Source-bound operation derivation for OpenAPI. The original artifact adapters
//! keep their established views; this consumer also needs signatures and local
//! response helpers, for which a Rust syntax tree avoids guessing across functions.

mod requests;
mod responses;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use syn::{Item, ItemFn, ItemStruct, spanned::Spanned};

use crate::{Envelope, path_params, role_of, routes, summary, tool_name, verbs_in};

/// One registered HTTP operation. Bodies describe transport and envelope shape,
/// not the typed response payloads owned by a later contract layer.
pub struct Operation {
    pub method: String,
    pub path: String,
    pub name: String,
    pub role: String,
    pub summary: String,
    pub params: Vec<Parameter>,
    pub request: Option<Request>,
    pub responses: Vec<Envelope>,
}

pub struct Request {
    pub media_type: &'static str,
    pub required: bool,
    pub schema: Value,
}

pub struct Parameter {
    pub name: String,
    pub schema: Value,
}

struct Source<'a> {
    text: &'a str,
    functions: BTreeMap<String, ItemFn>,
    structs: BTreeMap<String, ItemStruct>,
}

impl<'a> Source<'a> {
    fn parse(text: &'a str) -> Result<Self, String> {
        let file = syn::parse_file(text).map_err(|e| format!("parse handler source: {e}"))?;
        let mut source = Self {
            text,
            functions: BTreeMap::new(),
            structs: BTreeMap::new(),
        };
        for item in file.items {
            match item {
                Item::Fn(f) => {
                    source.functions.insert(f.sig.ident.to_string(), f);
                }
                Item::Struct(s) => {
                    source.structs.insert(s.ident.to_string(), s);
                }
                _ => {}
            }
        }
        Ok(source)
    }

    fn snippet(&self, node: &impl Spanned) -> &str {
        &self.text[node.span().byte_range()]
    }
}

/// Derive every registration, including the separate attempt-token channel. Additional module source is injected
/// by the caller for qualified request types; registered wire schemas are the
/// only names permitted in component references. No file or network I/O occurs.
pub fn operations(
    source: &str,
    modules: &[(&str, &str)],
    wire_schemas: &BTreeSet<String>,
) -> Result<Vec<Operation>, String> {
    let parsed = Source::parse(source)?;
    let modules: BTreeMap<_, _> = modules
        .iter()
        .map(|(name, text)| Source::parse(text).map(|source| (*name, source)))
        .collect::<Result<_, _>>()?;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for route in routes(source) {
        let handlers = method_handlers(route.handlers)?;
        if handlers.keys().cloned().collect::<BTreeSet<_>>() != verbs_in(route.handlers) {
            return Err(format!(
                "unsupported method registration for {}",
                route.path
            ));
        }
        for (method, handler) in handlers {
            if !seen.insert((route.path, method.clone())) {
                return Err(format!("duplicate {method} {}", route.path));
            }
            let function = parsed
                .functions
                .get(&handler)
                .ok_or_else(|| format!("missing handler {handler}"))?;
            let docs = doc_text(&function.attrs);
            // The internal namespace is the per-attempt credential channel, not
            // a role mintable on a tenant API key. Other routes require role docs.
            let internal = route.path.starts_with("/v1/internal/");
            let role = if internal {
                "attempt"
            } else {
                role_of(&docs).ok_or_else(|| format!("missing role on {handler}"))?
            };
            let summary = if internal {
                docs.split(" (`").next().unwrap_or("").trim().to_string()
            } else {
                summary(&docs)
            };
            if summary.is_empty() {
                return Err(format!("missing summary on {handler}"));
            }
            let request = requests::derive(&parsed, &modules, wire_schemas, function)
                .map_err(|e| format!("{method} {} request: {e}", route.path))?;
            let responses = responses::derive(&parsed, &handler, &method)
                .map_err(|e| format!("{method} {} response: {e}", route.path))?;
            result.push(Operation {
                name: tool_name(&method, route.path),
                method,
                path: route.path.to_string(),
                role: role.to_string(),
                summary,
                params: requests::parameters(function, &path_params(route.path))?,
                request,
                responses,
            });
        }
    }
    result.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    Ok(result)
}

fn doc_text(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            let syn::Meta::NameValue(meta) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(lit) = &meta.value else {
                return None;
            };
            let syn::Lit::Str(text) = &lit.lit else {
                return None;
            };
            Some(text.value().trim().to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn method_handlers(expr: &str) -> Result<BTreeMap<String, String>, String> {
    use syn::visit::Visit;
    struct Methods(BTreeMap<String, String>);
    impl<'ast> Visit<'ast> for Methods {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = call.func.as_ref()
                && let Some(name) = path.path.segments.last()
            {
                self.insert(&name.ident.to_string(), call.args.first());
            }
            syn::visit::visit_expr_call(self, call);
        }
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            self.insert(&call.method.to_string(), call.args.first());
            syn::visit::visit_expr_method_call(self, call);
        }
    }
    impl Methods {
        fn insert(&mut self, method: &str, arg: Option<&syn::Expr>) {
            if !matches!(
                method,
                "get"
                    | "post"
                    | "put"
                    | "patch"
                    | "delete"
                    | "head"
                    | "options"
                    | "trace"
                    | "connect"
            ) {
                return;
            }
            if let Some(syn::Expr::Path(path)) = arg
                && let Some(ident) = path.path.get_ident()
            {
                self.0.insert(method.to_uppercase(), ident.to_string());
            }
        }
    }
    let expr: syn::Expr = syn::parse_str(expr.trim().trim_matches(',').trim())
        .map_err(|e| format!("parse route handlers: {e}"))?;
    let mut methods = Methods(BTreeMap::new());
    methods.visit_expr(&expr);
    if methods.0.is_empty() {
        return Err("route has no supported handler".into());
    }
    Ok(methods.0)
}

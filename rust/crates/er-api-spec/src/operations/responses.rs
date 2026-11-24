//! Retain the GET ledger's classification and include success constructors from
//! local response helpers for write handlers. Unknown handlers fail the build.

use super::Source;
use crate::{Envelope, classify_envelope, object_keys};
use std::collections::BTreeSet;
use syn::visit::Visit;

pub(super) fn derive(
    source: &Source<'_>,
    name: &str,
    method: &str,
) -> Result<Vec<Envelope>, String> {
    let function = source
        .functions
        .get(name)
        .ok_or_else(|| format!("missing response helper {name}"))?;
    if method == "GET" {
        return classify_envelope(source.snippet(&function.block), source.text).map(|e| vec![e]);
    }
    let mut collector = Responses {
        source,
        seen: BTreeSet::new(),
        envelopes: Vec::new(),
        error: None,
    };
    collector.function(name);
    if let Some(error) = collector.error {
        return Err(error);
    }
    if collector.envelopes.is_empty() {
        return Err(format!("unclassifiable success response in {name}"));
    }
    Ok(collector.envelopes)
}

struct Responses<'a, 's> {
    source: &'a Source<'s>,
    seen: BTreeSet<String>,
    envelopes: Vec<Envelope>,
    error: Option<String>,
}

impl Responses<'_, '_> {
    fn function(&mut self, name: &str) {
        if !self.seen.insert(name.to_string()) {
            return;
        }
        if let Some(function) = self.source.functions.get(name) {
            self.block(&function.block);
        }
    }
    fn push(&mut self, envelope: Envelope) {
        if !self.envelopes.contains(&envelope) {
            self.envelopes.push(envelope);
        }
    }

    fn block(&mut self, block: &syn::Block) {
        // Explicit returns can occur before the tail expression. The visitor
        // below visits only those, not arbitrary constructors in local values.
        self.visit_block(block);
        if let Some(syn::Stmt::Expr(expr, None)) = block.stmts.last() {
            self.response(expr);
        }
    }

    fn response(&mut self, expr: &syn::Expr) {
        use syn::Expr;
        match expr {
            Expr::Block(expr) => self.block(&expr.block),
            Expr::Match(expr) => {
                for arm in &expr.arms {
                    self.response(&arm.body);
                }
            }
            Expr::If(expr) => {
                self.block(&expr.then_branch);
                if let Some((_, otherwise)) = &expr.else_branch {
                    self.response(otherwise);
                }
            }
            Expr::Await(expr) => self.response(&expr.base),
            Expr::Paren(expr) => self.response(&expr.expr),
            Expr::Return(expr) => {
                if let Some(value) = &expr.expr {
                    self.response(value);
                }
            }
            Expr::MethodCall(expr) if expr.method == "into_response" => {
                self.response(&expr.receiver);
            }
            // These closures are the returned value of the option/result
            // combinator, unlike unrelated callbacks in the handler body.
            Expr::MethodCall(expr) if expr.method == "map_or_else" => {
                for argument in &expr.args {
                    if let Expr::Closure(closure) = argument {
                        self.response(&closure.body);
                    }
                }
            }
            Expr::Tuple(expr) => {
                for value in &expr.elems {
                    self.response(value);
                }
            }
            Expr::Call(call) => self.call(call),
            _ => {}
        }
    }

    fn call(&mut self, call: &syn::ExprCall) {
        let syn::Expr::Path(path) = call.func.as_ref() else {
            return;
        };
        // One segment, generic arguments allowed: `Json::<Name>(value)` names
        // its body by turbofish, which `get_ident` would reject.
        let [segment] = path.path.segments.iter().collect::<Vec<_>>()[..] else {
            return;
        };
        let name = segment.ident.to_string();
        if name == "Json" {
            let snippet = self.source.snippet(call);
            if !object_keys(snippet).iter().any(|key| key == "detail") {
                match classify_envelope(snippet, self.source.text) {
                    Ok(envelope) => self.push(envelope),
                    Err(error) => {
                        self.error.get_or_insert(error);
                    }
                }
            }
            return;
        }
        if let Some(function) = self.source.functions.get(&name) {
            let output = self.source.snippet(&function.sig.output);
            if output.contains("Response") && !output.contains("Result<") {
                self.function(&name);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Responses<'_, '_> {
    fn visit_expr_return(&mut self, expr: &'ast syn::ExprReturn) {
        if let Some(value) = &expr.expr {
            self.response(value);
        }
    }

    // A return inside another function/closure is not the handler's response.
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
    fn visit_item_fn(&mut self, _: &'ast syn::ItemFn) {}
}

/// The component schemas a handler's success responses serialize, when every
/// success `Json(…)` in its body names a registered type:
///
/// - `Json(Name { … })` where `Name` is registered → `Name`
/// - `Json(NameWire::from(…))` (any associated fn) → `Name`, the wire's schema name
/// - `Json::<NameWire>(…)` / `Json::<Vec<NameWire>>(…)` → `Name` / an array of it
///
/// Empty when none is typed: the operation stays envelope-level. A handler that
/// types some success responses and not others is an error, because the
/// document would then describe only part of what the handler can return.
pub(super) fn typed(
    source: &Source<'_>,
    name: &str,
    schemas: &BTreeSet<String>,
) -> Result<Vec<serde_json::Value>, String> {
    let function = source
        .functions
        .get(name)
        .ok_or_else(|| format!("missing response helper {name}"))?;
    let mut calls = JsonCalls {
        source,
        schemas,
        seen: BTreeSet::from([name.to_string()]),
        typed: Vec::new(),
        untyped: Vec::new(),
    };
    calls.visit_block(&function.block);
    if calls.typed.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(untyped) = calls.untyped.first() {
        return Err(format!(
            "{name} mixes typed and untyped success responses; untyped: {untyped}"
        ));
    }
    let mut out = Vec::new();
    for schema in calls.typed {
        if !out.contains(&schema) {
            out.push(schema);
        }
    }
    Ok(out)
}

struct JsonCalls<'a, 's> {
    source: &'a Source<'s>,
    schemas: &'a BTreeSet<String>,
    /// Local response helpers already followed, so a recursive helper ends.
    seen: BTreeSet<String>,
    typed: Vec<serde_json::Value>,
    untyped: Vec<String>,
}

impl JsonCalls<'_, '_> {
    fn reference(&self, type_name: &str) -> Option<serde_json::Value> {
        let name = type_name.strip_suffix("Wire").unwrap_or(type_name);
        self.schemas
            .contains(name)
            .then(|| serde_json::json!({"$ref": format!("#/components/schemas/{name}")}))
    }

    /// `NameWire` or `Vec<NameWire>` written as a turbofish on `Json`.
    fn turbofish(&self, ty: &syn::Type) -> Option<serde_json::Value> {
        let syn::Type::Path(path) = ty else {
            return None;
        };
        let last = path.path.segments.last()?;
        if last.ident == "Vec" {
            let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
                return None;
            };
            let Some(syn::GenericArgument::Type(item)) = args.args.first() else {
                return None;
            };
            let item = self.turbofish(item)?;
            return Some(serde_json::json!({"type": "array", "items": item}));
        }
        self.reference(&last.ident.to_string())
    }

    fn argument(&self, value: &syn::Expr) -> Option<serde_json::Value> {
        match value {
            syn::Expr::Struct(value) => {
                self.reference(&value.path.segments.last()?.ident.to_string())
            }
            syn::Expr::Call(call) => {
                let syn::Expr::Path(path) = call.func.as_ref() else {
                    return None;
                };
                // `Owner::from(…)`, `Owner::new(…)`: the owner names the type.
                let owner = path.path.segments.iter().rev().nth(1)?.ident.to_string();
                self.reference(&owner)
            }
            _ => None,
        }
    }
}

impl<'ast> Visit<'ast> for JsonCalls<'_, '_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref()
            && let Some(last) = path.path.segments.last()
            && last.ident == "Json"
            && let Some(value) = call.args.first()
        {
            let snippet = self.source.snippet(call);
            if !object_keys(snippet).iter().any(|key| key == "detail") {
                let turbofish = match &last.arguments {
                    syn::PathArguments::AngleBracketed(args) => match args.args.first() {
                        Some(syn::GenericArgument::Type(ty)) => self.turbofish(ty),
                        _ => None,
                    },
                    _ => None,
                };
                match turbofish.or_else(|| self.argument(value)) {
                    Some(schema) => self.typed.push(schema),
                    None => self.untyped.push(snippet.chars().take(80).collect()),
                }
            }
        }
        // A local helper that builds the response (`provision_org(…).await`)
        // is part of what the handler can return, as the envelope collector
        // above also treats it.
        if let syn::Expr::Path(path) = call.func.as_ref()
            && let Some(helper) = path.path.get_ident().map(ToString::to_string)
            && let Some(function) = self.source.functions.get(&helper)
            && {
                let output = self.source.snippet(&function.sig.output);
                output.contains("Response") && !output.contains("Result<")
            }
            && self.seen.insert(helper)
        {
            self.visit_block(&function.block);
        }
        syn::visit::visit_expr_call(self, call);
    }
}

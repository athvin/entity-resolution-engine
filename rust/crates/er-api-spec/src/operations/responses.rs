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
        return classify_envelope(source.snippet(&function.block)).map(|e| vec![e]);
    }
    let mut collector = Responses {
        source,
        seen: BTreeSet::new(),
        envelopes: Vec::new(),
    };
    collector.function(name);
    if collector.envelopes.is_empty() {
        return Err(format!("unclassifiable success response in {name}"));
    }
    Ok(collector.envelopes)
}

struct Responses<'a, 's> {
    source: &'a Source<'s>,
    seen: BTreeSet<String>,
    envelopes: Vec<Envelope>,
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
        let Some(name) = path.path.get_ident() else {
            return;
        };
        let name = name.to_string();
        if name == "Json" {
            let snippet = self.source.snippet(call);
            if !object_keys(snippet).iter().any(|key| key == "detail")
                && let Ok(envelope) = classify_envelope(snippet)
            {
                self.push(envelope);
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

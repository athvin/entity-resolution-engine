//! Request signatures and serde field shapes. Unknown types/attributes are an
//! error, rather than the old MCP parser's deliberately preserved fallback.

use super::{Parameter, Request, Source};

pub(super) fn parameters(
    function: &syn::ItemFn,
    names: &[String],
) -> Result<Vec<Parameter>, String> {
    let path_type = function.sig.inputs.iter().find_map(|argument| {
        let syn::FnArg::Typed(argument) = argument else {
            return None;
        };
        generic(&argument.ty, "Path")
    });
    let types = match path_type {
        Some(Type::Tuple(tuple)) => tuple.elems.iter().collect::<Vec<_>>(),
        Some(ty) => vec![ty],
        None => vec![],
    };
    if types.len() != names.len() {
        return Err(format!(
            "path parameters do not match {} signature",
            function.sig.ident
        ));
    }
    names
        .iter()
        .zip(types)
        .map(|(name, ty)| {
            let schema = match type_name(ty).as_deref() {
                Some("String") => json!({"type":"string"}),
                Some("i64" | "i32" | "u64" | "u32" | "usize") => json!({"type":"integer"}),
                Some("bool") => json!({"type":"boolean"}),
                _ => return Err(format!("unsupported path parameter {name}")),
            };
            Ok(Parameter {
                name: name.clone(),
                schema,
            })
        })
        .collect()
}
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use syn::{GenericArgument, PathArguments, Type};

pub(super) fn derive(
    source: &Source<'_>,
    modules: &BTreeMap<&str, Source<'_>>,
    schemas: &BTreeSet<String>,
    function: &syn::ItemFn,
) -> Result<Option<Request>, String> {
    let mut request = None;
    for argument in &function.sig.inputs {
        let syn::FnArg::Typed(argument) = argument else {
            continue;
        };
        let (ty, required) = if let Some(inner) = generic(&argument.ty, "Option") {
            (inner, false)
        } else {
            (argument.ty.as_ref(), true)
        };
        let body = if let Some(inner) = generic(ty, "Json") {
            Some(Request {
                media_type: "application/json",
                required,
                schema: schema(source, modules, schemas, inner, &mut BTreeSet::new())?,
            })
        } else if type_name(ty).is_some_and(|n| n == "Multipart") {
            Some(Request {
                media_type: "multipart/form-data",
                required,
                schema: multipart_schema(&function.block)?,
            })
        } else if type_name(ty).is_some_and(|name| {
            matches!(
                name.as_str(),
                "Principal" | "State" | "Path" | "Query" | "RawQuery" | "HeaderMap"
            )
        }) {
            None
        } else {
            return Err(format!(
                "unsupported handler extractor {}",
                source.snippet(ty)
            ));
        };
        if body.is_some() {
            if request.is_some() {
                return Err("multiple body extractors".into());
            }
            request = body;
        }
    }
    Ok(request)
}

fn multipart_schema(body: &syn::Block) -> Result<Value, String> {
    use syn::visit::Visit;
    // The stream accepts exactly the named part and rejects an absent upload.
    // Derive its name from the comparison instead of maintaining a route table.
    #[derive(Default)]
    struct Fields {
        names: Vec<String>,
        stream: bool,
    }
    impl<'ast> Visit<'ast> for Fields {
        fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
            if matches!(expr.op, syn::BinOp::Ne(_))
                && let syn::Expr::MethodCall(left) = expr.left.as_ref()
                && left.method == "name"
                && let syn::Expr::Call(right) = expr.right.as_ref()
                && let syn::Expr::Path(path) = right.func.as_ref()
                && path.path.is_ident("Some")
                && let Some(syn::Expr::Lit(lit)) = right.args.first()
                && let syn::Lit::Str(name) = &lit.lit
            {
                self.names.push(name.value());
            }
            syn::visit::visit_expr_binary(self, expr);
        }
        fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
            self.stream |= expr.method == "next_field";
            syn::visit::visit_expr_method_call(self, expr);
        }
    }
    let mut fields = Fields::default();
    fields.visit_block(body);
    if fields.names.len() != 1 || !fields.stream {
        return Err("unsupported multipart field selection".into());
    }
    let name = fields.names[0].as_str();
    Ok(
        json!({"type":"object", "properties":{name:{"type":"string","format":"binary"}}, "required":[name]}),
    )
}

fn type_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    Some(path.path.segments.last()?.ident.to_string())
}

fn generic<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}

fn schema(
    source: &Source<'_>,
    modules: &BTreeMap<&str, Source<'_>>,
    schemas: &BTreeSet<String>,
    ty: &Type,
    stack: &mut BTreeSet<String>,
) -> Result<Value, String> {
    if let Some(inner) = generic(ty, "Option") {
        return Ok(json!({"anyOf":[schema(source,modules,schemas,inner,stack)?,{"type":"null"}]}));
    }
    if let Some(inner) = generic(ty, "Vec") {
        return Ok(json!({"type":"array","items":schema(source,modules,schemas,inner,stack)?}));
    }
    let Type::Path(path) = ty else {
        return Err("unsupported request field type".into());
    };
    let segment = path.path.segments.last().ok_or("empty type path")?;
    let name = segment.ident.to_string();
    match name.as_str() {
        "String" => return Ok(json!({"type":"string"})),
        "bool" => return Ok(json!({"type":"boolean"})),
        "i64" | "i32" | "u64" | "u32" | "usize" => return Ok(json!({"type":"integer"})),
        "f64" | "f32" => return Ok(json!({"type":"number"})),
        "Value" => return Ok(json!({})),
        "BTreeMap" | "HashMap" => {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                return Err("map lacks type arguments".into());
            };
            let types: Vec<_> = args
                .args
                .iter()
                .filter_map(|arg| {
                    if let GenericArgument::Type(t) = arg {
                        Some(t)
                    } else {
                        None
                    }
                })
                .collect();
            if types.len() != 2 || type_name(types[0]).as_deref() != Some("String") {
                return Err("JSON map key must be String".into());
            }
            return Ok(
                json!({"type":"object","additionalProperties":schema(source,modules,schemas,types[1],stack)?}),
            );
        }
        _ => {}
    }
    if path
        .path
        .segments
        .first()
        .is_some_and(|s| s.ident == "er_entities")
        && let Some(entity) = name.strip_suffix("Wire")
    {
        if !schemas.contains(entity) {
            return Err(format!("unregistered wire schema {entity}"));
        }
        return Ok(json!({"$ref":format!("#/components/schemas/{entity}")}));
    }
    let qualified = path
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>();
    let owner = if qualified.len() == 1 {
        source
    } else {
        modules
            .get(qualified[0].as_str())
            .ok_or_else(|| format!("missing source for {}", qualified.join("::")))?
    };
    let definition = owner
        .structs
        .get(&name)
        .ok_or_else(|| format!("unsupported request type {name}"))?;
    let key = qualified.join("::");
    if !stack.insert(key.clone()) {
        return Err(format!("recursive request type {key}"));
    }
    let container = attributes(&definition.attrs)?;
    let syn::Fields::Named(fields) = &definition.fields else {
        return Err(format!("unsupported tuple request {name}"));
    };
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in &fields.named {
        let attributes = attributes(&field.attrs)?;
        let field_name = attributes
            .rename
            .unwrap_or_else(|| field.ident.as_ref().expect("named field").to_string());
        properties.insert(
            field_name.clone(),
            schema(owner, modules, schemas, &field.ty, stack)?,
        );
        if !attributes.default && !container.default && generic(&field.ty, "Option").is_none() {
            required.push(field_name);
        }
    }
    stack.remove(&key);
    let mut result = json!({"type":"object","properties":properties,"required":required});
    // serde ignores unknown fields by default; false is justified only by an
    // actual deny_unknown_fields attribute, never by the fact a struct exists.
    if container.deny_unknown {
        result["additionalProperties"] = json!(false);
    }
    Ok(result)
}

#[derive(Default)]
struct Attributes {
    default: bool,
    deny_unknown: bool,
    rename: Option<String>,
}

fn attributes(attrs: &[syn::Attribute]) -> Result<Attributes, String> {
    let mut result = Attributes::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                result.default = true;
                if meta.input.peek(syn::Token![=]) {
                    let _: syn::LitStr = meta.value()?.parse()?;
                }
            } else if meta.path.is_ident("deny_unknown_fields") {
                result.deny_unknown = true;
            } else if meta.path.is_ident("rename") {
                result.rename = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else {
                return Err(meta.error("unsupported serde request attribute"));
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(result)
}

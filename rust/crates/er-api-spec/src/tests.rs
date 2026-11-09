use crate::{
    body_schema, derive_tools, get_handler_in, get_routes, method_and_path, path_params, role_of,
    routes, struct_fields, summary, tool_name, type_schema, verbs_in,
};

#[test]
fn one_walk_retains_commented_multiline_and_chained_registrations() {
    let src = r#"
        Router::new()
            .route("/v1/orgs/{org}", get(get_org).post(create_org))
            // This registration used to disappear from the regex walk.
            .route(
                "/v1/orgs/{org}/tags/{tag_id}",
                axum::routing::patch(update_tag).delete(delete_tag),
            )
            .route("/healthz", get(healthz))
    "#;
    let found = routes(src);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].path, "/v1/orgs/{org}");
    assert_eq!(
        verbs_in(found[0].handlers).into_iter().collect::<Vec<_>>(),
        ["GET", "POST"]
    );
    assert_eq!(
        verbs_in(found[1].handlers).into_iter().collect::<Vec<_>>(),
        ["DELETE", "PATCH"]
    );
    assert_eq!(get_handler_in(found[1].handlers), None);
    let gets = get_routes(src);
    assert_eq!(gets.len(), 2);
    assert_eq!(gets["/v1/orgs/{org}"], "get_org");
    assert_eq!(gets["/healthz"], "healthz");
    assert_eq!(routes("Router::new()"), Vec::new());
}

#[test]
fn method_identifiers_have_the_existing_boundary_rules() {
    assert!(verbs_in("forget(fake)").is_empty());
    assert_eq!(get_handler_in("forget(fake)"), None);
    assert_eq!(
        get_handler_in("axum::routing::get(real)"),
        Some("real".into())
    );
}

#[test]
fn qualified_roles_named_parameters_and_query_examples_keep_their_meaning() {
    let doc = "Admin+ (operator for structural blocks): Read jobs (`GET /v1/orgs/{org}/jobs?state=queued`). More detail.";
    assert_eq!(role_of(doc), Some("admin"));
    let (method, path) = method_and_path(doc).unwrap();
    assert_eq!(method, "GET");
    assert_eq!(path, "/v1/orgs/{org}/jobs");
    assert_eq!(summary(doc), "Read jobs");
    assert_eq!(path_params(&path), ["org"]);
    assert_ne!(
        tool_name("GET", "/v1/orgs"),
        tool_name("GET", "/v1/orgs/{org}")
    );
    assert_eq!(tool_name("GET", &path), "get_orgs_by_org_jobs");
}

#[test]
fn body_derivation_uses_only_the_supplied_source_and_keeps_optional_fields() {
    let src = r"
struct Input {
    name: String,
    #[serde(default)]
    enabled: bool,
    note: Option<String>,
    items: Vec<Item>,
}

struct Item {
    id: i64,
}
";
    let schema: serde_json::Value =
        serde_json::from_str(&body_schema(src, "Input").unwrap()).unwrap();
    assert_eq!(schema["required"], serde_json::json!(["name", "items"]));
    assert_eq!(
        schema["properties"]["items"]["items"]["required"],
        serde_json::json!(["id"])
    );
    assert_eq!(schema["properties"]["note"]["type"], "string");
    assert_eq!(schema["additionalProperties"], false);
    assert!(struct_fields("", "Input").is_none());
    assert_eq!(type_schema("Unsupported"), None);
    assert_eq!(
        body_schema(src, "serde_json::Value"),
        Some("{\"type\":\"object\"}".into())
    );
}

#[test]
fn a_get_never_borrows_the_following_handlers_body() {
    let src = r"
/// Public: Liveness (`GET /healthz`).
async fn healthz() -> StatusCode { StatusCode::OK }

/// Admin+ (operator for structural blocks): Create (`POST /v1/items`).
async fn create(Json(body): Json<Input>) -> Json<Value> { Json(json!(body)) }

struct Input {
    value: String,
}
";
    let tools = derive_tools(src);
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].path, "/healthz");
    assert_eq!(tools[0].body, "");
    assert!(!tools[0].needs_body);
    assert_eq!(tools[1].role, "admin");
    let body: serde_json::Value = serde_json::from_str(&tools[1].body).unwrap();
    assert_eq!(body["required"], serde_json::json!(["value"]));
    assert_eq!(derive_tools("// no documented handlers"), Vec::new());
}

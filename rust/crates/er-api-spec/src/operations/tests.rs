use crate::{Envelope, operations};
use serde_json::json;
use std::collections::BTreeSet;

#[test]
fn typed_responses_reach_both_contract_adapters() {
    let src = r#"
fn router() { Router::new().route("/jobs", get(read).post(write)); }
struct JobsResponse {
    jobs: Vec<Job>,
}

struct CreatedResponse {
    id: String,
    created: bool,
}
/// Viewer: read jobs (`GET /jobs`).
async fn read() { Json(JobsResponse { jobs }) }
/// Steward: create a job (`POST /jobs`).
async fn write() { Json(CreatedResponse { id, created: true }) }
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    assert_eq!(ops[0].responses, [Envelope::Object(vec!["jobs".into()])]);
    assert_eq!(
        ops[1].responses,
        [Envelope::Object(vec!["id".into(), "created".into()])]
    );
    assert_eq!(
        crate::envelope_ledger(src).unwrap(),
        "{\n  \"GET /jobs\": { \"envelope\": \"wrapped\", \"key\": \"jobs\" }\n}\n"
    );
}

#[test]
fn unsupported_typed_responses_fail_even_beside_a_readable_write_branch() {
    let src = r#"
fn router() { Router::new().route("/jobs", post(write)); }
struct Reply {
    #[serde(rename = "items")]
    jobs: Vec<Job>,
}
/// Steward: write jobs (`POST /jobs`).
async fn write() {
    if cached { Json(json!({"cached": true})) }
    else { Json(Reply { jobs }) }
}
"#;
    let error = operations(src, &[], &BTreeSet::new()).err().unwrap();
    assert!(error.contains("response struct Reply"), "{error}");
    let read = src
        .replace("post(write)", "get(write)")
        .replace("POST /jobs", "GET /jobs");
    // The GET adapter and ledger must also reject an unreadable typed-only response.
    let read = read.replace(
        "if cached { Json(json!({\"cached\": true})) }\n    else { Json(Reply { jobs }) }",
        "Json(Reply { jobs })",
    );
    assert!(operations(&read, &[], &BTreeSet::new()).is_err());
    assert!(crate::envelope_ledger(&read).is_err());
}

#[test]
fn chained_routes_keep_their_own_bodies_and_serde_policy() {
    let src = r#"
fn router() { Router::new().route("/things/{id}", get(read).patch(write)); }
/// Viewer+: read a thing (`GET /things/{id}`).
async fn read(Path(id): Path<String>) { Json(json!({"thing": value})) }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    name: String,
    #[serde(default)]
    enabled: bool,
    note: Option<String>,
    children: Vec<Child>,
}
struct Child { count: u32 }
/// Admin+: change a thing (`PATCH /things/{id}`).
async fn write(Path(id): Path<i64>, body: Option<Json<Input>>) { Json(json!({"changed": true})) }
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    assert_eq!(ops.len(), 2);
    let read = ops.iter().find(|o| o.method == "GET").unwrap();
    assert!(read.request.is_none());
    assert_eq!(read.params[0].name, "id");
    assert_eq!(read.params[0].schema, json!({"type":"string"}));
    let write = ops.iter().find(|o| o.method == "PATCH").unwrap();
    let body = write.request.as_ref().unwrap();
    assert_eq!(write.params[0].schema, json!({"type":"integer"}));
    assert!(!body.required);
    assert_eq!(body.schema["required"], json!(["name", "children"]));
    assert_eq!(body.schema["additionalProperties"], false);
    assert_eq!(
        body.schema["properties"]["note"]["anyOf"][1],
        json!({"type":"null"})
    );
    assert_eq!(
        body.schema["properties"]["children"]["items"]["properties"]["count"],
        json!({"type":"integer"})
    );
}

#[test]
fn qualified_module_inputs_and_registered_wire_references_are_source_bound() {
    let src = r#"
fn router() { Router::new().route("/mail", post(mail)).route("/wire", put(wire)); }
/// Operator: send mail (`POST /mail`).
async fn mail(body: Json<mail::Message>) { Json(json!(true)) }
/// Admin: put a wire (`PUT /wire`).
async fn wire(body: Json<er_entities::org::OrgWire>) { Json(json!(true)) }
"#;
    let module = "pub struct Message { pub to: String, pub values: BTreeMap<String, Value> }";
    let schemas = BTreeSet::from(["Org".to_string()]);
    let ops = operations(src, &[("mail", module)], &schemas).unwrap();
    let body = ops[0].request.as_ref().unwrap();
    assert_eq!(body.schema["properties"]["to"], json!({"type":"string"}));
    assert!(body.schema.get("additionalProperties").is_none());
    assert_eq!(
        body.schema["properties"]["values"]["additionalProperties"],
        json!({})
    );
    assert_eq!(
        ops[1].request.as_ref().unwrap().schema,
        json!({"$ref":"#/components/schemas/Org"})
    );
    assert!(operations(src, &[], &schemas).is_err());
    assert!(operations(src, &[("mail", module)], &BTreeSet::new()).is_err());
}

#[test]
fn write_responses_follow_helpers_without_publishing_error_envelopes() {
    let src = r#"
fn router() { Router::new().route("/write", post(write)); }
/// Steward: write (`POST /write`).
async fn write() -> impl IntoResponse { shared().await }
async fn shared() -> Response {
    if bad { return Json(json!({"detail": "bad"})).into_response(); }
    if one { Json(json!({"created": id})).into_response() }
    else { Json(json!({"existing": id})).into_response() }
}
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    assert_eq!(
        ops[0].responses,
        [
            Envelope::Object(vec!["created".into()]),
            Envelope::Object(vec!["existing".into()])
        ]
    );
}

#[test]
fn multipart_field_names_are_derived_and_unknown_selection_is_rejected() {
    let src = r#"
fn router() { Router::new().route("/upload", post(upload)); }
/// Steward: upload (`POST /upload`).
async fn upload(mut multipart: Multipart) -> Response {
    let field = multipart.next_field().await;
    if field.name() != Some("delivery") { continue; }
    Json(json!({"accepted":true})).into_response()
}
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    let body = ops[0].request.as_ref().unwrap();
    assert_eq!(body.media_type, "multipart/form-data");
    assert_eq!(body.schema["required"], json!(["delivery"]));
    assert_eq!(body.schema["properties"]["delivery"]["format"], "binary");
    assert!(
        operations(
            &src.replace(".name() !=", ".name() =="),
            &[],
            &BTreeSet::new()
        )
        .is_err()
    );
}

#[test]
fn unsupported_source_shapes_fail_instead_of_emitting_partial_operations() {
    let src = r#"
fn router() { Router::new().route("/write", post(write)); }
/// Admin: write (`POST /write`).
async fn write(body: Json<Input>) -> Response { Json(json!(true)).into_response() }
struct Input { name: String }
"#;
    for bad in [
        src.replace("Admin:", "Unknown:"),
        src.replace("name: String", "name: Unsupported"),
        src.replace(
            "Json(json!(true)).into_response()",
            "unsupported_response()",
        ),
        src.replace("name: String", "#[serde(flatten)] name: String"),
        src.replace("struct Input", "struct Other"),
        src.replace("Json<Input>", "Bytes"),
        src.replace("post(write)", "post(write).head(write)"),
        src.replace(
            "Json(json!(true)).into_response()",
            "let unused = Json(json!({\"decoy\":true})); unsupported_response()",
        ),
    ] {
        assert!(
            operations(&bad, &[], &BTreeSet::new()).is_err(),
            "accepted unsupported source: {bad}"
        );
    }
}

#[test]
fn internal_attempt_operations_use_a_separate_credential_channel() {
    let src = r#"
fn router() { Router::new().route("/v1/internal/attempts/{id}", get(work)); }
/// Read a work order (`GET /v1/internal/attempts/{id}`).
async fn work(Path(id): Path<String>, headers: HeaderMap) { Json(json!({"input": value})) }
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].role, "attempt");
    assert_eq!(ops[0].summary, "Read a work order");
    assert_eq!(ops[0].params[0].name, "id");
}

#[test]
fn returned_map_or_else_closures_contribute_success_responses_only() {
    let src = r#"
fn router() { Router::new().route("/v1/internal/attempts/{id}/start", post(start)); }
/// Report attempt start (`POST /v1/internal/attempts/{id}/start`).
async fn start(Path(id): Path<String>) {
    let unrelated = || Json(json!({"not_returned": true}));
    rows.next().map_or_else(
        || Json(json!({"detail":"stale"})).into_response(),
        |row| { let background = async { Json(json!({"not_returned_either": true})) }; (StatusCode::OK, Json(json!(row))).into_response() }
    )
}
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    assert_eq!(ops[0].responses, vec![Envelope::Bare]);
}

#[test]
fn registered_response_types_become_component_references() {
    let src = r#"
fn router() {
    Router::new()
        .route("/jobs", get(list).post(create))
        .route("/jobs/{id}", get(one))
        .route("/all", get(all))
        .route("/resolve", post(resolve))
        .route("/mixed", get(mixed));
}
struct JobList {
    jobs: Vec<JobWire>,
}
/// Viewer: list jobs (`GET /jobs`).
async fn list() { Json(JobList { jobs }) }
/// Steward: create a job (`POST /jobs`).
async fn create() {
    if bad { return (StatusCode::BAD_REQUEST, Json(json!({"detail": "no"}))).into_response(); }
    (StatusCode::ACCEPTED, Json(er_entities::job::JobWire::from(&job))).into_response()
}
/// Viewer: one job (`GET /jobs/{id}`).
async fn one(Path(id): Path<String>) { Json(JobWire::from(&job)) }
/// Operator: every job (`GET /all`).
async fn all() { Json::<Vec<JobWire>>(jobs.iter().map(JobWire::from).collect()) }
/// Steward: a write that names its body by turbofish (`POST /resolve`).
async fn resolve() { match go() { Ok(body) => Json::<JobList>(body).into_response(), Err(e) => e } }
/// Viewer: mixed (`GET /mixed`).
async fn mixed() { if a { Json(JobWire::from(&job)) } else { Json(json!(job)) } }
"#;
    let schemas: BTreeSet<String> = ["Job", "JobList"].map(String::from).into();
    let error = operations(src, &[], &schemas).err().unwrap();
    assert!(error.contains("mixes typed and untyped"), "{error}");

    let src = src.replace(".route(\"/mixed\", get(mixed))", "");
    let ops = operations(&src, &[], &schemas).unwrap();
    let schema = |path: &str, method: &str| {
        ops.iter()
            .find(|o| o.path == path && o.method == method)
            .unwrap()
            .response_schemas
            .clone()
    };
    let job = json!({"$ref": "#/components/schemas/Job"});
    assert_eq!(
        schema("/jobs", "GET"),
        [json!({"$ref": "#/components/schemas/JobList"})]
    );
    assert_eq!(schema("/jobs", "POST"), std::slice::from_ref(&job));
    assert_eq!(schema("/jobs/{id}", "GET"), std::slice::from_ref(&job));
    assert_eq!(
        schema("/all", "GET"),
        [json!({"type": "array", "items": job})]
    );
    // A write handler's turbofish over a variable names its body too.
    assert_eq!(
        schema("/resolve", "POST"),
        [json!({"$ref": "#/components/schemas/JobList"})]
    );

    // The ledger reads the wire spellings as the bare values they replace.
    let ledger = crate::envelope_ledger(&src).unwrap();
    assert!(
        ledger.contains("\"GET /all\": { \"envelope\": \"bare\" }"),
        "{ledger}"
    );
    assert!(
        ledger.contains("\"GET /jobs/{id}\": { \"envelope\": \"bare\" }"),
        "{ledger}"
    );

    // An unregistered type stays envelope-level rather than inventing a reference.
    let ops = operations(&src, &[], &BTreeSet::new()).unwrap();
    assert!(ops.iter().all(|o| o.response_schemas.is_empty()));
}

#[test]
fn typed_responses_follow_local_helpers_and_registered_owners() {
    let src = r#"
fn router() { Router::new().route("/orgs", get(list).post(create)); }
struct Created {
    name: String,
}
struct Provisioned {
    name: String,
    key: String,
}
struct Item {
    id: String,
}
/// Operator: list orgs (`GET /orgs`).
async fn list() { Json::<Vec<Item>>(rows.iter().map(Item::from).collect()) }
/// Operator: create an org (`POST /orgs`).
async fn create() -> impl IntoResponse {
    if provisioning { return provision(name).await; }
    (StatusCode::CREATED, Json(Created { name })).into_response()
}
async fn provision(name: &str) -> axum::response::Response {
    (StatusCode::CREATED, Json(Provisioned { name, key })).into_response()
}
"#;
    let schemas: BTreeSet<String> = ["Created", "Provisioned", "Item"].map(String::from).into();
    let ops = operations(src, &[], &schemas).unwrap();
    let reference = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
    let create = ops.iter().find(|o| o.method == "POST").unwrap();
    // The helper's body is part of what the handler returns.
    // The early helper return is visited before the tail expression.
    assert_eq!(
        create.response_schemas,
        [reference("Provisioned"), reference("Created")]
    );
    let list = ops.iter().find(|o| o.method == "GET").unwrap();
    assert_eq!(
        list.response_schemas,
        [json!({"type": "array", "items": reference("Item")})]
    );
    assert!(
        crate::envelope_ledger(src)
            .unwrap()
            .contains("\"GET /orgs\": { \"envelope\": \"bare\" }")
    );

    // A helper still returning `json!` makes the handler mixed, not silently partial.
    let partial = src.replace(
        "Json(Provisioned { name, key })",
        "Json(json!({\"name\": name}))",
    );
    let error = operations(&partial, &[], &schemas).err().unwrap();
    assert!(error.contains("mixes typed and untyped"), "{error}");
}

#[test]
fn query_structs_become_documented_query_parameters() {
    let src = r#"
fn router() { Router::new().route("/things/{id}/items", get(list)); }
#[derive(Deserialize)]
struct ListQuery {
    /// The opaque `next_cursor` of the previous page.
    cursor: Option<String>,
    /// Maximum items to return.
    /// Clamped to the range.
    limit: Option<i64>,
    #[serde(default)]
    verbose: bool,
    kind: String,
}
/// Viewer+: list items (`GET /things/{id}/items`).
async fn list(Path(id): Path<String>, Query(params): Query<ListQuery>) { Json(json!({"items": items})) }
"#;
    let ops = operations(src, &[], &BTreeSet::new()).unwrap();
    let params = &ops[0].params;
    let by = |name: &str| params.iter().find(|p| p.name == name).unwrap();
    // Path first, then the struct's fields in declaration order.
    assert_eq!(
        params
            .iter()
            .map(|p| (p.name.as_str(), p.location))
            .collect::<Vec<_>>(),
        [
            ("id", "path"),
            ("cursor", "query"),
            ("limit", "query"),
            ("verbose", "query"),
            ("kind", "query")
        ]
    );
    assert!(by("id").required);
    assert!(!by("cursor").required, "Option is optional");
    assert!(!by("verbose").required, "#[serde(default)] is optional");
    assert!(by("kind").required, "a bare field is a 400 when absent");
    assert_eq!(by("limit").schema, json!({"type":"integer"}));
    assert_eq!(
        by("limit").description.as_deref(),
        Some("Maximum items to return. Clamped to the range.")
    );
    assert_eq!(by("kind").description, None);
}

#[test]
fn an_untyped_query_bag_fails_generation() {
    let src = r#"
fn router() { Router::new().route("/things", get(list)); }
/// Viewer+: list (`GET /things`).
async fn list(Query(params): Query<HashMap<String, String>>) { Json(json!({"items": items})) }
"#;
    let error = operations(src, &[], &BTreeSet::new()).err().unwrap();
    assert!(error.contains("untyped Query<HashMap"), "{error}");
}

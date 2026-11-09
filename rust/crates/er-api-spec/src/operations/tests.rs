use crate::{Envelope, operations};
use serde_json::json;
use std::collections::BTreeSet;

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

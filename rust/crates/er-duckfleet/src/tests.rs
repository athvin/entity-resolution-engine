use super::*;
use serde_json::{Value, json};

fn fixture() -> (Value, Vec<Value>) {
    let pairs = [
        ("org-a", "dataset-a"),
        ("org-a", "dataset-b"),
        ("org-b", "dataset-c"),
    ];
    let bindings: Vec<_> = pairs
        .iter()
        .map(|(org, ds)| {
            json!({
                "org_id": org, "dataset_id": ds, "generation": 3,
                "state": "active", "valid_until": 200
            })
        })
        .collect();
    let credentials = pairs
        .iter()
        .map(|(org, ds)| {
            json!({
                "org_id": org, "dataset_id": ds, "generation": 3,
                "token": format!("{ds}-{}", "x".repeat(32))
            })
        })
        .collect();
    (
        json!({"bucket": "test-lake", "bindings": bindings}),
        credentials,
    )
}

fn parse(doc: &Value, credentials: &[Value]) -> Result<Registry, Error> {
    Registry::parse(&serde_json::to_vec(doc).unwrap(), |ns| {
        credentials
            .iter()
            .find(|c| {
                namespace(
                    c["org_id"].as_str().unwrap(),
                    c["dataset_id"].as_str().unwrap(),
                ) == ns
            })
            .map(|c| serde_json::to_vec(c).unwrap())
            .ok_or(Error::Credential)
    })
}

#[test]
fn same_org_and_cross_org_deployments_share_no_boundary() {
    let (doc, credentials) = fixture();
    let registry = parse(&doc, &credentials).unwrap();
    let a = registry.resolve("org-a", "dataset-a", 3, 100).unwrap();
    let b = registry.resolve("org-a", "dataset-b", 3, 100).unwrap();
    let c = registry.resolve("org-b", "dataset-c", 3, 100).unwrap();
    for other in [b, c] {
        assert_ne!(a.namespace(), other.namespace());
        assert_ne!(a.endpoint(), other.endpoint());
        assert_ne!(a.bearer_token(), other.bearer_token());
        assert!(!a.storage_root().starts_with(&other.storage_root()));
        assert!(!other.storage_root().starts_with(&a.storage_root()));
    }
    assert!(matches!(
        registry.resolve("org-a", "dataset-c", 3, 100),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        registry.resolve("org-b", "dataset-a", 3, 100),
        Err(Error::Unavailable)
    ));
}

#[test]
fn stale_expired_missing_and_inactive_bindings_refuse() {
    let (mut doc, credentials) = fixture();
    let registry = parse(&doc, &credentials).unwrap();
    for (id, generation, now) in [
        ("dataset-a", 2, 100),
        ("dataset-a", 3, 200),
        ("absent", 3, 100),
    ] {
        assert!(matches!(
            registry.resolve("org-a", id, generation, now),
            Err(Error::Unavailable)
        ));
    }
    for state in ["provisioning", "draining", "revoked"] {
        doc["bindings"][0]["state"] = json!(state);
        let registry = parse(&doc, &credentials[1..]).unwrap();
        assert!(matches!(
            registry.resolve("org-a", "dataset-a", 3, 100),
            Err(Error::Unavailable)
        ));
    }
}

#[test]
fn resource_overrides_and_duplicate_dataset_ownership_refuse() {
    for key in [
        "endpoint",
        "catalog",
        "data_path",
        "worker_identity",
        "token_file",
        "token",
    ] {
        let (mut doc, credentials) = fixture();
        doc["bindings"][0][key] = json!("attacker-controlled");
        assert!(matches!(parse(&doc, &credentials), Err(Error::Registry)));
    }
    let (mut doc, credentials) = fixture();
    doc["bindings"][2]["dataset_id"] = json!("dataset-a");
    assert!(matches!(parse(&doc, &credentials), Err(Error::Registry)));
}

#[test]
fn credentials_cannot_be_reused_or_swapped_across_datasets() {
    let (doc, mut credentials) = fixture();
    credentials[1]["token"] = credentials[0]["token"].clone();
    assert!(matches!(parse(&doc, &credentials), Err(Error::Credential)));
    let (doc, credentials) = fixture();
    assert!(matches!(
        Registry::parse(&serde_json::to_vec(&doc).unwrap(), |_| Ok(
            serde_json::to_vec(&credentials[1]).unwrap()
        )),
        Err(Error::Credential)
    ));
}

#[test]
fn rotation_requires_matching_projection_and_new_generation() {
    let (mut doc, mut credentials) = fixture();
    doc["bindings"][0]["generation"] = json!(4);
    assert!(matches!(parse(&doc, &credentials), Err(Error::Credential)));
    credentials[0]["generation"] = json!(4);
    credentials[0]["token"] = json!("rotated-abcdefghijklmnopqrstuvwxyz0123456789");
    let registry = parse(&doc, &credentials).unwrap();
    assert!(matches!(
        registry.resolve("org-a", "dataset-a", 3, 100),
        Err(Error::Unavailable)
    ));
    assert_eq!(
        registry
            .resolve("org-a", "dataset-a", 4, 100)
            .unwrap()
            .generation(),
        4
    );
}

#[test]
fn invalid_ids_and_buckets_cannot_escape_their_boundary() {
    for id in ["", "a/b", "a\0b", "..", "a b"] {
        let (mut doc, credentials) = fixture();
        doc["bindings"][0]["dataset_id"] = json!(id);
        assert!(matches!(parse(&doc, &credentials), Err(Error::Registry)));
    }
    for bucket in ["", "a", "bucket/path", "bucket?query", "-bucket", "bucket-"] {
        let (mut doc, credentials) = fixture();
        doc["bucket"] = json!(bucket);
        assert!(matches!(parse(&doc, &credentials), Err(Error::Registry)));
    }
    assert_ne!(namespace("a-b", "c"), namespace("a", "b-c"));
}

#[test]
fn parser_errors_do_not_expose_secret_values() {
    let (doc, _) = fixture();
    let error = Registry::parse(&serde_json::to_vec(&doc).unwrap(), |_| {
        Ok(br#"{"token":"sensitive-value"}"#.to_vec())
    })
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "invalid DuckFleet credential projection");
    assert!(!format!("{error:?}").contains("sensitive-value"));
}

#[test]
fn namespace_matches_terraform_contract() {
    assert_eq!(
        namespace("org-a", "dataset-a"),
        "duckfleet-2c92e260fe9935ecd6664c5a84639cd7b464e280"
    );
}

#[test]
fn artifact_scope_content_identity_and_rotation() {
    use crate::artifacts::{Artifact, Format, Staging};
    let (mut doc, mut credentials) = fixture();
    let registry = parse(&doc, &credentials).unwrap();
    let a = registry.resolve("org-a", "dataset-a", 3, 100).unwrap();
    let b = registry.resolve("org-a", "dataset-b", 3, 100).unwrap();
    let c = registry.resolve("org-b", "dataset-c", 3, 100).unwrap();
    let digest = hex::encode(Sha256::digest(b"accepted"));
    let artifact = Artifact::new(&a, "delivery-1", Format::Csv, 8, &digest).unwrap();
    let uri = artifact.uri(&a).unwrap();
    assert!(uri.starts_with(&a.storage_root()));
    assert!(uri.ends_with(&format!("/artifacts/immutable/delivery-1/{digest}.csv")));
    assert!(artifact.uri(&b).is_err());
    assert!(artifact.uri(&c).is_err());
    let staging = Staging::new(&a, uuid::Uuid::nil(), Format::Parquet);
    assert!(staging.uri(&a).unwrap().contains("/artifacts/staging/"));
    assert!(staging.uri(&b).is_err());
    assert!(staging.uri(&c).is_err());
    doc["bindings"][0]["generation"] = json!(4);
    credentials[0]["generation"] = json!(4);
    credentials[0]["token"] = json!("new-token-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
    let rotated = parse(&doc, &credentials).unwrap();
    let a_new = rotated.resolve("org-a", "dataset-a", 4, 100).unwrap();
    assert_eq!(artifact.uri(&a_new).unwrap(), uri);
    assert!(rotated.resolve("org-a", "dataset-a", 3, 100).is_err());
    assert!(rotated.resolve("org-a", "dataset-a", 4, 201).is_err());
}

#[test]
fn deserialized_artifacts_cannot_inject_paths_or_escape_limits() {
    use crate::artifacts::{Artifact, Format, MAX_ARTIFACT_BYTES};
    let (doc, credentials) = fixture();
    let registry = parse(&doc, &credentials).unwrap();
    let a = registry.resolve("org-a", "dataset-a", 3, 100).unwrap();
    let artifact = Artifact::new(&a, "delivery-1", Format::Csv, 8, &"a".repeat(64)).unwrap();
    let original = serde_json::to_value(artifact).unwrap();
    for (field, bad) in [
        ("version", json!(2)),
        ("org_id", json!("org-b")),
        ("dataset_id", json!("dataset-b")),
        ("identity", json!("../other")),
        ("identity", json!("file.csv?versionId=other")),
        ("sha256", json!("A".repeat(64))),
        ("sha256", json!("a".repeat(63))),
        ("bytes", json!(MAX_ARTIFACT_BYTES + 1)),
    ] {
        let mut value = original.clone();
        value[field] = bad;
        let decoded: Artifact = serde_json::from_value(value).unwrap();
        assert!(decoded.key(&a).is_err(), "accepted invalid {field}");
    }
    let mut value = original;
    value["uri"] = json!("s3://foreign/key");
    assert!(serde_json::from_value::<Artifact>(value).is_err());
}

#[test]
fn recovery_resolves_only_the_pinned_generation_and_never_a_revoked_binding() {
    let (mut doc, credentials) = fixture();
    let registry = parse(&doc, &credentials).unwrap();
    // Active at the pinned generation, even past `valid_until`: evidence reads
    // admit nothing, and an expired token is refused upstream, not guessed at.
    assert!(
        registry
            .resolve_for_recovery("org-a", "dataset-a", 3)
            .is_ok()
    );
    // A replacement generation is a different principal whose owner-scoped
    // lookups would report the attempt's requests as absent.
    assert!(matches!(
        registry.resolve_for_recovery("org-a", "dataset-a", 2),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        registry.resolve_for_recovery("org-b", "dataset-a", 3),
        Err(Error::Unavailable)
    ));
    doc["bindings"][0]["state"] = json!("draining");
    let registry = parse(&doc, &credentials).unwrap();
    assert!(
        registry
            .resolve_for_recovery("org-a", "dataset-a", 3)
            .is_ok()
    );
    assert!(matches!(
        registry.resolve("org-a", "dataset-a", 3, 100),
        Err(Error::Unavailable)
    ));
    // Draining without a projected credential still loads, and cannot recover.
    let registry = parse(&doc, &credentials[1..]).unwrap();
    assert!(matches!(
        registry.resolve_for_recovery("org-a", "dataset-a", 3),
        Err(Error::Unavailable)
    ));
    for state in ["provisioning", "revoked"] {
        doc["bindings"][0]["state"] = json!(state);
        let registry = parse(&doc, &credentials).unwrap();
        assert!(matches!(
            registry.resolve_for_recovery("org-a", "dataset-a", 3),
            Err(Error::Unavailable)
        ));
    }
}

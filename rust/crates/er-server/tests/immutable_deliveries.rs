//! Publication and replay boundaries, independent of network providers.
use std::path::{Path, PathBuf};

use er_server::delivery::{DeliveryPin, publish};

fn fixture() -> (PathBuf, er_config::SourceSpec) {
    let dir = std::env::temp_dir().join(format!("er-delivery-{}", er_ids::new_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let config =
        er_config::load_config_str(include_str!("../../er-cli/tests/fixtures/test.yaml")).unwrap();
    (dir, config.sources["crm"].clone())
}

fn stage(root: &Path, id: &str, csv: &str) {
    std::fs::write(root.join(format!(".crm.{id}.csv.part")), csv).unwrap();
}

#[test]
fn two_published_inputs_and_a_publication_retry_preserve_their_identity() {
    let (root, spec) = fixture();
    let a = er_ids::new_v7();
    let b = er_ids::new_v7();
    stage(&root, &a, "id\nA\n");
    let pin_a = publish(&root, "dataset-a", "crm", &a, &spec).unwrap();
    stage(&root, &b, "id\nB\n");
    let pin_b = publish(&root, "dataset-a", "crm", &b, &spec).unwrap();
    assert_eq!(pin_a.read(&root, "dataset-a", &spec).unwrap(), "id\nA\n");
    assert_eq!(pin_b.read(&root, "dataset-a", &spec).unwrap(), "id\nB\n");
    assert_eq!(
        publish(&root, "dataset-a", "crm", &a, &spec).unwrap(),
        pin_a
    );
    assert!(
        !root.join("crm.csv").exists(),
        "there must be no mutable alias"
    );
    stage(&root, &a, "id\nreplacement\n");
    assert!(publish(&root, "dataset-a", "crm", &a, &spec).is_err());
    assert_eq!(pin_a.read(&root, "dataset-a", &spec).unwrap(), "id\nA\n");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mapping_scope_and_content_changes_are_refused() {
    let (root, spec) = fixture();
    let id = er_ids::new_v7();
    stage(&root, &id, "id\nA\n");
    let pin = publish(&root, "dataset-a", "crm", &id, &spec).unwrap();
    assert!(pin.read(&root, "dataset-b", &spec).is_err());
    let mut changed = spec.clone();
    changed.record_id_column = "different_id".into();
    assert!(pin.read(&root, "dataset-a", &changed).is_err());
    changed = spec.clone();
    changed.columns.swap_indices(0, 1);
    assert!(
        pin.read(&root, "dataset-a", &changed).is_err(),
        "column order affects ingest identity"
    );
    let mut forged = pin.clone();
    forged.source = "billing".into();
    assert!(forged.read(&root, "dataset-a", &spec).is_err());
    std::fs::write(root.join(format!("deliveries/{id}.csv")), "id\nB\n").unwrap();
    assert!(pin.read(&root, "dataset-a", &spec).is_err());
    std::fs::remove_file(root.join(format!("deliveries/{id}.csv"))).unwrap();
    std::fs::write(root.join("crm.csv"), "id\nlatest\n").unwrap();
    assert!(
        pin.read(&root, "dataset-a", &spec).is_err(),
        "missing pin never falls back to latest"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_interrupted_manifest_publication_is_recoverable() {
    let (root, spec) = fixture();
    let id = er_ids::new_v7();
    stage(&root, &id, "id\nA\n");
    let blocked = root.join(format!("deliveries/{id}.json"));
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(publish(&root, "dataset-a", "crm", &id, &spec).is_err());
    assert!(root.join(format!(".crm.{id}.csv.part")).exists());
    assert!(root.join(format!("deliveries/{id}.csv")).exists());
    std::fs::remove_dir(blocked).unwrap();
    let pin = publish(&root, "dataset-a", "crm", &id, &spec).unwrap();
    assert_eq!(pin.read(&root, "dataset-a", &spec).unwrap(), "id\nA\n");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn delivery_ids_and_sources_cannot_be_paths() {
    let (root, spec) = fixture();
    for bad in ["../escape", "/tmp/escape", "a/b", "a\\b", "", ".", ".."] {
        assert!(publish(&root, "dataset-a", "crm", bad, &spec).is_err());
        assert!(publish(&root, "dataset-a", bad, &er_ids::new_v7(), &spec).is_err());
    }
    let value = serde_json::json!({"version": 2, "dataset_id": "dataset-a", "source":"crm",
        "delivery_id":er_ids::new_v7(), "bytes":0, "sha256":"", "source_sha256":""});
    let pin: DeliveryPin = serde_json::from_value(value).unwrap();
    assert!(pin.read(&root, "dataset-a", &spec).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn neither_the_delivery_directory_nor_its_files_can_redirect_to_another_dataset() {
    let (root, spec) = fixture();
    let (other, _) = fixture();
    let id = er_ids::new_v7();
    stage(&root, &id, "id\nA\n");
    std::os::unix::fs::symlink(&other, root.join("deliveries")).unwrap();
    assert!(publish(&root, "dataset-a", "crm", &id, &spec).is_err());
    std::fs::remove_file(root.join("deliveries")).unwrap();
    let pin = publish(&root, "dataset-a", "crm", &id, &spec).unwrap();
    let csv = root.join(format!("deliveries/{id}.csv"));
    std::fs::rename(&csv, other.join("input.csv")).unwrap();
    std::os::unix::fs::symlink(other.join("input.csv"), &csv).unwrap();
    assert!(pin.read(&root, "dataset-a", &spec).is_err());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

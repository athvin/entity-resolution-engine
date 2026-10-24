//! The `train` job — which the dispatcher had no arm for (behind
//! `ER_TEST_PG_DSN`).
//!
//! `train` has been in `SCHEDULABLE_KINDS` since the queue shipped, and every
//! tier-C config publish (one whose `comparisons` changed) enqueues one. The
//! dispatcher had no `"train"` arm, so it fell through to "dispatcher does not
//! handle job kind 'train'" and failed every single time.
//!
//! The failed job was the smaller half of the damage. A tier-C publish enqueues
//! `train` AND a `run_all_full` rebuild, so the rebuild went ahead and
//! re-resolved the entire corpus against the model the publish was meant to
//! replace — a config change that looked applied and was not.
//!
//! What these assertions pin down:
//!   * training produces a registry version, not a file on disk nobody reads;
//!   * it lands as a DRAFT and the active version is untouched, because
//!     training is not consent to re-cluster a tenant's data;
//!   * the trained settings are real — learned m/u, not the template's;
//!   * the TF snapshot is APPENDED, so the active model's frozen term
//!     frequencies still resolve to the same values afterwards. Overwriting
//!     would retune the live model underneath itself, which is the guarantee
//!     that makes an incremental run equal to a full one;
//!   * **a model's own suggestions are not training signal.** `pair_label`
//!     declares an `llm` origin and `read::all` does not filter on it, so a
//!     suggestion would otherwise be fed back as ground truth — the model
//!     trained on what it already believed.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_train

use std::path::PathBuf;

use er_server::{dispatcher, pg};

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const NICKNAMES: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// A standardized base_10 corpus — all training needs. Scoring is not required:
/// `run_train` learns from `int_std_records`.
fn standardize_lake(catalog: &str, data_path: &str) {
    let session = er_cli::open_lake(catalog, data_path).unwrap();
    let config = er_config::load_config_str(TEST_YAML).unwrap();
    let lexicon: Vec<(String, String)> = NICKNAMES
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    er_cli::seed_lexicon(&session, &lexicon).unwrap();
    for source in ["crm", "billing", "webforms"] {
        let csv =
            std::fs::read_to_string(repo_fixture(&format!("base_10/base/{source}.csv"))).unwrap();
        er_cli::run_ingest(
            &session,
            &config,
            source,
            &csv,
            "01TRAINBATCH00000000000000",
            "2026-01-01 00:00:00",
            false,
        )
        .unwrap();
    }
    er_cli::run_standardize(&session, &config, "2026-01-01 00:00:00").unwrap();
}

#[tokio::test]
async fn training_registers_a_draft_and_leaves_the_active_model_alone() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping train e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let tmp = std::env::temp_dir().join(format!("er_train_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    standardize_lake(&catalog, &data_path);

    // The org's config and model/TF inputs, as its env would name them.
    let config_path = tmp.join("config.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    // The frozen-TF file is COPIED, because the assertion below is that
    // training appends to it rather than replacing it — mutating the committed
    // fixture would be a determinism-contract violation and would make the test
    // order-dependent.
    let tf_path = tmp.join("tf.csv");
    std::fs::copy(repo_fixture("model_test_v1.tf.csv"), &tf_path).unwrap();
    let tf_before = std::fs::read_to_string(&tf_path).unwrap();

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            // `pair_labels` is in this list because the phase at the bottom
            // writes some, and without it the labels outlive the run: the
            // natural-key upsert in `create::record` does not save you, because
            // the conflict target is `(org_id, rec_a_key, rec_b_key)` while the
            // PRIMARY KEY is `id` — a fresh org each run means a new natural key
            // and a colliding id, which is a hard pkey error rather than an
            // upsert. `pg_approval.rs` already drops it for the same reason.
            "DROP TABLE IF EXISTS pair_labels, webhook_deliveries, matching_models, \
             staged_steward_actions, audit_log, webhooks, schedules, config_versions, \
             api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", config_path.to_str().unwrap())
        .await
        .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("org exists");

    // An active starter version, exactly as provisioning seeds one.
    let starter = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let seeded = pg::create_model_version(
        &client,
        &org_id,
        pg::DEFAULT_MODEL_NAME,
        "person",
        &starter,
        "v0001",
        Some("01JWMDTFSNAP00000000000001"),
        &serde_json::json!({ "source": "starter" }),
        "test",
    )
    .await
    .unwrap();
    pg::activate_model_version(&client, &org_id, &seeded.id)
        .await
        .unwrap();

    let env = serde_json::json!({
        "ER_LAKE_CATALOG": catalog,
        "ER_LAKE_DATA_PATH": data_path,
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
        "ER_TF_PATH": tf_path.to_string_lossy(),
    });

    // The arm that did not exist. Before this, the job's outcome was
    // "dispatcher does not handle job kind 'train'".
    dispatcher::train_and_register(&client, &org_id, &env, "01TRAINJOB00000000000000000")
        .await
        .expect("training should succeed");

    // A registry version, not a file nobody reads.
    let versions = pg::list_model_versions(&client, &org_id).await.unwrap();
    assert_eq!(versions.len(), 2, "the starter plus the trained one");
    let trained = versions
        .iter()
        .find(|v| v.model_version_label == "trained-01TRAINJOB00000000000000000")
        .expect("the trained version is registered under its job id");

    // A DRAFT. Training is not consent to re-cluster a tenant's corpus, so the
    // operator decides when it goes live — that is the whole point of the
    // registry being explicit about activation.
    assert_eq!(trained.state, "draft", "training must not self-activate");
    assert_eq!(trained.version, 2, "versions are monotonic");
    assert_eq!(trained.created_by, "trainer");
    assert_eq!(trained.provenance["source"], "train");
    // How `m` was estimated, recorded so a draft's origin is legible from the
    // registry. This tenant has no verdicts, so the EM estimate stands and the
    // count is zero — which must be WRITTEN rather than absent: a missing key
    // and "trained without labels" read the same to a steward, and only one of
    // them means the label path ran and found nothing.
    assert_eq!(
        trained.provenance["m_from_labels"], 0,
        "a train with no labels records zero, not nothing: {}",
        trained.provenance
    );
    assert_eq!(
        trained.provenance["job_id"], "01TRAINJOB00000000000000000",
        "a registry row must be traceable to the job that made it"
    );

    // And the active version is still the starter, byte for byte.
    let active = pg::active_model_version(&client, &org_id)
        .await
        .unwrap()
        .expect("something is still active");
    assert_eq!(
        active.id, seeded.id,
        "the active version must not have moved"
    );
    assert_eq!(
        active.model_json, starter,
        "the active model's settings must be untouched"
    );

    // The trained settings are REAL — learned, not a copy of the template.
    let before: serde_json::Value = serde_json::from_str(&starter).unwrap();
    let after: serde_json::Value = serde_json::from_str(&trained.model_json).unwrap();
    assert_ne!(
        before, after,
        "a trained model identical to its template means training did nothing"
    );
    // m/u probabilities are what training learns, so they have to be present
    // and in range; a NaN or an out-of-range value would score every pair the
    // same way and is the failure mode worth catching here.
    let levels: Vec<&serde_json::Value> = after["comparisons"]
        .as_array()
        .expect("comparisons survive training")
        .iter()
        .flat_map(|c| {
            c["comparison_levels"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
        })
        .collect();
    assert!(levels.len() >= 4, "the template's levels survive training");
    let mut learned = 0;
    for level in &levels {
        for key in ["m_probability", "u_probability"] {
            if let Some(p) = level.get(key).and_then(serde_json::Value::as_f64) {
                assert!(
                    p.is_finite() && (0.0..=1.0).contains(&p),
                    "{key} must be a probability, got {p}"
                );
                learned += 1;
            }
        }
    }
    assert!(
        learned > 0,
        "training must write m/u probabilities, or the model is untrained"
    );

    // The TF snapshot is APPENDED. Overwriting would retune the ACTIVE model's
    // frozen term frequencies underneath it — the guarantee that makes an
    // incremental run equal to a full one (see run_match_incremental).
    let tf_after = std::fs::read_to_string(&tf_path).unwrap();
    assert!(
        tf_after.starts_with(tf_before.trim_end_matches('\n')),
        "the existing snapshot's rows must still be there, unchanged and first"
    );
    assert!(
        tf_after.len() > tf_before.len(),
        "the new snapshot's rows must have been added"
    );
    assert!(
        tf_after.contains("tf-01TRAINJOB00000000000000000"),
        "the appended rows carry the new snapshot id, so each model reads only \
         its own: {}",
        &tf_after[..tf_after.len().min(200)]
    );
    // One header, not two — the append must not restate it.
    assert_eq!(
        tf_after
            .lines()
            .filter(|l| l.starts_with("model_version,"))
            .count(),
        1,
        "appending must not duplicate the CSV header"
    );

    // ---- a model's own suggestions are not training signal -------------------
    //
    // Appended as a second train rather than folded into the first, so the
    // assertions above keep measuring a no-label train and stay untouched.
    //
    // `pair_label.origin` declares `llm` for a model's own proposal and nothing
    // writes one yet (V6 will). `read::all` has no origin filter, so without the
    // guard in `train_and_register` a suggestion would be fed straight back in
    // as ground truth — the model trained on what it already believed, with no
    // error and nothing in provenance to show for it.
    //
    // `crm:C001`/`billing:B001` and `crm:C002`/`billing:B002` are genuinely the
    // same people in base_10 (personas P1 and P2), so the two human labels are
    // true and the count below is a real training set rather than a fiction.
    let labels: [(&str, &str, &str, &str); 3] = [
        ("crm:C001", "billing:B001", "match", "human"),
        ("crm:C002", "billing:B002", "match", "human"),
        // The one that must not count.
        ("crm:C003", "billing:B003", "match", "llm"),
    ];
    for (i, (a, b, verdict, origin)) in labels.iter().enumerate() {
        er_entities::pair_label::create::record(
            &client,
            &format!("01JLABEL000000000000000{i:03}"),
            &org_id,
            a,
            b,
            verdict,
            origin,
            &format!("{origin}:test"),
            None,
            None,
            None,
        )
        .await
        .expect("a label is recordable");
    }

    dispatcher::train_and_register(&client, &org_id, &env, "01TRAINJOB00000000000000001")
        .await
        .expect("training with labels should succeed");

    let with_labels = pg::list_model_versions(&client, &org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.model_version_label == "trained-01TRAINJOB00000000000000001")
        .expect("the second trained version is registered");
    assert_eq!(
        with_labels.provenance["m_from_labels"], 2,
        "two human match labels are used and the `llm` one is excluded — training \
         on a model's own suggestion is training on this system's output: {}",
        with_labels.provenance
    );
    // Still a draft, for the same reason the first one is: labels do not make
    // training consent to re-cluster a tenant's corpus either.
    assert_eq!(with_labels.state, "draft");

    let _ = std::fs::remove_dir_all(&tmp);
}

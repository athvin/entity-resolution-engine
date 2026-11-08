//! Account entity-type end-to-end gate (behind `DEDUPE_E2E_DUCKLAKE=1`, like the
//! person `lake_e2e`): `run_all` over a tiny Account corpus with the committed
//! hand-authored `acct_model_v1`, asserting the Account matching + golden
//! assembly produce the expected entities. The person pipeline is untouched
//! (separate fixtures/model, and every pipeline branch is gated on
//! `entity_type == account`). Correctness is asserted behaviourally: records that
//! are the same company co-cluster, distinct companies do not, and
//! `acct_golden_records` assembles one row per entity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use er_config::load_config_str;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn acct_run_all_clusters_accounts_and_assembles_golden() {
    if !enabled() {
        eprintln!("skipping Account DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_cli_acct_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    assert!(
        !config.entity_type.is_person(),
        "fixture is an account config"
    );

    let deliveries: Vec<(String, String)> = ["crm", "vendors"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("acct_base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("acct_model_v1.json")).unwrap();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    let lexicon: Vec<(String, String)> = Vec::new(); // accounts use no nickname lexicon

    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            mode: "full",
            rebuild_reason: None,
            segment: None,
            lexicon: &lexicon,
            ignore_words: &[],
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: Path::new(&tf_csv),
                model_version: "acct_v1",
                tf_snapshot_id: "ACCTTFSNAP0000000000000001",
            },
            run_id: "01ACCTRUNALL0000000000000000",
            ingested_at: "2026-03-01 00:00:00",
            scored_at: "2026-03-02 00:00:00",
            run_at: "2026-03-03 00:00:00",
        },
    )
    .unwrap();

    // Membership: 7 account records → 4 entities (3 merged dup pairs + 1 singleton),
    // written to the SHARED entity-agnostic tables.
    let rows: Vec<(String, String)> = session
        .query_strings(
            "select record_key, entity_id from lake.main.entity_membership order by record_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    assert_eq!(rows.len(), 7, "7 account records land in membership");
    let by_rk: BTreeMap<String, String> =
        rows.iter().map(|(rk, e)| (rk.clone(), e.clone())).collect();
    let entities: BTreeSet<String> = rows.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(
        entities.len(),
        4,
        "4 account entities (Acme/Globex/Initech merged, Umbrella alone)"
    );

    // The three same-company pairs (different sources, name-suffix/domain/phone
    // variations) must co-cluster.
    for (a, b) in [
        ("crm:c1", "vendors:v1"),
        ("crm:c2", "vendors:v2"),
        ("crm:c3", "vendors:v3"),
    ] {
        assert_eq!(
            by_rk.get(a),
            by_rk.get(b),
            "{a} and {b} are the same company and must co-cluster"
        );
    }
    // Umbrella (distinct company) must stay separate + be a singleton.
    assert_ne!(
        by_rk.get("crm:c4"),
        by_rk.get("crm:c1"),
        "Umbrella must not merge with Acme"
    );
    let umbrella = by_rk.get("crm:c4").unwrap();
    assert_eq!(
        rows.iter().filter(|(_, e)| e == umbrella).count(),
        1,
        "Umbrella is a singleton entity"
    );

    // Golden: one assembled row per entity, each with a surviving company_name +
    // domain (proves acct_golden_lineage/acct_golden_records assembled).
    let golden: Vec<(String, String)> = session
        .query_strings(
            "select coalesce(company_name, ''), coalesce(domain, '') \
             from lake.main.acct_golden_records order by id",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    assert_eq!(golden.len(), 4, "one golden row per account entity");
    assert!(
        golden.iter().all(|(c, d)| !c.is_empty() && !d.is_empty()),
        "every account golden row has a surviving company_name and domain"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Partition from `entity_id -> member` rows: the set of clusters, insensitive
/// to the entity ids themselves.
fn partition(pairs: &[(String, String)]) -> BTreeSet<BTreeSet<String>> {
    let mut by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (entity, member) in pairs {
        by.entry(entity.clone()).or_default().insert(member.clone());
    }
    by.into_values().collect()
}

/// A hand-authored scenario's declared ground truth, as a partition over
/// `record_key`s. `truth.csv` is read only by tests — the pipeline never sees
/// it — and it is what makes a cluster-count grid mean anything: without it the
/// grid can only report that the number moved, not which direction is wrong.
fn truth_partition(scenario: &str) -> BTreeSet<BTreeSet<String>> {
    let csv = std::fs::read_to_string(repo_fixture(&format!("{scenario}/truth.csv"))).unwrap();
    let mut by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in csv.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        assert_eq!(
            f.len(),
            3,
            "truth.csv row is persona,source,record_id: {line}"
        );
        by.entry(f[0].to_string())
            .or_default()
            .insert(format!("{}:{}", f[1], f[2]));
    }
    by.into_values().collect()
}

/// What one resolve of `acct_10` produced: the entity partition, and the
/// per-pair `evidence` of every scored pair. The evidence is kept because the
/// central finding here is about two pairs whose evidence is *identical*, and a
/// cluster count alone cannot show that.
struct Resolved {
    clusters: BTreeSet<BTreeSet<String>>,
    evidence: BTreeMap<(String, String), String>,
}

impl Resolved {
    fn of(&self, a: &str, b: &str) -> &str {
        self.evidence
            .get(&(a.to_string(), b.to_string()))
            .map_or_else(|| panic!("no scored pair {a} <-> {b}"), String::as_str)
    }
}

/// Resolve `acct_10` with `cfg` compiled at `bits` (and optionally an overridden
/// `auto_merge`).
fn resolve_acct_10(tag: &str, bits: f64, cfg: &str, auto_merge: Option<f64>) -> Resolved {
    let tmp = std::env::temp_dir().join(format!("er_cli_acctscale_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();

    let mut config = load_config_str(&std::fs::read_to_string(repo_fixture(cfg)).unwrap()).unwrap();
    config.thresholds.full_agreement_bits = Some(bits);
    if let Some(am) = auto_merge {
        config.thresholds.auto_merge = am;
    }
    let compiled = er_compile::compile(&config).expect("the account config compiles at this scale");

    let deliveries: Vec<(String, String)> = ["crm", "vendors", "billing"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("acct_10/base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    let lexicon: Vec<(String, String)> = Vec::new();

    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            mode: "full",
            rebuild_reason: None,
            segment: None,
            lexicon: &lexicon,
            ignore_words: &[],
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &compiled.model_json,
                tf_csv_path: Path::new(&tf_csv),
                model_version: "acct_grid",
                tf_snapshot_id: "ACCTTFSNAP0000000000000001",
            },
            run_id: "01ACCTGRID000000000000000000",
            ingested_at: "2026-03-01 00:00:00",
            scored_at: "2026-03-02 00:00:00",
            run_at: "2026-03-03 00:00:00",
        },
    )
    .unwrap();

    let rows: Vec<(String, String)> = session
        .query_strings(
            "select entity_id, record_key from lake.main.entity_membership order by record_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let evidence: BTreeMap<(String, String), String> = session
        .query_strings(
            "select rec_a_key, rec_b_key, coalesce(evidence,'') \
             from lake.main.match_scores order by rec_a_key, rec_b_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| ((r[0].clone(), r[1].clone()), r[2].clone()))
        .collect();
    let out = Resolved {
        clusters: partition(&rows),
        evidence,
    };
    drop(session);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

const BASE_CFG: &str = "acct_10/config.yaml";
const JW_CFG: &str = "acct_10/config_domain_jw.yaml";
/// The scenario config plus the `variant_match` level (#662).
const VARIANT_CFG: &str = "acct_10/config_variant.yaml";

/// Hooli and Vandelay are two distinct companies that share a switchboard
/// number and a coworking address — the `shared_switchboard` trap.
const HOOLI: [&str; 2] = ["crm:c8", "vendors:v8"];
const VANDELAY: [&str; 2] = ["billing:b9", "crm:c9"];

/// **The finding.** At the shipped `DEFAULT_FULL_AGREEMENT_BITS` the compiled
/// account model fuses Hooli Systems and Vandelay Industries into one account.
///
/// This is the concrete cost of the measurement in
/// `er-compile/tests/score_scale_calibration.rs`: the committed person model
/// measures ~60 bits of full agreement, the committed account model ~21.8, and
/// `compile` hands both the same 60. Scoring an account corpus ~2.7x louder than
/// its own reference model supports is not a safe direction — it buys recall with
/// false merges, which the constant's own doc comment calls the more expensive
/// mistake. Here it is, costing one.
#[test]
fn the_shipped_score_scale_merges_two_distinct_accounts() {
    if !enabled() {
        eprintln!("skipping Account scale (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let truth = truth_partition("acct_10");
    assert_eq!(
        truth.len(),
        10,
        "acct_10 declares 10 accounts over 20 records"
    );

    let default_bits = er_compile::CompileOptions::default().full_agreement_bits;
    let got = resolve_acct_10("default", default_bits, BASE_CFG, None);

    let fused: BTreeSet<String> = HOOLI
        .iter()
        .chain(VANDELAY.iter())
        .map(|s| (*s).to_string())
        .collect();
    assert!(
        got.clusters.contains(&fused),
        "expected the shipped scale to fuse Hooli with Vandelay; got {:?}",
        got.clusters
    );
    assert_ne!(
        got.clusters, truth,
        "if this now equals truth the scale has been recalibrated — update the finding, do not delete it"
    );
}

/// Why the scale alone cannot be fixed: the false merge above and a **true**
/// duplicate carry byte-identical evidence.
///
/// `billing:b4` (ITCH Systems, `intiech.io`) is the same company as
/// `crm:c4`/`vendors:v4` (Initech, `initech.io`) — a d/b/a alias on a
/// transposed domain. Against the four comparisons this model has, that pair and
/// the Hooli/Vandelay pair agree on exactly the same fields (postal and phone)
/// and disagree on exactly the same fields (name and domain). Same gamma vector,
/// same per-field weights, same total.
///
/// So **no value of `full_agreement_bits` and no `auto_merge` threshold can
/// accept one and reject the other.** Any monotone rescaling moves both together.
/// This is why the account gap was never a matter of picking a better constant.
#[test]
fn a_true_duplicate_and_a_false_merge_are_indistinguishable_to_this_model() {
    if !enabled() {
        eprintln!("skipping Account scale (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let got = resolve_acct_10("identical", 44.0, BASE_CFG, None);
    let true_dup = got.of("billing:b4", "crm:c4");
    let false_pair = got.of("crm:c8", "crm:c9");
    assert_eq!(
        true_dup, false_pair,
        "a true duplicate and a false pair must be shown to carry the SAME evidence; \
         if they now differ, the comparison space gained a discriminating feature"
    );
    assert!(
        true_dup.contains("\"gamma_domain_norm\":0"),
        "both score domain as a plain disagreement — the model has no near-match \
         level on domain, which is the missing feature: {true_dup}"
    );
}

/// The remedy, measured: a near-match level on `domain_norm` plus a scale and a
/// threshold that fit the account model resolve `acct_10` exactly.
///
/// `intiech.io` vs `initech.io` is one transposition; `hooli.com` vs
/// `vandelay.com` is unrelated. A `jaro_winkler` level separates them —
/// 12.0 bits against 4.3 — where `exact | else` cannot. The level alone is not
/// enough, because at `auto_merge: 0.95` *both* still clear the bar; and the
/// threshold alone is not enough, because at 60 bits the false pair's
/// probability exceeds even 0.995. All three have to move together, which is
/// the actual answer to "what should the account scale be".
#[test]
fn the_account_corpus_resolves_exactly_at_a_measured_scale_and_threshold() {
    if !enabled() {
        eprintln!("skipping Account scale (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let truth = truth_partition("acct_10");

    let fitted = resolve_acct_10("fitted", 44.0, JW_CFG, Some(0.99));
    assert_eq!(
        fitted.clusters, truth,
        "44 bits + a domain near-match level + auto_merge 0.99 must resolve acct_10 exactly"
    );

    // The scale is load-bearing independently of the threshold: at the shipped
    // 60 bits the false pair clears even a 0.995 auto_merge.
    let default_bits = er_compile::CompileOptions::default().full_agreement_bits;
    let loud = resolve_acct_10("loud_strict", default_bits, JW_CFG, Some(0.995));
    assert_ne!(
        loud.clusters, truth,
        "a stricter threshold alone must NOT be able to rescue the shipped scale"
    );
}

/// The #662 repro, kept green: an account config declaring `variant_match`
/// compiles to the `company_variants` column and the pipeline RUNS — before
/// the fix it died at predict with `Binder Error: ... "name_variants"`.
///
/// `company_variants` is the one-element list of `company_name_norm`, so the
/// level can never fire where `exact` did not already; its value here is that
/// the declared token is no longer fatal, scored with the weight the model
/// assigns it. The config is the measured answer (`config_domain_jw.yaml`)
/// plus the level, and the corpus still resolves exactly at the measured
/// scale — the level redistributes `company_name_norm`'s bits, so this is
/// also the guard against that redistribution quietly costing a persona.
#[test]
fn variant_match_runs_on_an_account_config() {
    if !enabled() {
        eprintln!("skipping Account variant_match (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    // The compiled model carries the level over the ACCOUNT column — the byte
    // string the hand-authored `acct_model_v1.json` pins (quotes escaped: the
    // model is serialized JSON text).
    let cfg = std::fs::read_to_string(repo_fixture(VARIANT_CFG)).unwrap();
    let config = load_config_str(&cfg).unwrap();
    let compiled =
        er_compile::compile(&config).expect("an account config declaring variant_match compiles");
    assert!(
        compiled
            .model_json
            .contains("list_intersect(\\\"company_variants_l\\\", \\\"company_variants_r\\\")"),
        "the level resolves to the account variants column"
    );
    assert!(
        !compiled.model_json.contains("name_variants"),
        "no person column leaks into an account model"
    );

    // And the run that used to die resolves the corpus exactly.
    let got = resolve_acct_10("variant", 44.0, VARIANT_CFG, Some(0.99));
    let truth = truth_partition("acct_10");
    assert_eq!(
        got.clusters, truth,
        "variant_match declared and runnable: acct_10 resolves to truth at the measured scale"
    );
}

/// Resolve the two-record "Acme Pty" vs "Acme" corpus with `ignore_words`
/// seeded as given, returning the standardized `(record_key,
/// company_name_norm)` rows and the entity partition.
fn resolve_ignore_words_pair(
    tag: &str,
    ignore_words: &[(String, String)],
) -> (Vec<(String, String)>, BTreeSet<BTreeSet<String>>) {
    let tmp = std::env::temp_dir().join(format!("er_cli_acctiw_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();

    let mut config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    config.transforms.insert(
        "company_name".to_string(),
        vec![er_config::TransformRule::IgnoreWords {
            dictionary: "legal_suffixes".to_string(),
        }],
    );

    // Identical but for the one suffix token: `domain_exact` blocks the pair
    // together either way, and every other compared field agrees, so
    // `company_name` — the account config's only comparison — decides.
    let csv = "crm_id,company,website,phone,street,city,state,zip,last_modified\n\
               a1,Acme Pty,acme.com,+14155550100,1 Main St,Springfield,IL,62701,2026-01-01\n\
               a2,Acme,acme.com,+14155550100,1 Main St,Springfield,IL,62701,2026-01-01\n";
    let deliveries = vec![("crm".to_string(), csv.to_string())];
    let model_json = std::fs::read_to_string(repo_fixture("acct_model_v1.json")).unwrap();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    let lexicon: Vec<(String, String)> = Vec::new();

    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            mode: "full",
            rebuild_reason: None,
            segment: None,
            lexicon: &lexicon,
            ignore_words,
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: Path::new(&tf_csv),
                model_version: "acct_v1",
                tf_snapshot_id: "ACCTTFSNAP0000000000000001",
            },
            run_id: "01ACCTIGWL000000000000000A",
            ingested_at: "2026-03-01 00:00:00",
            scored_at: "2026-03-02 00:00:00",
            run_at: "2026-03-03 00:00:00",
        },
    )
    .unwrap();

    let norms: Vec<(String, String)> = session
        .query_strings(
            "select record_key, coalesce(company_name_norm, '') \
             from lake.main.acct_int_std_records order by record_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let rows: Vec<(String, String)> = session
        .query_strings(
            "select entity_id, record_key from lake.main.entity_membership order by record_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let clusters = partition(&rows);
    drop(session);
    let _ = std::fs::remove_dir_all(&tmp);
    (norms, clusters)
}

/// #799 golden: an ignore-words dictionary reaches Account comparison. The
/// hard-coded `company_name_norm` suffix list (inc/llc/gmbh/…) already makes
/// the ticket's literal `Acme Inc` / `Acme LLC` pair agree, so the
/// dictionary's visible work has to come from a suffix OUTSIDE that list —
/// here `pty`. "Acme Pty" vs "Acme" standardize to the same
/// `company_name_norm` only when the seeded dictionary carries the token, and
/// because the account comparison set is `company_name: [exact, null]` alone,
/// the transform is the difference between an exact agreement and a
/// disagreement on the one compared field — the pair co-clusters exactly when
/// the dictionary is loaded.
#[test]
fn ignore_words_dictionary_decides_an_account_pair() {
    if !enabled() {
        eprintln!("skipping Account ignore-words e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (norms_with, clusters_with) =
        resolve_ignore_words_pair("with", &[("pty".to_string(), "legal_suffixes".to_string())]);
    let (norms_without, clusters_without) = resolve_ignore_words_pair("without", &[]);

    assert_eq!(
        norms_with,
        vec![
            ("crm:a1".to_string(), "acme".to_string()),
            ("crm:a2".to_string(), "acme".to_string()),
        ],
        "with `pty` in legal_suffixes, both records normalize to `acme`"
    );
    assert_eq!(
        norms_without,
        vec![
            ("crm:a1".to_string(), "acme pty".to_string()),
            ("crm:a2".to_string(), "acme".to_string()),
        ],
        "without the dictionary `pty` survives — it is outside company_name_norm's \
         hard-coded suffix list"
    );
    assert_eq!(
        clusters_with.len(),
        1,
        "dictionary loaded: the pair agrees and merges into one entity"
    );
    assert_eq!(
        clusters_without.len(),
        2,
        "dictionary absent: the sole compared field disagrees and the pair stays split"
    );
}

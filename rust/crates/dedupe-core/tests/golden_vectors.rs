//! Comparison-vector golden gate: concat, TF join, blocked id pairs, and
//! comparison vectors must match the committed per-stage golden
//! materializations exactly on fake_1000.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

struct Fixture {
    session: Session,
    settings: Settings,
    concat: TableHandle,
}

fn setup() -> Fixture {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    Fixture {
        session,
        settings,
        concat,
    }
}

#[test]
fn concat_matches_golden() {
    let f = setup();
    assert_table_matches_parquet(
        &f.session,
        &f.concat,
        "stages/__dedupe__concat.parquet",
        &["unique_id"],
        &["first_name", "surname", "dob", "city", "email", "cluster"],
        &[],
    );
}

#[test]
fn concat_with_tf_matches_golden() {
    let f = setup();
    let tf_tables = predict::materialize_tf_tables(&f.session, &f.settings, &f.concat).unwrap();

    // Materialize the concat_with_tf CTE standalone for comparison.
    let mut p = dedupe_core::sql::pipeline::CtePipeline::new().with_input(&f.concat);
    for t in &tf_tables {
        p.add_input(t);
    }
    p.enqueue(
        dedupe_core::stages::tf::concat_with_tf_sql(
            &f.settings.tf_columns(),
            &f.settings.tf_prefix,
        ),
        dedupe_core::stages::tf::DF_CONCAT_WITH_TF,
    );
    let ctf = f.session.materialize(&p).unwrap();

    assert_table_matches_parquet(
        &f.session,
        &ctf,
        "stages/__dedupe__concat_with_tf.parquet",
        &["unique_id"],
        &["first_name", "surname", "dob", "city", "email", "cluster"],
        &[
            ("tf_first_name", 0.0),
            ("tf_surname", 0.0),
            ("tf_city", 0.0),
            ("tf_email", 0.0),
        ],
    );
}

#[test]
fn blocked_id_pairs_match_golden() {
    let f = setup();
    let blocked = predict::materialize_blocked_pairs(&f.session, &f.settings, &f.concat).unwrap();
    assert_table_matches_parquet(
        &f.session,
        &blocked,
        "stages/__dedupe__blocked_id_pairs.parquet",
        &["join_key_l", "join_key_r"],
        &["match_key"],
        &[],
    );
}

#[test]
fn comparison_vectors_match_golden() {
    let f = setup();
    let tf_tables = predict::materialize_tf_tables(&f.session, &f.settings, &f.concat).unwrap();
    let blocked = predict::materialize_blocked_pairs(&f.session, &f.settings, &f.concat).unwrap();
    let pipeline =
        predict::comparison_vectors_pipeline(&f.settings, &f.concat, &tf_tables, &blocked);
    let cv = f.session.materialize(&pipeline).unwrap();

    assert_table_matches_parquet(
        &f.session,
        &cv,
        "stages/__dedupe__comparison_vectors.parquet",
        &["unique_id_l", "unique_id_r", "match_key"],
        &[
            "gamma_first_name",
            "gamma_surname",
            "gamma_dob",
            "gamma_city",
            "gamma_email",
            "first_name_l",
            "first_name_r",
            "surname_l",
            "surname_r",
            "dob_l",
            "dob_r",
            "city_l",
            "city_r",
            "email_l",
            "email_r",
        ],
        &[
            ("tf_first_name_l", 0.0),
            ("tf_first_name_r", 0.0),
            ("tf_surname_l", 0.0),
            ("tf_surname_r", 0.0),
            ("tf_city_l", 0.0),
            ("tf_city_r", 0.0),
            ("tf_email_l", 0.0),
            ("tf_email_r", 0.0),
        ],
    );
}

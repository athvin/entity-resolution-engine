//! Byte-exact parity of `render_sources_yml` against the committed
//! `dbt/models/sources.yml` (vendored as `tests/dbt_sources_golden.yml`), the same
//! equality Python's `tests/unit/test_dbt_sources_parity.py` asserts. Regenerate
//! the vendored copy with `cp dbt/models/sources.yml
//! rust/crates/er-schema/tests/dbt_sources_golden.yml`.

use er_schema::dbt_sources::render_sources_yml;
use er_schema::model::all_specs;

#[test]
fn matches_committed_sources_yml() {
    let golden = include_str!("dbt_sources_golden.yml");
    let rendered = render_sources_yml(&all_specs());
    assert_eq!(
        rendered, golden,
        "render_sources_yml is not byte-identical to dbt/models/sources.yml"
    );
}

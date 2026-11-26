//! `er-server api-check` (#658): the API launch gate's comparison, file
//! against file, for the production deploy.
//!
//! A CI verb rather than server code, so its report is printed for the person
//! reading the deploy log, as `er-openapi-check`'s is. The detection itself is
//! `openapi::breaking_changes`, shared with the PR-time advisory check, so the
//! gate and the preview can never disagree about what breaks.

/// `er-server api-check <base.json> <candidate.json> [--strict]` (#658): the
/// breaking changes from a deployed contract to a candidate one, both read
/// from files. Exit 0 when compatible (or, without `--strict`, advisory), 2
/// when `--strict` and something breaks, 64 when an input cannot be read or
/// the arguments are wrong. An unreadable input is never "no changes": the
/// launch gate must refuse a release it could not check.
#[must_use]
pub fn run(args: &[String]) -> i32 {
    let strict = args.iter().any(|a| a == "--strict");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [base_path, candidate_path] = files.as_slice() else {
        eprintln!("usage: er-server api-check <base.json> <candidate.json> [--strict]");
        return 64;
    };
    let read = |path: &str| {
        std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {path}: {e}"))
            .and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map(|_| text)
                    .map_err(|e| format!("{path} is not JSON: {e}"))
            })
    };
    let (base, candidate) = match (read(base_path), read(candidate_path)) {
        (Ok(base), Ok(candidate)) => (base, candidate),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("er-server api-check: {e}");
            return 64;
        }
    };
    let breaks = crate::openapi::breaking_changes(&base, &candidate);
    if breaks.is_empty() {
        println!("er-server api-check: no breaking API changes");
        return 0;
    }
    eprintln!(
        "er-server api-check: {} breaking API change(s) from the deployed contract:",
        breaks.len()
    );
    for b in &breaks {
        eprintln!("  - {b}");
    }
    if strict { 2 } else { 0 }
}

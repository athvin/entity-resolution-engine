//! Every node of every kind's DAG has a body to run.
//!
//! `dispatch_coverage.rs` proves each job KIND has a destination. It stops
//! there, and the gap that leaves is one level down: a kind can own a compiled
//! DAG whose individual TASK has no arm in `run_pod_task` / `run_control_task`,
//! so the kind routes, plans, launches — and then the first pod to run that
//! task falls through to the "does not yet run task" refusal and fails the job.
//!
//! That is not hypothetical. `profile`'s DAG opens with a `reconcile_schema`
//! source, and the arm for that task named `lake_maintain`, `lake_migrate` and
//! `train` but not `profile`, while `deploy.yml` has listed `profile` in
//! `DAG_KINDS` throughout — so every profile job failed at its first task, in
//! dev and in production, with a precondition error naming a task the build
//! genuinely did not run. Nothing caught it because nothing compared the DAGs
//! against the dispatch tables.
//!
//! The check matters more once Phase 4 (#645) lands: with the legacy in-process
//! arm deleted there is no second path a kind can fall back to, so "every node
//! has a body" stops being a nicety and becomes the whole contract.
//!
//! ## Why this reads the source
//!
//! The honest check would call `run_pod_task` and assert the error is not the
//! refusal — but every arm that is NOT refused would then execute a real task
//! body against a real lake, which is not something a unit test may do. The
//! routing decision is a `match` on `(kind, task_id)` whose arms are needed for
//! their typed input pins, so it cannot be reduced to data without a second
//! source of truth that could silently drift from the match it describes.
//!
//! So the dispatch tables are parsed, like `dispatch_coverage.rs`'s legacy half,
//! `egress_guard.rs` and `freeform_sql.rs` do for their own structural claims.
//! The parser is exercised by its own unit tests below — a source scanner that
//! matches nothing reports a clean tree, so the parser is the part that must be
//! proven before its verdict means anything.

use er_dag::NodeKind;
use er_server::dags::{self, JobKind};

const DAGS: &str = include_str!("../src/dags/mod.rs");

/// One parsed `match` arm: the kinds it names, and the task id it handles.
type Arm = (Vec<String>, String);

/// The body of a top-level `fn` in `DAGS`, from its signature to the next
/// item at column 0.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} is gone from dags/mod.rs — retarget this gate"));
    let rest = &source[start + signature.len()..];
    // The next item at column 0 ends the function, in any of its spellings.
    // `\nfn ` alone is enough only by accident today — a private `control_pin`
    // happens to sit between `run_control_task` and the `pub fn run_pod_task`
    // that follows it, and it is what stops the control region short of the pod
    // table. Move or rename that helper and the two regions overlap, at which
    // point a pod-only arm would satisfy a control node and the gate would go
    // on reporting a clean tree. `the_two_tables_are_separate` is the assertion
    // that notices; this list is what keeps it from having to.
    let end = ["\nfn ", "\npub fn ", "\npub async fn ", "\nasync fn "]
        .iter()
        .filter_map(|needle| rest.find(needle))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Skip whitespace, then consume `want` if it is there.
fn expect(b: &[u8], i: &mut usize, want: u8) -> bool {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    let ok = *i < b.len() && b[*i] == want;
    if ok {
        *i += 1;
    }
    ok
}

/// Parse `( JobKind::A | JobKind::B , "task" )` patterns out of a `match` body.
///
/// Newlines are whitespace to this, so a rustfmt-exploded multi-line pattern
/// parses identically to a one-line one — which matters, because widening an
/// arm by one kind is exactly what makes rustfmt explode it.
fn arms(body: &str) -> Vec<Arm> {
    let b = body.as_bytes();
    let mut found = Vec::new();
    for open in 0..b.len() {
        if b[open] != b'(' {
            continue;
        }
        let mut i = open + 1;
        let mut kinds = Vec::new();
        // `JobKind::Ident` separated by `|`.
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if !body[i..].starts_with("JobKind::") {
                break;
            }
            i += "JobKind::".len();
            let name_start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            kinds.push(body[name_start..i].to_string());
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && b[i] == b'|' {
                i += 1;
            } else {
                break;
            }
        }
        if kinds.is_empty() {
            continue;
        }
        // `, "task"` then an optional trailing comma and the closing paren.
        if !expect(b, &mut i, b',') || !expect(b, &mut i, b'"') {
            continue;
        }
        let task_start = i;
        while i < b.len() && b[i] != b'"' {
            i += 1;
        }
        if i >= b.len() {
            continue;
        }
        let task = body[task_start..i].to_string();
        i += 1;
        let _ = expect(b, &mut i, b',');
        if expect(b, &mut i, b')') {
            found.push((kinds, task));
        }
    }
    found
}

/// `JobKind`'s variant identifier, which is what the match arms spell.
fn variant(kind: JobKind) -> &'static str {
    match kind {
        JobKind::RunAllFull => "RunAllFull",
        JobKind::RunAllIncremental => "RunAllIncremental",
        JobKind::Correct => "Correct",
        JobKind::Train => "Train",
        JobKind::LakeMaintain => "LakeMaintain",
        JobKind::Profile => "Profile",
        JobKind::LakeMigrate => "LakeMigrate",
        JobKind::Provision => "Provision",
        JobKind::DatasetProvision => "DatasetProvision",
        JobKind::ApplyCorrections => "ApplyCorrections",
        JobKind::ApplyApproved => "ApplyApproved",
        JobKind::Extract => "Extract",
    }
}

#[test]
fn every_dag_node_has_a_body() {
    let pod = arms(function_body(DAGS, "pub fn run_pod_task("));
    let control = arms(function_body(DAGS, "pub async fn run_control_task("));
    assert!(
        pod.len() >= 10 && control.len() >= 5,
        "the dispatch tables parsed as {} pod and {} control arms, which is too few to be \
         the real tables — the parser has stopped matching, not the tables shrunk",
        pod.len(),
        control.len()
    );

    let mut missing = Vec::new();
    for kind in JobKind::ALL {
        let dag = dags::dag_for(kind);
        for node in &dag.nodes {
            let (table, which) = match node.kind {
                NodeKind::Pod => (&pod, "run_pod_task"),
                NodeKind::Control => (&control, "run_control_task"),
            };
            let routed = table.iter().any(|(kinds, task)| {
                task == node.task_id && kinds.iter().any(|k| k == variant(kind))
            });
            if !routed {
                missing.push(format!(
                    "  {} task '{}' of kind '{}' has no arm in {which}",
                    node.kind.as_str(),
                    node.task_id,
                    kind.as_str()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "every node of every kind's DAG needs a body, or the job fails at that task with a \
         precondition refusal:\n{}",
        missing.join("\n")
    );
}

/// The two regions must not overlap. If they did, the gate would still pass —
/// a control node satisfied by a pod arm looks identical to a routed one — so
/// this is checked directly rather than inferred from a green run.
#[test]
fn the_two_tables_are_separate() {
    let pod = arms(function_body(DAGS, "pub fn run_pod_task("));
    let control = arms(function_body(DAGS, "pub async fn run_control_task("));
    let tasks = |table: &[Arm]| -> Vec<String> { table.iter().map(|(_, t)| t.clone()).collect() };
    assert!(
        tasks(&control).iter().all(|t| t != "maintain"),
        "the control region reaches into run_pod_task's arms: {:?}",
        tasks(&control)
    );
    assert!(
        tasks(&pod).iter().all(|t| t != "register_model"),
        "the pod region reaches into run_control_task's arms: {:?}",
        tasks(&pod)
    );
}

#[test]
fn the_parser_reads_both_arm_shapes() {
    // One line, several kinds.
    assert_eq!(
        arms(r#"(JobKind::LakeMaintain | JobKind::Train, "reconcile_schema") => x"#),
        vec![(
            vec!["LakeMaintain".to_string(), "Train".to_string()],
            "reconcile_schema".to_string()
        )]
    );
    // One kind.
    assert_eq!(
        arms(r#"(JobKind::Train, "train") => y"#),
        vec![(vec!["Train".to_string()], "train".to_string())]
    );
    // rustfmt's exploded form, trailing comma and all.
    let exploded = "(\n    JobKind::RunAllFull\n    | JobKind::Correct,\n    \"assemble\",\n) => z";
    assert_eq!(
        arms(exploded),
        vec![(
            vec!["RunAllFull".to_string(), "Correct".to_string()],
            "assemble".to_string()
        )]
    );
    // The fallback arm binds a variable rather than naming kinds, so it is not
    // an arm this gate counts — if it were, every task would look routed.
    assert_eq!(arms(r"(kind, task) => Err(...)"), []);
}

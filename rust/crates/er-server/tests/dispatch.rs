//! Dispatcher mechanics (no infra): claim order, state transitions, and the
//! injected-runner seam that the binary fills with `er_cli::run_all`.

use er_server::{AppState, dispatch_next, enqueue};

#[test]
fn claims_highest_priority_then_fifo_and_records_outcome() {
    let state = AppState::new(String::new());
    // Enqueue in order: low(0), high(5), high(5-again).
    enqueue(&state, "acme", "run", 0);
    let first_high = enqueue(&state, "acme", "run", 5);
    let second_high = enqueue(&state, "acme", "run", 5);

    // Highest priority wins; the earlier of the two priority-5 jobs goes first.
    let job = dispatch_next(&state, |_| Ok(())).unwrap();
    assert_eq!(job.id, first_high.id);
    assert_eq!(job.state, "succeeded");

    // Then the second priority-5 job; a failing runner marks it failed.
    let job = dispatch_next(&state, |_| Err("boom".to_string())).unwrap();
    assert_eq!(job.id, second_high.id);
    assert_eq!(job.state, "failed");

    // Then the priority-0 job.
    let job = dispatch_next(&state, |_| Ok(())).unwrap();
    assert_eq!(job.priority, 0);
    assert_eq!(job.state, "succeeded");

    // Queue drained.
    assert!(dispatch_next(&state, |_| Ok(())).is_none());
}

#[test]
fn runner_sees_the_claimed_job() {
    let state = AppState::new(String::new());
    enqueue(&state, "beta", "provision", 1);
    let seen = std::cell::RefCell::new(None);
    dispatch_next(&state, |job| {
        *seen.borrow_mut() = Some((job.org_id.clone(), job.kind.clone(), job.state.clone()));
        Ok(())
    });
    assert_eq!(
        seen.into_inner(),
        Some((
            "beta".to_string(),
            "provision".to_string(),
            "running".to_string()
        ))
    );
}

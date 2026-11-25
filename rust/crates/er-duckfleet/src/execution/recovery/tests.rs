use super::*;
use crate::execution::CommitState;

const GRACE: Duration = Duration::from_mins(15);

fn evidence() -> Evidence {
    Evidence {
        owner_live: false,
        request_journaled: true,
        commit_journaled: true,
        receipt: Some(Ok(false)),
        commit: Some(Ok(RequestState::Terminal(RequestOutcome::Unknown))),
        quiet_for: GRACE,
        grace: GRACE,
    }
}

#[test]
fn a_receipt_proves_publication_whoever_is_alive_and_whatever_the_service_says() {
    for owner_live in [false, true] {
        for commit in [
            None,
            Some(Err(ReadFailed)),
            Some(Ok(RequestState::Pending)),
            Some(Ok(RequestState::NotFound)),
            Some(Ok(RequestState::Terminal(RequestOutcome::Unknown))),
        ] {
            let decision = decide(&Evidence {
                owner_live,
                receipt: Some(Ok(true)),
                commit,
                quiet_for: Duration::ZERO,
                ..evidence()
            });
            assert_eq!(decision, Decision::Committed);
        }
    }
}

#[test]
fn a_live_owner_is_the_only_settler_without_a_receipt() {
    for commit_journaled in [false, true] {
        let decision = decide(&Evidence {
            owner_live: true,
            commit_journaled,
            receipt: None,
            ..evidence()
        });
        assert_eq!(decision, Decision::Hold(Hold::OwnerLive));
    }
}

#[test]
fn an_unjournaled_commit_was_never_sent_so_nothing_can_publish() {
    // No service evidence is consulted: journal-before-send is the proof, and
    // a non-live owner can no longer journal.
    let base = Evidence {
        commit_journaled: false,
        receipt: None,
        commit: None,
        quiet_for: Duration::ZERO,
        ..evidence()
    };
    assert_eq!(decide(&base), Decision::RolledBack);
    assert_eq!(
        decide(&Evidence {
            request_journaled: false,
            ..base
        }),
        Decision::NotExecuted
    );
}

#[test]
fn a_journaled_commit_without_a_readable_receipt_holds() {
    for receipt in [None, Some(Err(ReadFailed))] {
        assert_eq!(
            decide(&Evidence {
                receipt,
                ..evidence()
            }),
            Decision::Hold(Hold::ReceiptUnavailable)
        );
    }
}

#[test]
fn a_commit_still_executing_or_unreadable_holds() {
    assert_eq!(
        decide(&Evidence {
            commit: Some(Ok(RequestState::Pending)),
            ..evidence()
        }),
        Decision::Hold(Hold::CommitPending)
    );
    for commit in [None, Some(Err(ReadFailed))] {
        assert_eq!(
            decide(&Evidence {
                commit,
                ..evidence()
            }),
            Decision::Hold(Hold::CommitUnavailable)
        );
    }
}

#[test]
fn a_successful_commit_without_a_receipt_is_a_contradiction_not_a_rollback() {
    assert_eq!(
        decide(&Evidence {
            commit: Some(Ok(RequestState::Terminal(RequestOutcome::StatementResult))),
            ..evidence()
        }),
        Decision::Hold(Hold::Contradiction)
    );
}

#[test]
fn a_finished_or_unadmitted_commit_without_a_receipt_rolls_back_only_after_the_grace() {
    for commit in [
        RequestState::NotFound,
        RequestState::Terminal(RequestOutcome::NotExecuted),
        RequestState::Terminal(RequestOutcome::Unknown),
    ] {
        let within = Evidence {
            commit: Some(Ok(commit)),
            quiet_for: Duration::from_secs(899),
            ..evidence()
        };
        assert_eq!(decide(&within), Decision::Hold(Hold::Grace));
        assert_eq!(
            decide(&Evidence {
                quiet_for: GRACE,
                ..within
            }),
            Decision::RolledBack
        );
    }
}

#[test]
fn only_conclusive_decisions_carry_a_release_phase() {
    assert_eq!(Decision::Committed.phase(), Some("committed"));
    assert_eq!(Decision::RolledBack.phase(), Some("rolled_back"));
    assert_eq!(Decision::NotExecuted.phase(), Some("not_executed"));
    assert_eq!(Decision::Hold(Hold::Grace).phase(), None);
}

#[test]
fn the_owner_settles_only_a_returned_or_unsent_commit() {
    assert_eq!(
        decide_owner(CommitState::Returned, true),
        Some(Decision::Committed)
    );
    assert_eq!(
        decide_owner(CommitState::NotSent, true),
        Some(Decision::RolledBack)
    );
    assert_eq!(
        decide_owner(CommitState::NotSent, false),
        Some(Decision::NotExecuted)
    );
    assert_eq!(decide_owner(CommitState::Sent, true), None);
}

#[test]
fn request_records_parse_strictly() {
    let parse = |v: Value| parse_request(&v);
    assert_eq!(
        parse(json!({"state":"accepted"})),
        Some(RequestState::Pending)
    );
    assert_eq!(
        parse(json!({"state":"executing","outcome":"unknown"})),
        Some(RequestState::Pending)
    );
    assert_eq!(
        parse(json!({"state":"terminal","outcome":"statement_result"})),
        Some(RequestState::Terminal(RequestOutcome::StatementResult))
    );
    assert_eq!(
        parse(json!({"state":"terminal","outcome":"not_executed"})),
        Some(RequestState::Terminal(RequestOutcome::NotExecuted))
    );
    assert_eq!(
        parse(json!({"state":"terminal","outcome":"unknown"})),
        Some(RequestState::Terminal(RequestOutcome::Unknown))
    );
    // An unrecognised outcome or state is not evidence of anything.
    assert_eq!(parse(json!({"state":"terminal","outcome":"maybe"})), None);
    assert_eq!(parse(json!({"state":"terminal"})), None);
    assert_eq!(parse(json!({"state":"done"})), None);
}

#[test]
fn the_receipt_names_only_the_attempt() {
    assert_eq!(
        receipt("0190-attempt"),
        r#"{"remote_attempt":"0190-attempt"}"#
    );
}

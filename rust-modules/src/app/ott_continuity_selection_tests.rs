//! Source-domain transition changes completion, not viewer or retry intent.
use super::*;

fn next(id: &str) -> Decision {
    Decision::Start {
        entry_id: id.into(),
        position_ms: 0,
        reason: Reason::Next,
    }
}

#[test]
fn selection_domain_transition_replaces_completion_only_for_current_generation() {
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "A");
    assert_eq!(state.catalog_updated(7, &["A"], true), Decision::Idle);
    state.selection_domain_changed(6, false);
    assert_eq!(state.peek_next(7, "A", &["A"], false), Some("A"));
    state.selection_domain_changed(7, false);
    assert!(
        state.accepts_end(7, "A"),
        "current selection survives the domain change"
    );
    assert_eq!(state.ended(7, "A", &["A"], false), Decision::Wait);
    assert_eq!(state.catalog_updated(7, &["A", "B"], false), next("B"));
    assert_eq!(state.ended(7, "B", &["A", "B"], false), Decision::Wait);
    state.selection_domain_changed(7, true);
    assert_eq!(state.catalog_updated(7, &["A", "B"], false), next("A"));
}

#[test]
fn selection_domain_transition_preserves_stop_pause_and_pending_restore() {
    for paused in [false, true] {
        let mut state = Continuity::new(7, None);
        state.user_selected(7, "A");
        if paused {
            state.pause(7);
        } else {
            state.stop(7);
        }
        state.selection_domain_changed(7, false);
        state.selection_domain_changed(7, true);
        assert!(!state.accepts_end(7, "A"));
        assert_eq!(state.catalog_updated(7, &["A", "B"], true), Decision::Idle);
        assert_eq!(state.tick(7, u64::MAX, &["A", "B"], true), Decision::Idle);
    }
    let mut state = Continuity::new(
        7,
        Some(ResumePoint {
            entry_id: "saved".into(),
            position_ms: 42_000,
        }),
    );
    assert!(matches!(
        state.catalog_updated(7, &["A"], false),
        Decision::Start { .. }
    ));
    state.selection_domain_changed(7, false);
    assert_eq!(state.pending_restore().unwrap().entry_id, "saved");
    assert_eq!(
        state.catalog_updated(7, &["A", "saved"], false),
        Decision::Start {
            entry_id: "saved".into(),
            position_ms: 42_000,
            reason: Reason::Restore,
        }
    );
}

#[test]
fn selection_domain_transition_retains_exact_retry_deadline_failed_rows_and_budget() {
    let ids = &["A", "B", "C", "D", "E"];
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "A");
    assert_eq!(state.start_failed(7, "A", ids, true, 100), next("B"));
    assert_eq!(state.start_failed(7, "B", ids, true, 100), Decision::Wait);
    let before = state.pending_retry().unwrap();
    assert_eq!(
        (
            before.generation,
            before.after,
            before.failures,
            before.not_before_ms
        ),
        (7, "B", 2, 1100)
    );
    state.selection_domain_changed(7, false);
    let after = state.pending_retry().unwrap();
    assert_eq!(
        (
            after.generation,
            after.after,
            after.failures,
            after.not_before_ms
        ),
        (7, "B", 2, 1100)
    );
    assert_eq!(state.tick(7, 1099, ids, false), Decision::Wait);
    assert_eq!(state.tick(7, 1100, ids, false), next("C"));
    assert_eq!(state.start_failed(7, "C", ids, false, 1100), Decision::Wait);
    assert_eq!(state.tick(7, 3099, ids, false), Decision::Wait);
    assert_eq!(state.tick(7, 3100, ids, false), next("D"));
    assert_eq!(state.start_failed(7, "D", ids, false, 3100), Decision::Wait);
    assert_eq!(state.tick(7, 7100, ids, false), next("E"));
    state.selection_domain_changed(7, true);
    assert_eq!(state.start_failed(7, "E", ids, false, 7100), Decision::Idle);
    assert_eq!(
        state.tick(7, u64::MAX, ids, true),
        Decision::Idle,
        "domain transition must not clear failed rows or renew the finite failure budget"
    );
}

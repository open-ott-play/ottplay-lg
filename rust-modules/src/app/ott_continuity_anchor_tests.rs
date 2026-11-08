//! Canonical position never replaces the installed identity's ownership fence.
use super::*;

fn next(id: &str) -> Decision {
    Decision::Start {
        entry_id: id.into(),
        position_ms: 0,
        reason: Reason::Next,
    }
}

#[test]
fn canonical_anchor_keeps_installed_identity_generation_pause_and_stop_fences() {
    let ids = &["A", "B", "C"];
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "favorite:B");
    assert_eq!(state.peek_next_at(6, "favorite:B", "B", ids, false), None);
    assert_eq!(state.peek_next_at(7, "other", "B", ids, false), None);
    assert_eq!(
        state.ended_at(6, "favorite:B", "B", ids, false),
        Decision::Idle
    );
    assert_eq!(state.ended_at(7, "other", "B", ids, false), Decision::Idle);
    assert_eq!(
        state.peek_next_at(7, "favorite:B", "B", ids, false),
        Some("C")
    );
    assert!(
        state.accepts_end(7, "favorite:B"),
        "peek cannot rewrite current identity"
    );
    state.pause(7);
    assert_eq!(
        state.ended_at(7, "favorite:B", "B", ids, false),
        Decision::Idle
    );
    state.resume(7);
    assert_eq!(state.ended_at(7, "favorite:B", "B", ids, false), next("C"));
    state.stop(7);
    assert_eq!(state.peek_next_at(7, "C", "C", ids, true), None);
    assert_eq!(state.ended_at(7, "C", "C", ids, true), Decision::Idle);
    assert_eq!(state.catalog_updated(7, ids, true), Decision::Idle);
}

#[test]
fn canonical_anchor_waits_after_the_folder_row_until_its_successor_arrives() {
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "favorite:B");
    assert_eq!(
        state.ended_at(7, "favorite:B", "B", &["A", "B"], false),
        Decision::Wait
    );
    assert_eq!(state.catalog_updated(7, &["A", "B"], false), Decision::Wait);
    assert_eq!(state.catalog_updated(7, &["A", "B", "C"], false), next("C"));
    assert!(state.accepts_end(7, "C"));
    assert!(!state.accepts_end(7, "favorite:B"));
}

#[test]
fn canonical_anchor_single_row_wrap_emits_folder_id_not_removed_favorite() {
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "favorite:B");
    assert_eq!(
        state.peek_next_at(7, "favorite:B", "B", &["B"], true),
        Some("B")
    );
    assert_eq!(
        state.ended_at(7, "favorite:B", "B", &["B"], true),
        next("B")
    );
    assert!(state.accepts_end(7, "B"));
    assert!(!state.accepts_end(7, "favorite:B"));
}

#[test]
fn canonical_successor_failure_uses_existing_failed_rows_and_retry_budget() {
    let ids = &["A", "B", "C", "D"];
    let mut state = Continuity::new(7, None);
    state.user_selected(7, "favorite:A");
    assert_eq!(state.ended_at(7, "favorite:A", "A", ids, false), next("B"));
    assert_eq!(state.start_failed(7, "B", ids, false, 100), next("C"));
    assert_eq!(state.start_failed(7, "C", ids, false, 100), Decision::Wait);
    let retry = state.pending_retry().unwrap();
    assert_eq!(
        (retry.after, retry.failures, retry.not_before_ms),
        ("C", 2, 1100)
    );
    assert_eq!(state.tick(7, 1099, ids, false), Decision::Wait);
    assert_eq!(state.tick(7, 1100, ids, false), next("D"));
}

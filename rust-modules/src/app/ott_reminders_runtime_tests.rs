use super::*;

fn record(start_s: i64) -> Record {
    Record {
        key: Key {
            source_id: "source-one".into(),
            channel_id: "stable-channel".into(),
            programme_id: format!("programme-{start_s}"),
            start_s,
        },
        end_s: start_s + 600,
        title: "Programme".into(),
    }
}

fn context(revision: u64, active: bool) -> Context<'static> {
    Context {
        source_id: "source-one",
        revision,
        active,
    }
}

fn loaded() -> (State, Saved) {
    let saved = Saved::default();
    let mut state = State::default();
    state.load_committed(&saved, 100_000, 1).unwrap();
    (state, saved)
}

fn committed(start_s: i64) -> (State, Saved) {
    let (mut state, saved) = loaded();
    let (next, token) = state.toggle(&saved, record(start_s), 100_000).unwrap();
    let finished = state.finish_save(token, Ok(&next)).unwrap();
    assert_eq!(finished.outcome, CommitOutcome::Committed);
    (state, next)
}

fn prompt(state: &mut State, now_ms: u64, revision: u64) -> PromptToken {
    state
        .poll(now_ms, context(revision, true), 0, |_| true)
        .unwrap()
        .into_iter()
        .find_map(|effect| match effect {
            Fx::PromptSwitch { token, .. } => Some(token),
            _ => None,
        })
        .unwrap()
}

#[test]
fn proposal_is_not_visible_until_exact_verified_save_and_survives_reload() {
    let (mut state, saved) = loaded();
    let item = record(200);
    let (candidate, token) = state.toggle(&saved, item.clone(), 100_000).unwrap();
    assert!(!state.contains(&item.key));
    assert!(saved.reminders.records().is_empty());
    assert!(state
        .poll(150_000, context(1, true), 1, |_| true)
        .unwrap()
        .is_empty());
    assert_eq!(
        state.finish_save(token, Ok(&candidate)).unwrap().outcome,
        CommitOutcome::Committed
    );
    assert!(state.contains(&item.key));
    state.load_committed(&candidate, 150_000, 2).unwrap();
    assert!(state.contains(&item.key));
}

#[test]
fn refused_or_failed_write_keeps_committed_rows_and_releases_pending_slot() {
    let (mut state, saved) = committed(200);
    let (_, token) = state.toggle(&saved, record(200), 110_000).unwrap();
    assert!(state.contains(&record(200).key));
    assert_eq!(
        state
            .finish_save(token, Err(SaveFailure::Rejected))
            .unwrap()
            .outcome,
        CommitOutcome::Rejected
    );
    assert!(state.contains(&record(200).key));
    assert!(state.toggle(&saved, record(200), 110_000).is_ok());
}

#[test]
fn unknown_save_never_commits_and_withdraws_existing_prompt() {
    let (mut state, saved) = committed(200);
    let old = prompt(&mut state, 200_000, 1);
    let (_, token) = state.toggle(&saved, record(300), 200_000).unwrap();
    let done = state.finish_save(token, Err(SaveFailure::Unknown)).unwrap();
    assert_eq!(done.outcome, CommitOutcome::Unknown);
    assert_eq!(done.effects, vec![Fx::ClosePrompt(old)]);
    assert!(!state.contains(&record(300).key));
    assert!(state.accept(old, 200_000, context(1, true), true).is_none());
    assert!(state.toggle(&saved, record(300), 200_000).is_err());
}

#[test]
fn successful_writer_of_different_document_is_unknown_not_committed() {
    let (mut state, saved) = loaded();
    let (_, token) = state.toggle(&saved, record(200), 100_000).unwrap();
    let done = state.finish_save(token, Ok(&saved)).unwrap();
    assert_eq!(done.outcome, CommitOutcome::Unknown);
    assert!(!state.contains(&record(200).key));
    assert!(state.toggle(&saved, record(200), 100_000).is_err());
}

#[test]
fn stale_completion_cannot_release_a_new_owners_pending_write() {
    let (mut state, saved) = loaded();
    let (_, old) = state.toggle(&saved, record(200), 100_000).unwrap();
    state.load_committed(&saved, 100_000, 2).unwrap();
    let (next, current) = state.toggle(&saved, record(300), 100_000).unwrap();
    assert!(state.finish_save(old, Err(SaveFailure::Rejected)).is_err());
    assert!(state.toggle(&saved, record(400), 100_000).is_err());
    assert_eq!(
        state.finish_save(current, Ok(&next)).unwrap().outcome,
        CommitOutcome::Committed
    );
    assert!(state.contains(&record(300).key));
}

#[test]
fn unrelated_settings_write_does_not_reinitialize_a_delivered_reminder() {
    let (mut state, mut saved) = committed(200);
    let effects = state.poll(150_000, context(1, true), 1, |_| true).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [Fx::LeadNotice { minutes: 1, .. }]
    ));
    saved
        .settings
        .insert("unrelatedSetting".into(), serde_json::json!(true));
    assert!(state
        .poll(151_000, context(1, true), 1, |_| true)
        .unwrap()
        .is_empty());
    // The next normal proposal clones the owner's latest snapshot.
    let (candidate, _) = state.toggle(&saved, record(300), 151_000).unwrap();
    assert_eq!(
        candidate.settings.get("unrelatedSetting"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn stale_saved_snapshot_cannot_overwrite_committed_reminders() {
    let (mut state, _) = committed(200);
    assert!(state
        .toggle(&Saved::default(), record(300), 100_000)
        .is_err());
    assert!(state.contains(&record(200).key));
}

#[test]
fn accepted_intent_requires_current_source_generation_activity_and_calendar() {
    let (mut state, _) = committed(200);
    let token = prompt(&mut state, 200_000, 7);
    assert!(state
        .accept(token, 200_000, context(8, true), true)
        .is_none());
    assert!(state
        .accept(
            token,
            200_000,
            Context {
                source_id: "other",
                ..context(7, true)
            },
            true
        )
        .is_none());
    assert!(state
        .accept(token, 200_000, context(7, false), true)
        .is_none());
    assert!(state
        .accept(token, 199_000, context(7, true), true)
        .is_none());
    assert!(state
        .accept(token, 800_000, context(7, true), true)
        .is_none());
    assert!(state
        .accept(token, 200_000, context(7, true), false)
        .is_none());
    assert_eq!(
        state.accept(token, 200_000, context(7, true), true),
        Some(record(200))
    );
    assert!(state
        .accept(token, 200_000, context(7, true), true)
        .is_none());
}

#[test]
fn source_transition_closes_old_prompt_without_replaying_it_on_return() {
    let (mut state, _) = committed(200);
    let token = prompt(&mut state, 200_000, 1);
    let effects = state.poll(201_000, context(2, true), 1, |_| true).unwrap();
    assert_eq!(effects, vec![Fx::ClosePrompt(token)]);
    assert!(state
        .poll(202_000, context(1, true), 1, |_| true)
        .unwrap()
        .is_empty());
}

#[test]
fn pending_save_blocks_acceptance_and_rejection_restores_same_intent() {
    let (mut state, saved) = committed(200);
    let token = prompt(&mut state, 200_000, 1);
    let (_, save) = state.toggle(&saved, record(300), 200_000).unwrap();
    assert!(state
        .accept(token, 200_000, context(1, true), true)
        .is_none());
    state.finish_save(save, Err(SaveFailure::Rejected)).unwrap();
    assert_eq!(
        state.accept(token, 200_000, context(1, true), true),
        Some(record(200))
    );
}

#[test]
fn dismiss_invalidate_and_reload_do_not_reuse_prompt_authority() {
    let (mut state, saved) = committed(200);
    let token = prompt(&mut state, 200_000, 1);
    assert!(state.dismiss(token));
    assert!(!state.dismiss(token));
    state.invalidate();
    assert!(!state.contains(&record(200).key));
    assert!(state
        .accept(token, 200_000, context(1, true), true)
        .is_none());
    state.load_committed(&saved, 200_000, 2).unwrap();
    assert!(state.contains(&record(200).key));
    assert!(state
        .poll(201_000, context(1, true), 1, |_| true)
        .unwrap()
        .is_empty());
    assert!(state
        .accept(token, 201_000, context(1, true), true)
        .is_none());
}

#[test]
fn invalid_authoritative_load_leaves_projection_unavailable() {
    let (mut state, saved) = committed(200);
    assert!(state.load_committed(&saved, 100_000, 0).is_err());
    assert!(state.toggle(&saved, record(300), 100_000).is_err());
    assert!(!state.contains(&record(200).key));
}

#[test]
fn reusing_a_load_owner_is_rejected_even_after_invalidation() {
    let (mut state, saved) = committed(200);
    let old = prompt(&mut state, 200_000, 1);
    state.invalidate();
    assert!(state.load_committed(&saved, 100_000, 1).is_err());
    assert!(state.accept(old, 200_000, context(1, true), true).is_none());
    state.load_committed(&saved, 100_000, 2).unwrap();
    let current = prompt(&mut state, 200_000, 1);
    assert_ne!(old, current);
    assert!(state.accept(old, 200_000, context(1, true), true).is_none());
}

#[test]
fn policy_conversion_is_bounded_and_does_not_invent_a_timer() {
    let (mut state, _) = committed(86_600);
    assert!(state
        .poll(100_000, context(1, true), u32::MAX, |_| true)
        .unwrap()
        .is_empty());
    assert!(matches!(
        state
            .poll(200_000, context(1, true), u32::MAX, |_| true)
            .unwrap()
            .as_slice(),
        [Fx::LeadNotice { .. }]
    ));
}

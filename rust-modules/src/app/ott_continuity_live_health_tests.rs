use super::*;
use super::super::live_health::{Sample, Tracker};

fn sample(now_ms: u64, position_ms: u64) -> Sample<'static, u64> {
    Sample { generation: 7, play_generation: 12, selection_id: "channel", identity: &19,
        position_ms, now_ms }
}

fn retrying() -> Continuity {
    let mut policy = Continuity::new(7, None);
    policy.user_selected(7, "channel");
    assert_eq!(policy.live_disconnected(7, "channel", 0), Decision::Wait);
    assert_eq!(policy.tick(7, 1_000, &["channel", "neighbor"], true),
        Decision::Start { entry_id: "channel".into(), position_ms: 0, reason: Reason::LiveRetry });
    assert_eq!(policy.consecutive_failures, 1);
    policy
}

#[test]
fn sustained_same_owner_live_progress_renews_budget_without_changing_selection() {
    let mut policy = retrying();
    let mut health = Tracker::default();
    for second in 0..30 {
        assert!(!policy.observe_live_progress(&mut health, Some(sample(1_000 + second * 1_000, second * 1_000))));
    }
    assert!(policy.observe_live_progress(&mut health, Some(sample(31_000, 30_000))));
    assert_eq!(policy.consecutive_failures, 0);
    assert_eq!(policy.active_selection(7), Some("channel"));
    assert!(policy.pending_retry().is_none());
    assert_eq!(policy.live_disconnected(7, "channel", 32_000), Decision::Wait);
    assert_eq!(policy.pending_retry().unwrap().failures, 1);
    assert_eq!(policy.pending_retry().unwrap().not_before_ms, 33_000);
}

#[test]
fn healthy_playback_after_a_long_outage_restores_fast_first_retry() {
    let mut policy = Continuity::new(7, None);
    policy.user_selected(7, "channel");
    let mut now = 0;
    for _ in 0..24 {
        assert_eq!(policy.live_disconnected(7, "channel", now), Decision::Wait);
        now = policy.pending_retry().unwrap().not_before_ms;
        assert!(matches!(policy.tick(7, now, &["channel"], true),
            Decision::Start { reason: Reason::LiveRetry, .. }));
    }
    assert_eq!(policy.consecutive_failures, 24);
    let mut health = Tracker::default();
    for second in 0..30 {
        assert!(!policy.observe_live_progress(&mut health,
            Some(sample(now + second * 1_000, second * 1_000))));
    }
    assert!(policy.observe_live_progress(&mut health, Some(sample(now + 30_000, 30_000))));
    assert_eq!(policy.live_disconnected(7, "channel", now + 31_000), Decision::Wait);
    let pending = policy.pending_retry().unwrap();
    assert_eq!(pending.failures, 1);
    assert_eq!(pending.not_before_ms, now + 32_000);
}

#[test]
fn short_playing_bursts_keep_the_capped_backoff_instead_of_resetting_it() {
    let mut policy = Continuity::new(7, None);
    policy.user_selected(7, "channel");
    let mut health = Tracker::default();
    let mut now = 0;
    for attempt in 0..24 {
        assert_eq!(policy.live_disconnected(7, "channel", now), Decision::Wait);
        let pending = policy.pending_retry().unwrap();
        assert_eq!(pending.failures, attempt + 1);
        assert_eq!(pending.not_before_ms - now,
            [1_000, 2_000, 4_000, 8_000, 15_000, 30_000][attempt.min(5)]);
        now = pending.not_before_ms;
        assert!(matches!(policy.tick(7, now, &["channel", "neighbor"], true),
            Decision::Start { entry_id, reason: Reason::LiveRetry, .. } if entry_id == "channel"));
        for second in 0..=3 {
            assert!(!policy.observe_live_progress(&mut health,
                Some(sample(now + second * 1_000, second * 1_000))));
        }
        now += 4_000;
    }
    assert_eq!(policy.live_disconnected(7, "channel", now), Decision::Wait);
    assert_eq!(policy.consecutive_failures, 25);
    assert_eq!(policy.pending_retry().unwrap().not_before_ms - now, 30_000);
}

#[test]
fn pending_and_withdrawn_retries_cannot_be_cleared_by_position_samples() {
    for exhausted in [false, true] {
        let mut policy = Continuity::new(7, None);
        policy.user_selected(7, "channel");
        assert_eq!(policy.live_disconnected(7, "channel", 0), Decision::Wait);
        if exhausted { policy.failed_continuation = FailedContinuation::Halted; }
        let mut health = Tracker::default();
        for second in 0..=60 {
            assert!(!policy.observe_live_progress(&mut health,
                Some(sample(second * 1_000, second * 1_000))));
        }
        assert_eq!(policy.consecutive_failures, 1);
        assert_eq!(policy.failed_continuation, if exhausted { FailedContinuation::Halted }
            else { FailedContinuation::SameCooldown { not_before_ms: 1_000 } });
    }
}

#[test]
fn pause_suspend_stop_and_source_changes_withdraw_the_healthy_prefix() {
    for boundary in 0..5 {
        let mut policy = retrying();
        let mut health = Tracker::default();
        for second in 0..=29 {
            assert!(!policy.observe_live_progress(&mut health,
                Some(sample(second * 1_000, second * 1_000))));
        }
        match boundary {
            0 => { policy.pause(7); policy.resume(7); }
            1 => { policy.suspend(7); policy.resume_after_suspend(7); }
            2 => { policy.stop(7); policy.retain_selection(7, "channel"); }
            3 => { policy.rebase(8); }
            _ => { policy.retain_selection(7, "neighbor"); policy.retain_selection(7, "channel"); }
        }
        assert!(!policy.observe_live_progress(&mut health, Some(sample(30_000, 30_000))));
        assert_eq!(policy.consecutive_failures, 1, "boundary {boundary} must not renew the budget");
    }
}

#[test]
fn a_new_failure_invalidates_healthy_prefix_even_if_the_runtime_reuses_a_decoder_id() {
    let mut policy = retrying();
    let mut health = Tracker::default();
    for second in 0..=29 {
        assert!(!policy.observe_live_progress(&mut health,
            Some(sample(second * 1_000, second * 1_000))));
    }
    assert_eq!(policy.live_disconnected(7, "channel", 29_000), Decision::Wait);
    let due = policy.pending_retry().unwrap().not_before_ms;
    assert!(matches!(policy.tick(7, due, &["channel"], true),
        Decision::Start { reason: Reason::LiveRetry, .. }));
    assert!(!policy.observe_live_progress(&mut health, Some(sample(due, due))));
    assert_eq!(policy.consecutive_failures, 2);
}

#[test]
fn epoch_exhaustion_fails_closed_without_wrapping_or_restoring_budget() {
    let mut policy = retrying();
    policy.live_health_epoch = u64::MAX - 1;
    policy.suspend(7);
    policy.resume_after_suspend(7);
    let mut health = Tracker::default();
    for second in 0..=60 {
        assert!(!policy.observe_live_progress(&mut health,
            Some(sample(second * 1_000, second * 1_000))));
    }
    assert_eq!(policy.consecutive_failures, 1);
    assert_eq!(policy.live_health_epoch, u64::MAX);
}

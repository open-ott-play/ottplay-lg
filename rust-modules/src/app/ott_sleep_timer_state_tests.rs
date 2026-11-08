use super::*;

fn sample(now_ms: u64) -> Sample<'static> {
    Sample { now_ms, choice: Choice::Minutes30, revision: Some("one"), activity: 1,
        permitted: true, defer_expiry: false }
}

#[test]
fn classic_choices_are_bounded_and_default_to_off() {
    assert_eq!(Choice::default(), Choice::Off);
    for (index, minutes) in [0, 30, 60, 120, 180].into_iter().enumerate() {
        let choice = Choice::from_index(index as u64).unwrap();
        assert_eq!(choice.index(), index as u8);
        assert_eq!(choice.duration_ms(), (minutes != 0).then_some(minutes * 60_000));
    }
    assert!(Choice::from_index(5).is_none());
    assert!(Choice::from_index(u64::MAX).is_none());
}

#[test]
fn unchanged_render_or_poll_samples_do_not_extend_deadline_and_expiry_is_once() {
    let mut timer = State::default();
    assert_eq!(timer.step(sample(100)), None);
    assert_eq!(timer.step(sample(1_800_099)), None);
    assert_eq!(timer.step(sample(1_800_100)), Some(Fx::EnterStandby));
    assert_eq!(timer.step(sample(1_800_101)), None);
    assert_eq!(timer.step(sample(9_000_000)), None);
}

#[test]
fn real_input_on_the_deadline_rearms_without_using_its_wrapping_clock_as_time() {
    let mut timer = State::default();
    let mut input = sample(100);
    input.activity = u32::MAX;
    timer.step(input);
    let mut input = sample(1_800_100);
    input.activity = 0;
    assert_eq!(timer.step(input), None);
    input.now_ms += 1_800_000;
    assert_eq!(timer.step(input), Some(Fx::EnterStandby));
}

#[test]
fn off_and_policy_revocation_retire_the_old_deadline() {
    for off in [false, true] {
        let mut timer = State::default();
        timer.step(sample(100));
        let mut input = sample(1_800_100);
        if off { input.choice = Choice::Off; } else { input.permitted = false; }
        assert_eq!(timer.step(input), None);
        input.now_ms = 8_000_000;
        assert_eq!(timer.step(input), None);
        input.choice = Choice::Minutes30;
        input.permitted = true;
        assert_eq!(timer.step(input), None);
        input.now_ms += 1_800_000;
        assert_eq!(timer.step(input), Some(Fx::EnterStandby));
    }
}

#[test]
fn changed_duration_or_acknowledged_same_value_replaces_deadline() {
    for same_value in [false, true] {
        let mut timer = State::default();
        timer.step(sample(0));
        let mut input = sample(1_800_000);
        input.revision = Some("two");
        if !same_value { input.choice = Choice::Minutes60; }
        assert_eq!(timer.step(input), None);
        input.now_ms += input.choice.duration_ms().unwrap();
        assert_eq!(timer.step(input), Some(Fx::EnterStandby));
    }
}

#[test]
fn pending_save_defers_expiry_but_failure_does_not_renew_old_choice() {
    let mut timer = State::default();
    timer.step(sample(0));
    let mut input = sample(1_800_000);
    input.defer_expiry = true;
    assert_eq!(timer.step(input), None);
    input.now_ms += 10;
    input.defer_expiry = false;
    assert_eq!(timer.step(input), Some(Fx::EnterStandby));
}

#[test]
fn pending_disable_that_commits_cannot_fire_the_retired_deadline() {
    let mut timer = State::default();
    timer.step(sample(0));
    let mut input = sample(1_800_000);
    input.defer_expiry = true;
    assert_eq!(timer.step(input), None);
    input.defer_expiry = false;
    input.choice = Choice::Off;
    input.revision = Some("two");
    assert_eq!(timer.step(input), None);
}

#[test]
fn backward_time_rearms_and_overflow_never_becomes_immediate_expiry() {
    let mut timer = State::default();
    timer.step(sample(5_000));
    assert_eq!(timer.step(sample(1_000)), None);
    assert_eq!(timer.step(sample(1_801_000)), Some(Fx::EnterStandby));
    let mut timer = State::default();
    assert_eq!(timer.step(sample(u64::MAX - 10)), None);
    assert_eq!(timer.step(sample(u64::MAX)), None);
    assert_eq!(timer.step(sample(0)), None);
    assert_eq!(timer.step(sample(1_800_000)), Some(Fx::EnterStandby));
}

#[test]
fn fresh_runtime_arms_full_saved_interval_without_a_persisted_deadline() {
    let mut first = State::default();
    first.step(sample(0));
    assert_eq!(first.step(sample(1_800_000)), Some(Fx::EnterStandby));
    let mut restarted = State::default();
    assert_eq!(restarted.step(sample(9_000_000)), None);
    assert_eq!(restarted.step(sample(10_800_000)), Some(Fx::EnterStandby));
}

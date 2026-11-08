//! Rejected playback requests must not consume the previous item's finish mark.
//! Only the accepted case starts a resolver; its server slot is unregistered,
//! so it exits before sockets/decoding. Drain that worker before any assertion.
use super::*;
use super::test_support::{fresh_registry, unregistered_sid};

#[test]
fn empty_request_preserves_outgoing_finished_mark() {
    let mut ps = PlaybackSession::IDLE;
    let _guard = fresh_registry(&mut ps);
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    ps.finished = true;
    let ticket = play_request_ticket();
    let accepted = request_play(&mut ps, &mut meta, plx_platform::devcaps::assumed_shared(),
        unregistered_sid(), "", "", "h264", "aac", "Invalid", "", None);
    let finished = ps.finished;
    let published = ps.publication().finished;
    let ticket_unchanged = play_request_is_current(ticket);
    reset_player_control_for_test(&ps);
    reset_session(&mut ps);
    assert!(!accepted, "empty source must be rejected");
    assert!(finished && published, "an empty request consumed the outgoing finish mark");
    assert!(ticket_unchanged, "an empty request must not acquire playback authority");
}

#[test]
fn busy_request_preserves_outgoing_finished_mark() {
    let mut ps = PlaybackSession::IDLE;
    let _guard = fresh_registry(&mut ps);
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    ps.finished = true;
    force_applying_for_test(42);
    let ticket = play_request_ticket();
    let accepted = request_play(&mut ps, &mut meta, plx_platform::devcaps::assumed_shared(),
        unregistered_sid(), "replacement", "/library/parts/2/file.mkv", "h264", "aac", "Busy", "", None);
    let finished = ps.finished;
    let published = ps.publication().finished;
    let ticket_unchanged = play_request_is_current(ticket);
    reset_player_control_for_test(&ps);
    reset_session(&mut ps);
    assert!(!accepted, "an applying route must reject the overlapping request");
    assert!(finished && published, "a Busy request consumed the outgoing finish mark");
    assert!(ticket_unchanged, "a Busy request must not acquire playback authority");
}

#[test]
fn admitted_request_clears_the_previous_finished_mark() {
    let mut ps = PlaybackSession::IDLE;
    let _guard = fresh_registry(&mut ps);
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    ps.finished = true;
    let ticket = play_request_ticket();
    let accepted = request_play(&mut ps, &mut meta, plx_platform::devcaps::assumed_shared(),
        unregistered_sid(), "replacement", "/library/parts/2/file.mkv", "h264", "aac", "Replacement", "", None);
    let finished = ps.finished;
    let published = ps.publication().finished;
    let new_authority = !play_request_is_current(ticket);
    cancel_play(&mut ps);
    let drained = wait_resolve_workers_for_test(std::time::Duration::from_secs(10));
    reset_player_control_for_test(&ps);
    reset_session(&mut ps);
    assert!(drained, "resolver must be gone before another test reuses the registry");
    assert!(accepted, "the valid idle request should be admitted");
    assert!(!finished && !published, "the replacement must not inherit a previous finish mark");
    assert!(new_authority);
}

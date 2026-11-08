//! Protocol fixtures: outcomes are injected, not claims that an inactive route can admit a write.
use super::*;
use crate::player::overlay_tests::TestHost;
use plx_machine::machine::{EntryId, InputOwner, Tick};
use plx_ui::fixture::FixtureMeasure;

fn cx() -> Cx<'static, TestHost> {
    Cx { views: (), tick: Tick::default(), measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(EntryId(44)) }
}
fn request(page: &mut PlayerOverlayScreen, keep_open: bool) -> TrackCommitRequest {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    page.request_track_commit::<TestHost>(TrackCommit::TextAppearance(Default::default()), keep_open,
        &mut Effects::new(&mut out, MachineId::Instance(plx_machine::machine::InstanceId(7)), &mut present));
    out.into_iter().find_map(|item| match item.fx {
        Fx::App(AppFx::Player(PlayerReq::CommitTrackAddressed(request))) => Some(request), _ => None,
    }).expect("one addressed request")
}
fn reply(page: &mut PlayerOverlayScreen, id: RequestId, outcome: TrackCommitOutcome) -> bool {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    page.track_commit_reply::<TestHost>(id, outcome, &plx_media::route::PlaybackSession::IDLE, &cx(),
        &mut Effects::new(&mut out, MachineId::Instance(plx_machine::machine::InstanceId(7)), &mut present));
    out.iter().any(|item| matches!(item.fx, Fx::Nav(NavOp::Dismiss(_))))
}
fn menu() -> PlayerOverlayScreen {
    PlayerOverlayScreen::new(&plx_media::route::PlaybackSession::IDLE,
        plx_data::stores::metadata::MetadataStore::default().view(), EntryId(44), OverlayKind::Tracks { tab: 1 })
}
#[test]
fn matching_owner_reply_is_required_to_close_a_track_choice() {
    let _lock = plx_base::testlock::serial();
    let mut page = menu();
    let req = request(&mut page, false);
    assert!(page.accepts_track_commit(&req));
    assert!(!reply(&mut page, RequestId(req.reply.req.0 + 1), TrackCommitOutcome::Admitted));
    assert!(page.accepts_track_commit(&req));
    assert!(reply(&mut page, req.reply.req, TrackCommitOutcome::Admitted));
    assert!(!page.accepts_track_commit(&req));
    assert!(!reply(&mut page, req.reply.req, TrackCommitOutcome::Rejected(
        plx_appkit::track_menu::TrackCommitFailure::Busy)));
}
#[test]
fn late_reply_after_source_replacement_dismisses_instead_of_freezing_old_menu() {
    let _lock = plx_base::testlock::serial();
    let mut page = menu();
    page.track_source = TrackCommitSource::External(plx_media::route::ExternalTracks::new(
        Default::default(), vec![], vec![], None).unwrap());
    let req = request(&mut page, true);
    assert!(reply(&mut page, req.reply.req, TrackCommitOutcome::Saved));
    assert!(page.track_closed);
    assert!(!page.accepts_track_commit(&req));
}
#[test]
fn retired_instance_ignores_late_result_and_new_instance_has_no_pending_request() {
    let _lock = plx_base::testlock::serial();
    let mut old = menu();
    let req = request(&mut old, true);
    old.retire_track_commit();
    assert!(!reply(&mut old, req.reply.req, TrackCommitOutcome::Saved));
    let mut reopened = menu();
    assert!(!reopened.accepts_track_commit(&req));
    assert!(!reply(&mut reopened, req.reply.req, TrackCommitOutcome::Saved));
}
#[test]
fn same_identity_rejected_during_pending_restart_remains_visible_without_retry() {
    let _lock = plx_base::testlock::serial();
    let mut page = menu();
    let req = request(&mut page, true);
    assert!(!reply(&mut page, req.reply.req, TrackCommitOutcome::Rejected(
        plx_appkit::track_menu::TrackCommitFailure::StalePlayback)));
    let Panel::Tracks(panel) = &page.panel else { panic!("track panel") };
    assert_eq!(panel.page_path(), vec![plx_appkit::track_menu::TrackPage::CommitFailure]);
    assert!(!panel.ids().contains(&plx_appkit::track_menu::TrackRow::CommitRetry));
}

#[test]
fn pending_and_retired_requests_have_distinct_logical_state() {
    let _lock = plx_base::testlock::serial();
    let mut page = menu();
    let mut before = Canon::new(); page.write(&mut before);
    let _ = request(&mut page, true);
    let mut pending = Canon::new(); page.write(&mut pending);
    page.retire_track_commit();
    let mut retired = Canon::new(); page.write(&mut retired);
    assert_ne!(before.finish(), pending.finish());
    assert_ne!(pending.finish(), retired.finish());
}

#[test]
fn temporary_deeper_event_preserves_pending_but_permanent_leave_retires_it() {
    let _lock = plx_base::testlock::serial();
    let mut page = menu();
    let req = request(&mut page, true);
    let mut out = Vec::new(); let mut present = plx_machine::present::Present::new();
    for (leave, retained) in [(plx_machine::machine::Leave::Deeper, true), (plx_machine::machine::Leave::ForGood, false)] {
        page.step(&ScreenEvent::WillLeave(leave), &cx(),
            &mut Effects::new(&mut out, req.reply.to, &mut present));
        assert_eq!(page.accepts_track_commit(&req), retained);
    }
    let mut info = PlayerOverlayScreen::new(&plx_media::route::PlaybackSession::IDLE,
        plx_data::stores::metadata::MetadataStore::default().view(), EntryId(44), OverlayKind::Info);
    info.step(&ScreenEvent::Closing, &cx(), &mut Effects::new(&mut out, req.reply.to, &mut present));
    assert!(!info.track_closed, "track retirement must not change unrelated panels");
}

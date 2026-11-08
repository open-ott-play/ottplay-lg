//! Real held-Bridge/Dispatcher address lifecycle, without playback/network admission.
use super::*;
use super::super::test_support::{frame, tick};
use plx_screens::registry::PlayerReq;
use plx_machine::machine::{Addr, RequestId};

fn emit(rig: &mut Bridge, from: MachineId, value: AppFx) {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    rig.app_effect(from, value, &mut Effects::new(&mut out, from, &mut present));
}

fn open_and_request(d: &mut Dispatcher<AppHost>, rig: &mut Bridge) -> TrackCommitRequest {
    frame(d, rig, AppArg::Player, tick(0), vec![]);
    super::super::open_player_overlay(plx_media::route::idle_session_for_test(),
        plx_data::stores::metadata::MetadataStore::default().view(), d,
        plx_screens::player::overlay::OverlayKind::Timing);
    super::super::frame(d, rig, tick(1), vec![]);
    let instance = d.nav.modals.surfaces.last().unwrap().entry.inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance), Delivery::Screen(ScreenEvent::Input(InputEvent {
        kind: InputKind::Key { key: Key::Right, sym: plx_ui::consts::SDLK_RIGHT, wcode: 0, edge: Edge::Down, at_edge: false },
        at: tick(2), source: Source::Sdl,
    }))));
    super::super::frame(d, rig, tick(2), vec![]);
    rig.take_player_reqs().into_iter().find_map(|r| match r {
        PlayerReq::CommitTrackAddressed(r) => Some(r), _ => None,
    }).expect("actual overlay emitted addressed request")
}
#[test]
fn bridge_preserves_origin_and_refuses_forged_or_non_instance_envelopes() {
    let _guard = plx_base::testlock::serial();
    let mut rig = Bridge::for_test(|| 0);
    let request = TrackCommitRequest {
        reply: Addr { to: MachineId::Instance(InstanceId(70)), req: RequestId(1) },
        source: plx_appkit::track_menu::TrackCommitSource::Inactive,
        commit: plx_appkit::track_menu::TrackCommit::SubtitleOffset(100),
    };
    emit(&mut rig, MachineId::Instance(InstanceId(71)), AppFx::Player(PlayerReq::CommitTrackAddressed(request.clone())));
    emit(&mut rig, MachineId::Player, AppFx::Player(PlayerReq::CommitTrackAddressed(request.clone())));
    assert!(rig.take_player_reqs().is_empty());
    emit(&mut rig, request.reply.to, AppFx::Player(PlayerReq::CommitTrackAddressed(request.clone())));
    assert_eq!(rig.take_player_reqs(), vec![PlayerReq::CommitTrackAddressed(request)]);
}
#[test]
fn stale_live_request_registers_once_and_duplicate_cannot_cancel_its_reply() {
    let _guard = plx_base::testlock::serial();
    let mut d = Dispatcher::<AppHost>::new();
    let mut rig = Bridge::for_test(|| 0);
    let request = open_and_request(&mut d, &mut rig);
    assert_eq!(begin_track_commit(&mut d, plx_media::route::idle_session_for_test(), &request),
        TrackCommitAdmission::Reject(TrackCommitFailure::StalePlayback));
    assert!(d.nav.is_deliverable(&request.reply));
    assert_eq!(begin_track_commit(&mut d, plx_media::route::idle_session_for_test(), &request), TrackCommitAdmission::Ignore);
    assert!(d.nav.is_deliverable(&request.reply));
    finish_track_commit(&mut d, &request, TrackCommitOutcome::Rejected(TrackCommitFailure::Busy));
    assert!(!d.nav.is_deliverable(&request.reply));
    finish_track_commit(&mut d, &request, TrackCommitOutcome::Saved); // Duplicate success must not override rejection.
    super::super::frame(&mut d, &mut rig, tick(3), vec![]);
    assert!(!exact_track_overlay(&d, &request), "real Async delivery closed the rejected capsule after inflight removal");
}
#[test]
fn a_queued_request_from_retired_overlay_never_reaches_source_classification() {
    let _guard = plx_base::testlock::serial();
    let mut d = Dispatcher::<AppHost>::new();
    let mut rig = Bridge::for_test(|| 0);
    let request = open_and_request(&mut d, &mut rig);
    super::super::dismiss_player_overlays(&mut d);
    super::super::frame(&mut d, &mut rig, tick(3), vec![]);
    assert_eq!(begin_track_commit(&mut d, plx_media::route::idle_session_for_test(), &request), TrackCommitAdmission::Ignore);
    assert!(!d.nav.is_deliverable(&request.reply));
}

fn cover(d: &mut Dispatcher<AppHost>, rig: &mut Bridge) {
    d.emit(MachineId::Nav, Fx::Nav(NavOp::Push(AppArg::Search)));
    super::super::frame(d, rig, tick(3), vec![]);
    assert!(!d.nav.covered_modals.is_empty(), "actual page push retained the overlay");
}
fn uncover(d: &mut Dispatcher<AppHost>, rig: &mut Bridge) {
    d.emit(MachineId::Nav, Fx::Nav(NavOp::Pop));
    super::super::frame(d, rig, tick(5), vec![]);
}
#[test]
fn registered_result_delivers_while_covered_and_uncovered_instance_accepts_next_edit() {
    let _guard = plx_base::testlock::serial();
    let mut d = Dispatcher::<AppHost>::new(); let mut rig = Bridge::for_test(|| 0);
    let request = open_and_request(&mut d, &mut rig);
    let MachineId::Instance(instance) = request.reply.to else { panic!("instance") };
    // Controlled already-admitted registration: this fixture isolates delivery, not route policy.
    d.track_inflight(instance, request.reply.req);
    cover(&mut d, &mut rig);
    assert!(!exact_track_overlay(&d, &request));
    assert!(exact_track_overlay_in(&d, &request, true));
    assert_eq!(begin_track_commit(&mut d, plx_media::route::idle_session_for_test(), &request), TrackCommitAdmission::Ignore);
    finish_track_commit(&mut d, &request, TrackCommitOutcome::Admitted);
    super::super::frame(&mut d, &mut rig, tick(4), vec![]);
    assert!(!d.nav.is_deliverable(&request.reply));
    uncover(&mut d, &mut rig);
    assert!(d.nav.modals.surfaces.iter().any(|s| s.entry.inst.as_ref().is_some_and(|i| i.id == instance)));
    d.emit(MachineId::Nav, Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::Input(InputEvent {
        kind: InputKind::Key { key: Key::Right, sym: plx_ui::consts::SDLK_RIGHT, wcode: 0, edge: Edge::Down, at_edge: false },
        at: tick(6), source: Source::Sdl,
    }))));
    super::super::frame(&mut d, &mut rig, tick(6), vec![]);
    assert!(rig.take_player_reqs().into_iter().any(|r| matches!(r, PlayerReq::CommitTrackAddressed(next)
        if next.reply.to == request.reply.to && next.reply.req != request.reply.req)));
}
#[test]
fn queued_request_drained_after_cover_gets_rejection_instead_of_becoming_stuck() {
    let _guard = plx_base::testlock::serial();
    let mut d = Dispatcher::<AppHost>::new(); let mut rig = Bridge::for_test(|| 0);
    let request = open_and_request(&mut d, &mut rig);
    cover(&mut d, &mut rig);
    assert_eq!(begin_track_commit(&mut d, plx_media::route::idle_session_for_test(), &request),
        TrackCommitAdmission::Reject(TrackCommitFailure::StalePlayback));
    finish_track_commit(&mut d, &request, TrackCommitOutcome::Rejected(TrackCommitFailure::StalePlayback));
    super::super::frame(&mut d, &mut rig, tick(4), vec![]);
    uncover(&mut d, &mut rig);
    assert!(!d.nav.is_deliverable(&request.reply));
    assert!(!exact_track_overlay_in(&d, &request, true));
}

#[test]
fn pending_track_authority_survives_cover_but_ends_on_actual_back() {
    let _guard = plx_base::testlock::serial();
    let _globals = plx_media::route::HlsTrackRequest::test_globals_guard();
    let mut d = Dispatcher::<AppHost>::new();
    let mut rig = Bridge::for_test(|| 0);
    let request = open_and_request(&mut d, &mut rig);
    assert!(!track_commit_is_pending(&d, &request), "not registered yet");
    let MachineId::Instance(instance) = request.reply.to else { panic!("instance") };
    d.track_inflight(instance, request.reply.req);
    assert!(track_commit_is_pending(&d, &request));
    cover(&mut d, &mut rig);
    assert!(track_commit_is_pending(&d, &request), "covered live owner retains authority");
    uncover(&mut d, &mut rig);
    d.emit(MachineId::Nav, Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::Input(InputEvent {
        kind: InputKind::Key { key: Key::Back, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
        at: tick(6), source: Source::Sdl,
    }))));
    super::super::frame(&mut d, &mut rig, tick(6), vec![]);
    assert!(!track_commit_is_pending(&d, &request), "Back revokes before an async result can publish");
    for step in 7..60 { super::super::frame(&mut d, &mut rig, tick(step), vec![]); }
    assert!(!track_commit_is_pending(&d, &request), "retired owner cannot regain authority");
    assert!(!d.nav.modals.surfaces.iter().any(|surface|
        surface.entry.inst.as_ref().is_some_and(|body| body.id == instance)), "original overlay retired");
}

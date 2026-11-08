use super::*;
use plx_media::ff::{hls_presentation::HlsPlayback, ExternalSource};

fn admitted(playback: &HlsPlayback) -> route::PlaybackSession {
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &playback.media_info_for_test(),
        ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    ps
}
fn choice(ps: &route::PlaybackSession, index: usize) -> route::HlsTrackChoice {
    let catalog = route::hls_track_catalog(ps).unwrap();
    catalog.choice(catalog.rows()[index].id).unwrap()
}
#[test]
fn a_running_worker_is_cancelled_and_only_the_latest_choice_waits() {
    let mut slots = Slots::default();
    assert!(slots.replace("english").is_empty());
    let (first, cancelled) = slots.start().unwrap();
    assert_eq!(slots.replace("spanish"), ["english"]);
    assert!(cancelled.load(Ordering::Acquire));
    assert!(slots.start().is_none(), "old worker still occupies the only slot");
    assert_eq!(slots.replace("english"), ["spanish"]);
    assert!(slots.finish(first.wrapping_add(1)).is_none(), "foreign completion cannot free worker");
    assert!(slots.start().is_none());
    assert!(slots.finish(first).is_none(), "retired result has no authority");
    let (latest, _) = slots.start().unwrap();
    assert_eq!(slots.finish(latest), Some("english"));
    assert!(slots.start().is_none());
}
#[test]
fn retirement_and_drop_cancel_workers_and_never_start_retired_queue() {
    let mut slots = Slots::default();
    slots.replace(1);
    let (id, cancelled) = slots.start().unwrap();
    slots.replace(2);
    assert_eq!(slots.retire_if(|_| true), [2]);
    assert!(cancelled.load(Ordering::Acquire));
    assert!(slots.finish(id).is_none());
    assert!(slots.start().is_none());
    slots.replace(3);
    let (_, cancelled) = slots.start().unwrap();
    drop(slots);
    assert!(cancelled.load(Ordering::Acquire));
}
#[test]
fn real_hls_completion_publishes_choice_only_after_preparation() {
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = admitted(&HlsPlayback::fixture_for_test());
    let target = choice(&ps, 1);
    let request = route::begin_hls_track_request(&mut ps, &target).unwrap();
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[0].selected);
    let prepared = request.prepared_for_test();
    assert_eq!(apply_ready(&mut ps, &request, Ok(prepared)), Outcome::Admitted);
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[1].selected);
    route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
}
#[test]
fn newer_same_target_request_and_reopened_source_reject_late_preparation() {
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let playback = HlsPlayback::fixture_for_test();
    let mut ps = admitted(&playback);
    let other = choice(&ps, 1);
    let current = choice(&ps, 0);
    let old = route::begin_hls_track_request(&mut ps, &other).unwrap();
    let newer = route::begin_hls_track_request(&mut ps, &current).unwrap();
    assert_eq!(apply_ready(&mut ps, &old, Ok(old.prepared_for_test())), Outcome::Rejected(Failure::StalePlayback));
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[0].selected);
    ps = admitted(&playback);
    assert_eq!(apply_ready(&mut ps, &newer, Ok(newer.prepared_for_test())), Outcome::Rejected(Failure::StalePlayback));
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[0].selected);
    route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
}
#[test]
fn failed_preparation_keeps_the_current_language_and_never_falls_back() {
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = admitted(&HlsPlayback::fixture_for_test());
    let target = choice(&ps, 1);
    let request = route::begin_hls_track_request(&mut ps, &target).unwrap();
    assert_eq!(apply_ready(&mut ps, &request, Err("private transport error")), Outcome::Rejected(Failure::Unavailable));
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[0].selected);
    route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
}

#[test]
fn actual_back_from_hls_menu_revokes_prepared_audio_before_publication() {
    use crate::app::bridge::{self, AppHost, Bridge};
    use plx_machine::machine::{Delivery, Edge, Fx, Key, MachineId, Tick};
    use plx_screens::registry::AppArg;
    use plx_ui::{dispatch::Dispatcher, screen::ScreenEvent};

    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = admitted(&HlsPlayback::fixture_for_test());
    let mut d = Dispatcher::<AppHost>::new();
    let mut rig = Bridge::for_test(|| 0);
    let at = |i: u32| Tick { ms: i * 16, dt_us: 16_000 };
    rig.publish_playback(&ps, true);
    bridge::show_page(&mut d, AppArg::Player);
    bridge::frame(&mut d, &mut rig, at(0), vec![]);
    bridge::open_player_overlay(&ps, plx_data::stores::metadata::MetadataStore::default().view(),
        &mut d, plx_screens::player::overlay::OverlayKind::Tracks { tab: 0 });
    bridge::frame(&mut d, &mut rig, at(1), vec![]);
    let instance = d.nav.modals.surfaces.last().unwrap().entry.inst.as_ref().unwrap().id;
    let press = |d: &mut Dispatcher<AppHost>, rig: &mut Bridge, key, index| {
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::Input(bridge::script_edge(key, Edge::Down, at(index))))));
        bridge::frame(d, rig, at(index), vec![]);
    };
    press(&mut d, &mut rig, Key::Down, 2);
    press(&mut d, &mut rig, Key::Ok, 3);
    let ui_request = rig.take_player_reqs().into_iter().find_map(|request| match request {
        PlayerReq::CommitTrackAddressed(request) => Some(request), _ => None,
    }).expect("actual HLS menu emitted addressed choice");
    let TrackCommit::Hls(choice) = ui_request.commit.clone() else { panic!("HLS choice") };
    assert_eq!(bridge::begin_track_commit(&mut d, &ps, &ui_request), bridge::TrackCommitAdmission::Ready);
    assert!(owner_pending(&d, Some(&ui_request)));
    let route_request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
    let prepared = route_request.prepared_for_test();
    let pending = Pending { route: route_request, reply: Recipient::Ui(Some(ui_request)) };
    press(&mut d, &mut rig, Key::Back, 4);
    let Recipient::Ui(request) = &pending.reply else { panic!("UI recipient") };
    assert!(!owner_pending(&d, request.as_ref()));
    assert_eq!(apply_owned_ready(&d, &mut ps, &pending, Ok(prepared)),
        Outcome::Rejected(Failure::StalePlayback));
    assert!(route::hls_track_catalog(&ps).unwrap().rows()[0].selected,
        "retired menu did not switch to the prepared second language");
    assert!(owner_pending(&d, None), "trusted unaddressed requests have no overlay owner");
}

#[test]
fn failed_hls_subtitle_preparation_preserves_accepted_selection_and_av() {
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
    let catalog = route::hls_track_catalog(&ps).unwrap();
    let rows = catalog.subtitle_rows();
    let first = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(Some(rows[0].id)).unwrap()).unwrap();
    assert_eq!(apply_ready(&mut ps, &first, Ok(first.prepared_for_test())), Outcome::Admitted);
    let audio = route::hls_track_catalog(&ps).unwrap().rows();
    let identity = route::playback_identity(&ps);
    let worker = route::worker_ticket();
    let state = plx_media::player::state(&ps);
    let position = plx_media::player::SHARED.playpos_ns.load(Ordering::Acquire);
    let audio_switch = plx_media::player::audio_switching();
    let next = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(Some(rows[1].id)).unwrap()).unwrap();
    assert_eq!(apply_ready(&mut ps, &next, Err("subtitle unavailable")), Outcome::Rejected(Failure::Unavailable));
    let after = route::hls_track_catalog(&ps).unwrap();
    assert_eq!(after.selected_subtitle(), Some(rows[0].id));
    assert_eq!(after.rows(), audio);
    assert_eq!(route::playback_identity(&ps), identity);
    assert_eq!(route::worker_ticket(), worker);
    assert_eq!(plx_media::player::state(&ps), state);
    assert_eq!(plx_media::player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert_eq!(plx_media::player::audio_switching(), audio_switch);
}

#[test]
fn hls_subtitle_off_retires_pending_preparation_and_reopened_source_rejects_it() {
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let playback = HlsPlayback::fixture_with_subtitles_for_test();
    let mut ps = admitted(&playback);
    let catalog = route::hls_track_catalog(&ps).unwrap();
    let pick = catalog.subtitle_choice(Some(catalog.subtitle_rows()[0].id)).unwrap();
    let old = route::begin_hls_track_request(&mut ps, &pick).unwrap();
    let mut slots = Slots::default();
    slots.replace(old.clone());
    let (old_id, cancelled) = slots.start().unwrap();
    let off = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(None).unwrap()).unwrap();
    assert_eq!(slots.replace(off.clone()).len(), 1);
    assert!(cancelled.load(Ordering::Acquire));
    assert!(slots.start().is_none(), "Off shares the one occupied worker slot");
    assert_eq!(apply_ready(&mut ps, &old, Ok(old.prepared_for_test())), Outcome::Rejected(Failure::StalePlayback));
    assert!(slots.finish(old_id).is_none());
    let (off_id, _) = slots.start().unwrap();
    let ready = slots.finish(off_id).unwrap();
    assert!(matches!(apply_ready(&mut ps, &ready, Ok(ready.prepared_for_test())), Outcome::Admitted | Outcome::Unchanged));
    assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
    let old = route::begin_hls_track_request(&mut ps, &pick).unwrap();
    ps = admitted(&playback);
    assert_eq!(apply_ready(&mut ps, &old, Ok(old.prepared_for_test())), Outcome::Rejected(Failure::StalePlayback));
    assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
}

#[test]
fn selected_hls_text_opens_shared_timing_and_commits_offset_without_av_change() {
    use plx_appkit::track_menu::{TrackMenuState, TrackOk, TrackRow};
    use plx_appkit::timing_capsule::{CapsuleOut, TimingCapsule};
    use plx_media::player;
    let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
    let meta = plx_data::stores::metadata::MetadataStore::default();
    let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
    assert!(!menu.focus_id(TrackRow::Timing), "Off cannot open timing");
    let catalog = route::hls_track_catalog(&ps).unwrap();
    let choice = catalog.subtitle_choice(Some(catalog.subtitle_rows()[0].id)).unwrap();
    let request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
    assert!(!menu.focus_id(TrackRow::Timing), "pending selection is not published");
    assert_eq!(apply_ready(&mut ps, &request, Ok(request.prepared_for_test())), Outcome::Admitted);
    let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
    assert!(menu.focus_id(TrackRow::Timing));
    assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::OpenTiming);
    let (lo, hi) = player::subtitle_offset_range_ms();
    let mut capsule = TimingCapsule::new(player::subtitle_offset_ms(), lo, hi);
    let Some(CapsuleOut::Step(offset)) = capsule.key(plx_ui::consts::Key::Right { alt: false },
        plx_machine::machine::Edge::Down, 0) else { panic!("shared timing step") };
    let before = route::playback_identity(&ps);
    let worker = route::worker_ticket();
    let audio = route::hls_track_catalog(&ps).unwrap().rows();
    let switching = player::audio_switching();
    let state = player::state(&ps);
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    assert_eq!(crate::app::playback::commit_track_outcome(&mut ps, TrackCommit::SubtitleOffset(offset)), Outcome::Admitted);
    assert_eq!(player::subtitle_offset_ms(), offset);
    assert_eq!(offset, player::SUBTITLE_OFFSET_STEP_MS);
    assert_eq!(route::playback_identity(&ps), before);
    assert_eq!(route::worker_ticket(), worker);
    assert_eq!(route::hls_track_catalog(&ps).unwrap().rows(), audio);
    assert_eq!(player::audio_switching(), switching);
    assert_eq!(player::state(&ps), state);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
}

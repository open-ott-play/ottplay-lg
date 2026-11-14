//! Held resolver tests: no provider, storage worker or decoder is started.
use super::*;
use plx_machine::machine::UiMode;

fn fixture() -> (App, Box<Runtime>) {
    let mut app = ready_app_fixture();
    app.running = true;
    app.bridge.apply_ui_mode(UiMode::Classic);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    runtime.saved.profiles.profiles = vec![synthetic_profile()];
    runtime.saved.profiles.active_profile = Some("portal-fixture".into());
    runtime.catalog.entries = vec![synthetic_entry()];
    runtime.page = "channels";
    runtime.provider = ProviderKind::VPortal;
    runtime.reply = current_ui_owner(&app);
    assert!(runtime.reply.is_some());
    runtime.resolver_busy = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.write_permit.revoke();
    runtime.selected = Some("late-fixture".into());
    browse_focus::record(&mut runtime, "late-fixture");
    (app, runtime)
}
fn begin(runtime: &mut Runtime, app: &App) {
    runtime.play_generation += 1;
    runtime.play_target = Some(PlaybackRequest::ordinary("late-fixture", 0));
    runtime.play_profile = Some((runtime.play_generation, synthetic_profile()));
    classic_play_handoff::begin(runtime, app);
    assert_eq!(runtime.view().pending_play.as_deref(), Some("late-fixture"));
    assert_eq!(runtime.view().status, "Opening video…");
    assert!(runtime.playing.is_none(), "cover cannot invent a playing source");
}

#[test]
fn classic_play_handoff_is_bound_to_exact_owner_request_source_and_generation() {
    let _serial = plx_base::testlock::serial();
    let (app, mut r) = fixture();
    begin(&mut r, &app);
    r.catalog_revision += 1;
    assert!(r.view().pending_play.is_some(), "progressive catalogue updates do not dismiss handoff");
    r.generation += 1;
    assert!(r.view().pending_play.is_none());
    r.generation -= 1;
    r.play_target.as_mut().unwrap().position_ms += 1;
    assert!(r.view().pending_play.is_none());
    r.play_target.as_mut().unwrap().position_ms = 0;
    r.play_profile.as_mut().unwrap().1.source.push_str("-changed");
    assert!(r.view().pending_play.is_none());
    r.play_profile.as_mut().unwrap().1 = synthetic_profile();
    r.reply = None;
    assert!(r.view().pending_play.is_none());
}

#[test]
fn classic_play_handoff_back_keeps_exact_list_focus_and_retires_late_work() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut r) = fixture();
    begin(&mut r, &app);
    let catalog = r.generation;
    let play = r.play_generation;
    let focus = r.view().selected_row;
    let selected = r.selected.clone();
    apply_ui_action(&mut r, &mut app, OttAction::Back, true, false, false);
    assert_eq!(r.page, "channels");
    assert_eq!(r.generation, catalog);
    assert_eq!(r.selected, selected);
    assert_eq!(r.view().selected_row, focus);
    assert!(r.view().pending_play.is_none());
    assert!(r.play_target.is_none());
    assert_ne!(r.play_generation, play);
    assert_eq!(r.reply, current_ui_owner(&app));
    assert!(!app.pages.has_pending_navigation());
    classic_play_handoff::completed(&mut r, play, true);
    assert!(r.view().pending_play.is_none(), "cancelled work cannot reopen the cover or Player");
}

#[test]
fn classic_play_handoff_failure_preserves_selection_and_does_not_navigate() {
    let _serial = plx_base::testlock::serial();
    let (app, mut r) = fixture();
    begin(&mut r, &app);
    let focus = r.view().selected_row;
    let play = r.play_generation;
    classic_play_handoff::completed(&mut r, play, false);
    r.status = "Playback failed".into();
    assert!(r.view().pending_play.is_none());
    assert_eq!(r.view().selected_row, focus);
    assert_eq!(r.page, "channels");
    assert!(!app.pages.has_pending_navigation());
    assert!(r.playing.is_none());
}

#[test]
fn classic_play_handoff_modern_and_failed_back_without_video_keep_existing_hierarchy() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut r) = fixture();
    r.saved.settings.insert("interfaceMode".into(), json!(1));
    r.play_target = Some(PlaybackRequest::ordinary("late-fixture", 0));
    r.play_profile = Some((r.play_generation, synthetic_profile()));
    classic_play_handoff::begin(&mut r, &app);
    assert!(r.view().pending_play.is_none());
    r.cancel_play_request();
    r.saved.settings.insert("interfaceMode".into(), json!(0));
    assert!(!classic_play_handoff::close_catalog(&mut r, &mut app));
    assert!(!app.pages.has_pending_navigation(), "a catalogue is not proof of installed video");
}

#[test]
fn classic_play_handoff_selected_vod_admission_cannot_auto_advance_on_failure() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut r) = fixture();
    apply_ui_action(&mut r, &mut app, OttAction::Play("late-fixture".into()), true, true, false);
    let request = r.play_target.as_ref().expect("accepted selected video");
    assert!(!request.advance_on_failure);
    assert!(r.view().pending_play.is_some());
    assert!(r.playing.is_none());
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn install(app: &mut App, runtime: Box<Runtime>) -> Box<Runtime> {
    app.ott = Some(runtime);
    local_integration::install(app, None, "late-fixture");
    let mut r = app.ott.take().unwrap();
    let playing = r.playing.as_mut().unwrap();
    playing.profile = synthetic_profile();
    playing.entry = synthetic_entry();
    playing.transient_owner = None;
    playing.advance_on_failure = false;
    assert!(playing_matches(playing, app));
    r
}

#[test]
fn classic_play_handoff_admitted_cover_survives_target_retirement_until_player_owns_screen() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, mut r) = fixture();
    begin(&mut r, &app);
    r = install(&mut app, r);
    let generation = r.play_generation;
    classic_play_handoff::completed(&mut r, generation, true);
    r.take_completed_play_target();
    assert!(r.view().pending_play.is_some(), "real admission must not flash the list during Player navigation");
    r.reply = None;
    assert!(r.view().pending_play.is_none(), "old presentation cannot survive its owner");
}

#[test]
fn classic_selected_vod_successful_eof_advances_with_failure_skip_disabled() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, r) = fixture();
    let mut r = install(&mut app, r);
    let mut next = synthetic_entry();
    next.id = "next-video".into();
    next.media = MediaLocator::VPortal { request: json!({"id":2}) };
    r.catalog.entries.push(next);
    r.catalog_loading = false;
    r.catalog_complete = true;
    r.continuity.user_selected(r.generation, "late-fixture");
    assert!(!r.playing.as_ref().unwrap().advance_on_failure);
    app.ott = Some(r);
    assert!(finish_catalog_playback(&mut app));
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.play_target.as_ref().map(|request|request.id.as_str()), Some("next-video"));
    assert!(r.resolver_next.is_some(), "normal EOF still enters the existing source resolver");
}

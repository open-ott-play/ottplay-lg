//! App/content regressions for addressed native subtitle settings.
//! A revoked permit exercises the real writer's definite failure before any
//! filesystem/key-manager IO. Injected Saved/Uncertain Jobs test the application
//! completion boundary; they do not prove an encrypted durable write.
use super::*;
use plx_appkit::track_menu::{StyleField, TrackCommit, TrackCommitSource, TrackPage, TrackRow};
use plx_machine::machine::{Delivery, InputOwner, ScreenEvent};
use plx_media::player::{
    self,
    text_style::{TextAppearance, TextColor},
    PlaybackState,
};
use plx_media::route::{self, NativeSubtitlePreferences};
use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};
use plx_screens::player::overlay::{OverlayKind, Panel};
use plx_screens::registry::TrackCommitRequest;
use plx_ui::form::FormId;

struct StyleGlobals {
    state: u8,
    size: SubtitleSize,
    position: SubtitlePosition,
    tone: SubtitleTone,
}
impl StyleGlobals {
    fn capture() -> Self {
        Self {
            state: player::swap_state_for_test(PlaybackState::Idle),
            size: route::subtitle_size(),
            position: route::subtitle_position(),
            tone: player::subtitle_tone(),
        }
    }
}
impl Drop for StyleGlobals {
    fn drop(&mut self) {
        player::SHARED.reset_session();
        player::TX.reset();
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
        route::restore_subtitle_size(self.size);
        route::restore_subtitle_position(self.position);
        player::restore_subtitle_tone(self.tone);
        player::restore_state_for_test(self.state);
    }
}

fn style_fixture() -> App {
    style_fixture_with_subtitle(false)
}

fn style_fixture_with_subtitle(active_subtitle: bool) -> App {
    let mut app = app_fixture();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.saving = false;
        runtime.write_permit.revoke();
    }
    install_external_fixture(&mut app, "a", active_subtitle);
    mount_player(&mut app);
    app
}

/// Real source admission mints a fresh ExternalTracks identity even if the
/// source URL is repeated. No inspection, demux, Engine or physical Load runs.
fn install_external(app: &mut App, name: &str) {
    install_external_fixture(app, name, false);
}

fn install_external_fixture(app: &mut App, name: &str, active_subtitle: bool) {
    assert!(!app.adapters.player.is_live());
    route::forget_external_source(&mut app.player.session);
    route::reset_player_control_for_test(&app.player.session);
    let info = plx_media::ff::inspect::MediaInfo {
        // Style is meaningful only for an admitted active subtitle. The real
        // external menu intentionally omits it for an empty or Off catalog.
        tracks: if active_subtitle { plx_media::ff::inspect::TrackInventory::from_partial(Vec::new(), vec![
            plx_media::ff::inspect::InspectedTrack {
                kind: plx_media::ff::inspect::TrackKind::Subtitle,
                stream_index: 2,
                ordinal: 0,
                codec: "subrip".into(),
                language: "eng".into(),
                title: "Plain text fixture".into(),
                channels: 0,
                sample_rate: 0,
                bit_depth: 0,
                decoder_available: true,
                disposition_known: true,
                default: false,
                forced: false,
            },
        ]) } else { Default::default() },
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 60_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: plx_data::metadata::Dovi::NONE,
    };
    route::request_external_source(
        &mut app.player.session,
        &info,
        plx_media::ff::ExternalSource::Http(format!("https://media.invalid/{name}.mkv")),
        name,
        "Subtitle boundary fixture",
        plx_platform::devcaps::assumed_shared(),
    )
    .expect("the inert external route must be admitted");
    route::reset_player_control_for_test(&app.player.session);
    if active_subtitle {
        let subtitle = route::external_tracks(&app.player.session)
            .expect("admitted external catalog")
            .choice(plx_media::ff::inspect::TrackKind::Subtitle, Some(0))
            .expect("fixture text subtitle must be supported");
        assert_eq!(route::apply_external_track(&mut app.player.session, &subtitle), Ok(true));
        assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_subtitle(), Some(0));
    }
    player::swap_state_for_test(PlaybackState::Paused);
    route::restore_native_subtitle_preferences(
        &mut app.player.session,
        NativeSubtitlePreferences {
            size: SubtitleSize::Medium,
            position: SubtitlePosition::Low,
            tone: SubtitleTone::White,
        },
    );
    route::restore_subtitle_text_appearance(&mut app.player.session, TextAppearance::default());
    app.bridge.sync_subtitle_preferences(&app.player.session);
    assert_eq!(player::state(&app.player.session), PlaybackState::Paused);
    assert!(route::has_url(&app.player.session));
    assert!(route::external_tracks(&app.player.session).is_some());
    assert!(
        !app.adapters.player.is_live(),
        "this is not a decoder fixture"
    );
}

/// Cross the same dispatcher queue and content performer as a real player action.
/// Addressed UI tests may pass the actual overlay-produced request unchanged.
fn through_content(app: &mut App, request: PlayerReq) {
    let from = match &request {
        PlayerReq::CommitTrackAddressed(request) => request.reply.to,
        _ => MachineId::Nav,
    };
    through_content_from(app, from, request);
}

fn through_content_from(app: &mut App, from: MachineId, request: PlayerReq) {
    app.pages.emit(from, Fx::App(AppFx::Player(request)));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    let frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms,
        dt: 0.016,
        underlay_moving: false,
        player: true,
        present: false,
        deferred: false,
        rn: "player",
    };
    content::content_requests(app, &frame);
}

/// Wait only for the existing worker's real failure. Do not settle it yet:
/// PendingSave must still be the tag admitted by the actual gateway.
fn capture_revoked_worker(app: &mut App) -> Job {
    let runtime = app.ott.as_mut().unwrap();
    assert!(!runtime.write_permit.is_valid());
    assert!(runtime.saving);
    assert!(runtime.subtitle_save.is_some());
    let job = runtime
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the revoked real writer must answer without IO");
    assert!(
        matches!(
            &job,
            Job::Saved(Err("Could not prepare protected OTT settings for saving"), None, false, _)
        ),
        "fixture must observe the real definite failure, not an invented success"
    );
    assert!(runtime.subtitle_save.is_some(), "receiving is not settling");
    job
}

/// Model a verified completion after the actual writer has already finished.
/// Callers must first capture/drop its definite-failure job: no competing writer
/// is allowed to remain and later inject an unexpected second completion.
fn deliver_model_saved(app: &mut App, candidate: Saved) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(candidate),
            None,
            false,
            ok(json!({"persisted": true})),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
}

fn live_size(app: &App) -> SubtitleSize {
    route::native_subtitle_preferences(&app.player.session)
        .unwrap()
        .size
}

#[test]
fn late_subtitle_saved_for_a_does_not_publish_into_b_until_fresh_overlay() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let old_source = TrackCommitSource::capture(&app.player.session);
    let candidate = subtitle_settings::Edit::Size(SubtitleSize::Large)
        .candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    drop(capture_revoked_worker(&mut app));
    assert_eq!(
        live_size(&app),
        SubtitleSize::Medium,
        "no optimistic publication"
    );

    install_external(&mut app, "b");
    assert!(
        !old_source.matches(&app.player.session),
        "A and B must be distinct owners"
    );
    deliver_model_saved(&mut app, candidate.clone());
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, candidate.settings);
    assert_eq!(
        live_size(&app),
        SubtitleSize::Medium,
        "A's completion must not mutate B's live setting"
    );
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert!(!app.ott.as_ref().unwrap().saving);

    through_content(
        &mut app,
        PlayerReq::OpenOverlay(OverlayKind::Tracks { tab: 1 }),
    );
    assert_eq!(
        live_size(&app),
        SubtitleSize::Large,
        "fresh menu reads the verified installation preference"
    );
    assert!(
        !app.ott.as_ref().unwrap().saving,
        "opening must not start a new write"
    );
    assert!(matches!(
        app.ott.as_ref().unwrap().rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn unknown_subtitle_completion_stays_blocked_after_overlay_reopen() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let candidate = subtitle_settings::Edit::Size(SubtitleSize::Large)
        .candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    drop(capture_revoked_worker(&mut app));
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Uncertain(Some(candidate), None))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert!(!app.ott.as_ref().unwrap().loaded);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(
        live_size(&app),
        SubtitleSize::Medium,
        "visible after-image is not proof of durability and must not publish"
    );

    bridge::dismiss_player_overlays(&mut app.pages);
    settle_pages(&mut app);
    through_content(
        &mut app,
        PlayerReq::OpenOverlay(OverlayKind::Tracks { tab: 1 }),
    );
    settle_pages(&mut app);
    assert_eq!(
        live_size(&app),
        SubtitleSize::Medium,
        "reopen must not publish uncertain data"
    );
    // Re-request the exact visible value: uncertainty must be checked BEFORE
    // the unchanged/no-write fast path. Otherwise it could publish this value.
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    assert_eq!(live_size(&app), SubtitleSize::Medium);
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(matches!(
        app.ott.as_ref().unwrap().rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn definite_subtitle_write_failure_releases_slot_and_allows_a_real_retry() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = app.ott.as_ref().unwrap().saved.settings.clone();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    let failed = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failed)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, before);
    assert_eq!(live_size(&app), SubtitleSize::Medium);
    assert!(app.ott.as_ref().unwrap().loaded);
    assert!(!app.ott.as_ref().unwrap().persistence_uncertain);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());

    // The permit deliberately stays revoked, so retry is another real admitted
    // transaction and another real failure, rather than touching storage.
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    let failed_again = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failed_again)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(!app.ott.as_ref().unwrap().persistence_uncertain);
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, before);
    assert_eq!(live_size(&app), SubtitleSize::Medium);
}

// Additional lifecycle boundary cases, appended to the shared app test fixture.
#[test]
fn verified_subtitle_save_publishes_to_same_source_while_standby_without_starting_media() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = plx_appkit::track_menu::TrackCommitSource::capture(&app.player.session);
    let wanted = plx_media::player::text_style::TextAppearance {
        color: plx_media::player::text_style::TextColor::Yellow,
        ..Default::default()
    };
    let candidate =
        subtitle_settings::Edit::Appearance(wanted).candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(plx_appkit::track_menu::TrackCommit::TextAppearance(wanted)),
    );
    drop(capture_revoked_worker(&mut app));

    // Model the foreground gate at completion without asking a native decoder
    // to park or reload a real file. The already-admitted write owns this source.
    app.ott_standby = true;
    assert!(!super::super::playback_allowed(&app));
    deliver_model_saved(&mut app, candidate);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        wanted
    );
    assert!(before.matches(&app.player.session));
    assert!(app.ott_standby);
    assert!(
        !app.adapters.player.is_live(),
        "publishing a setting must not start media"
    );
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());

    app.ott_standby = false;
    super::super::resume(&mut app);
    runtime_tick(&mut app);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        wanted,
        "returning to the same source must not retain the old subtitle style"
    );
    assert!(!app.adapters.player.is_live());
}

#[test]
fn verified_subtitle_save_is_not_lost_during_same_source_resolving_state() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = plx_appkit::track_menu::TrackCommitSource::capture(&app.player.session);
    let wanted = plx_media::player::text_style::TextAppearance {
        font: plx_media::player::text_style::TextFont::InterRegular,
        ..Default::default()
    };
    let candidate =
        subtitle_settings::Edit::Appearance(wanted).candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(plx_appkit::track_menu::TrackCommit::TextAppearance(wanted)),
    );
    drop(capture_revoked_worker(&mut app));

    // Model the transient Resolving state while retaining the admitted source.
    // This is a completion-state boundary, not a running resolver fixture.
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Resolving);
    assert_eq!(player::state(&app.player.session), PlaybackState::Resolving);
    assert!(before.matches(&app.player.session));
    deliver_model_saved(&mut app, candidate);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        wanted
    );
    assert!(before.matches(&app.player.session));
    assert!(!app.adapters.player.is_live());
}

#[test]
fn trusted_subtitle_command_without_runtime_cannot_publish_or_bypass_storage() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = plx_media::route::subtitle_text_appearance(&app.player.session);
    app.ott = None;
    super::super::commit_track(
        &mut app,
        plx_appkit::track_menu::TrackCommit::TextAppearance(
            plx_media::player::text_style::TextAppearance {
                color: plx_media::player::text_style::TextColor::Cyan,
                ..Default::default()
            },
        ),
    );
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
    assert!(app.ott.is_none());
    assert!(!app.adapters.player.is_live());
}

fn content_overlay_owner(app: &App) -> MachineId {
    let Some(InputOwner::Entry(entry)) = app.pages.nav.input_owner() else {
        panic!("mounted track overlay must own input");
    };
    MachineId::Instance(
        app.pages
            .nav
            .entry(entry)
            .expect("live overlay entry")
            .inst
            .as_ref()
            .expect("mounted overlay")
            .id,
    )
}

fn content_open_tracks(app: &mut App) {
    app.bridge.publish_playback(&app.player.session, true);
    bridge::open_player_overlay(
        &app.player.session,
        app.bridge.metadata_view(),
        &mut app.pages,
        OverlayKind::Tracks { tab: 1 },
    );
    settle_pages(app);
    assert_eq!(
        bridge::player_overlay_kind(&app.pages),
        Some(OverlayKind::Tracks { tab: 1 })
    );
    // Opening/focusing contributes only non-edit HUD requests.
    let initial = app.bridge.take_player_reqs();
    assert!(!initial.iter().any(|request| matches!(
        request,
        PlayerReq::CommitTrack(_) | PlayerReq::CommitTrackAddressed(_)
    )));
}

fn content_hit_track_row(app: &mut App, row: TrackRow) -> Vec<PlayerReq> {
    assert!(
        bridge::player_overlay_mut(&mut app.pages)
            .expect("mounted track panel")
            .seat_track_row(row),
        "row must actually exist: {row:?}"
    );
    let owner = content_overlay_owner(app);
    app.pages.emit(
        MachineId::Input,
        Fx::Deliver(owner, Delivery::Screen(ScreenEvent::Activate(row.key().0))),
    );
    settle_pages(app);
    app.bridge.take_player_reqs()
}

fn content_request_color(app: &mut App, color: TextColor) -> TrackCommitRequest {
    content_open_tracks(app);
    for row in [TrackRow::Style, TrackRow::OpenField(StyleField::TextColor)] {
        let requests = content_hit_track_row(app, row);
        assert!(
            !requests.iter().any(|request| matches!(
                request,
                PlayerReq::CommitTrack(_) | PlayerReq::CommitTrackAddressed(_)
            )),
            "navigation must not silently edit a preference"
        );
    }
    let mut requests = content_hit_track_row(
        app,
        TrackRow::Choice(StyleField::TextColor, usize::from(color.index())),
    )
    .into_iter()
    .filter_map(|request| match request {
        PlayerReq::CommitTrackAddressed(request) => Some(request),
        PlayerReq::CommitTrack(_) => panic!("interactive overlay used the trusted legacy path"),
        _ => None,
    });
    let request = requests
        .next()
        .expect("real choice must emit one addressed edit");
    assert!(
        requests.next().is_none(),
        "one click must not emit two writes"
    );
    assert!(matches!(&request.commit, TrackCommit::TextAppearance(value) if value.color == color));
    assert!(request.source.matches(&app.player.session));
    assert!(bridge::player_overlay_mut(&mut app.pages)
        .unwrap()
        .accepts_track_commit(&request));
    request
}

fn content_close_tracks(app: &mut App) {
    bridge::dismiss_player_overlays(&mut app.pages);
    // PlayerPanel stays mounted through Closing until its appear spring rests.
    // The generic 24-frame settling helper is shorter than that fade. Observe
    // actual dispatcher retirement before opening the replacement owner.
    for _ in 0..200 {
        if !bridge::player_overlay_up(&app.pages) { break; }
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
    assert!(!bridge::player_overlay_up(&app.pages), "track panel must finish dismissal");
}

fn native_settings_action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT settings");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
}

fn settle_native_controls(app: &mut App) {
    for _ in 0..32 {
        let tick = next_tick(app);
        let live = bridge::player(&app.pages).is_some();
        app.bridge.publish_playback(&app.player.session, live);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        runtime_tick(app);
    }
}

fn native_settings_fixture() -> App {
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().loaded = true;
    app.ott.as_mut().unwrap().write_permit.revoke();
    install_external(&mut app, "native-settings");
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    native_settings_action(&mut app, OttAction::ShowSettings);
    settle_pages(&mut app);
    app
}

#[test]
fn signed_out_ott_settings_opens_actual_audio_and_subtitle_panels_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("ott-native-settings");
    let _globals = StyleGlobals::capture();
    for (key, tab) in [("native.audio", 0), ("native.subtitles", 1)] {
        let mut app = native_settings_fixture();
        assert_eq!(plx_plex::plex::session::current_snapshot().seat(), plx_plex::plex::session::Seat::Unseated);
        let ticket = route::play_request_ticket();
        assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|row|
            row.action == OttAction::EditSetting(key.into())));
        native_settings_action(&mut app, OttAction::EditSetting(key.into()));
        settle_native_controls(&mut app);
        assert!(matches!(app.route(), AppArg::Player));
        assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::Tracks { tab }));
        assert!(app.ott.as_ref().unwrap().native_controls.is_none());
        assert!(route::play_request_is_current(ticket));
        assert_eq!(player::state(&app.player.session), PlaybackState::Paused);
        assert!(!app.adapters.player.is_live());
    }
}

#[test]
fn signed_out_subtitle_settings_reaches_real_addressed_saved_preference_path() {
    let _serial = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("ott-native-subtitle-save");
    let _globals = StyleGlobals::capture();
    let mut app = native_settings_fixture();
    install_external_fixture(&mut app, "native-settings", true);
    native_settings_action(&mut app, OttAction::EditSetting("native.subtitles".into()));
    settle_native_controls(&mut app);
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::Tracks { tab: 1 }));
    let request = content_request_color(&mut app, TextColor::Yellow);
    through_content(&mut app, PlayerReq::CommitTrackAddressed(request));
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_some());
    let failure = capture_revoked_worker(&mut app);
    app.ott.as_ref().unwrap().tx.send(failure).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(bridge::player_overlay_up(&app.pages), "failed write keeps the real menu available");
}

#[test]
fn native_settings_without_stream_refuses_without_account_navigation_or_worker() {
    let _serial = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("ott-native-settings-idle");
    let _globals = StyleGlobals::capture();
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().loaded = true;
    app.ott.as_mut().unwrap().write_permit.revoke();
    assert!(!app.ott.as_ref().unwrap().persistence_uncertain);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    native_settings_action(&mut app, OttAction::ShowSettings);
    for key in ["native.audio", "native.subtitles"] {
        native_settings_action(&mut app, OttAction::EditSetting(key.into()));
        settle_native_controls(&mut app);
        assert!(matches!(app.route(), AppArg::Ott));
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.native_controls.is_none());
        assert_eq!(runtime.status, "Start a video to choose audio or subtitle settings");
        assert!(!bridge::player_overlay_up(&app.pages));
        assert!(!runtime.saving);
    }
}

#[test]
fn cancelled_source_cannot_receive_a_deferred_settings_track_panel() {
    let _serial = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("ott-native-settings-retired");
    let _globals = StyleGlobals::capture();
    let mut app = native_settings_fixture();
    native_settings_action(&mut app, OttAction::EditSetting("native.subtitles".into()));
    route::cancel_play(&mut app.player.session);
    settle_native_controls(&mut app);
    assert!(app.ott.as_ref().unwrap().native_controls.is_none());
    assert!(!bridge::player_overlay_up(&app.pages));
    assert!(!app.adapters.player.is_live());
}

#[test]
fn changed_kiosk_authority_cancels_a_deferred_settings_track_panel() {
    let _serial = plx_base::testlock::serial();
    let _session = plx_plex::plex::session::TempSession::new("ott-native-settings-policy");
    let _globals = StyleGlobals::capture();
    let mut app = native_settings_fixture();
    native_settings_action(&mut app, OttAction::EditSetting("native.subtitles".into()));
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!(true));
    settle_native_controls(&mut app);
    assert!(app.ott.as_ref().unwrap().native_controls.is_none());
    assert!(!bridge::player_overlay_up(&app.pages));
}

#[test]
fn addressed_content_admits_real_overlay_choice_then_reports_actual_save_failure() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    let before = plx_media::route::subtitle_text_appearance(&app.player.session);
    let saved_before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let request = content_request_color(&mut app, TextColor::Yellow);

    through_content(&mut app, PlayerReq::CommitTrackAddressed(request.clone()));
    assert!(
        app.ott.as_ref().unwrap().saving,
        "content must reach Runtime.save"
    );
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_some());
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        saved_before
    );
    assert!(
        bridge::player_overlay_mut(&mut app.pages)
            .unwrap()
            .accepts_track_commit(&request),
        "admission alone is not a successful completion"
    );

    // Exercise the real writer's revoked-permit result: no filesystem write.
    let failure = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failure)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        saved_before
    );
    let overlay = bridge::player_overlay_mut(&mut app.pages).expect("failure leaves menu open");
    assert!(!overlay.accepts_track_commit(&request));
    let Panel::Tracks(panel) = overlay.panel() else {
        panic!("track failure panel")
    };
    assert_eq!(panel.page_path().last(), Some(&TrackPage::CommitFailure));
    assert!(panel.ids().contains(&TrackRow::CommitRetry));
}

#[test]
fn addressed_content_rejects_a_real_choice_after_source_replacement_before_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    let request = content_request_color(&mut app, TextColor::Yellow);
    install_external(&mut app, "b");
    assert!(!request.source.matches(&app.player.session));
    let before = plx_media::route::subtitle_text_appearance(&app.player.session);
    let saved_before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();

    through_content(&mut app, PlayerReq::CommitTrackAddressed(request));
    let runtime = app.ott.as_ref().unwrap();
    assert!(
        !runtime.saving,
        "stale choice must be fenced before Runtime.save"
    );
    assert!(runtime.subtitle_save.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved_before);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
}

#[test]
fn addressed_content_cannot_retarget_a_retired_overlay_choice_to_its_replacement() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    let retired = content_request_color(&mut app, TextColor::Yellow);
    content_close_tracks(&mut app);
    let current = content_request_color(&mut app, TextColor::Green);
    assert_ne!(
        retired.reply.to, current.reply.to,
        "reopening must create a distinct mounted overlay owner"
    );
    assert!(
        retired.source.matches(&app.player.session),
        "only overlay identity has changed"
    );

    through_content(&mut app, PlayerReq::CommitTrackAddressed(retired));
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert!(
        bridge::player_overlay_mut(&mut app.pages)
            .unwrap()
            .accepts_track_commit(&current),
        "old request must not consume the replacement's pending choice"
    );

    // The exact replacement request remains admissible, proving the stale path
    // did not merely disable all edits or leave a Busy fence behind.
    through_content(&mut app, PlayerReq::CommitTrackAddressed(current));
    assert!(app.ott.as_ref().unwrap().saving);
    let failure = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failure)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
}

#[test]
fn addressed_late_model_success_cannot_complete_or_close_reopened_overlay() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    let retired = content_request_color(&mut app, TextColor::Yellow);
    let TrackCommit::TextAppearance(appearance) = retired.commit.clone() else {
        panic!("appearance fixture");
    };
    let candidate =
        subtitle_settings::Edit::Appearance(appearance).candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(&mut app, PlayerReq::CommitTrackAddressed(retired.clone()));
    drop(capture_revoked_worker(&mut app));
    content_close_tracks(&mut app);
    let current = content_request_color(&mut app, TextColor::Green);
    assert_ne!(retired.reply.to, current.reply.to);

    // Substitute at the Job boundary after consuming the real worker result.
    // This proves stale completion addressing, not successful physical storage.
    deliver_model_saved(&mut app, candidate);
    settle_pages(&mut app);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        appearance
    );
    let overlay = bridge::player_overlay_mut(&mut app.pages).expect("new overlay must stay open");
    assert!(
        overlay.accepts_track_commit(&current),
        "late result for old instance must not clear replacement's pending edit"
    );
    assert!(!overlay.accepts_track_commit(&retired));
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
}

#[test]
fn trusted_app_commit_uses_runtime_persistence_before_applying_subtitle_appearance() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = plx_media::route::subtitle_text_appearance(&app.player.session);
    let after = TextAppearance {
        color: TextColor::Yellow,
        ..before
    };
    let saved_before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();

    // Actual entry point used by playback::commit_track_from_app and the
    // developer scenario. No UI request is fabricated for this trusted path.
    crate::app::ott_runtime::commit_track(&mut app, TrackCommit::TextAppearance(after));
    assert!(app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_some());
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
    let failure = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failure)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(
        plx_media::route::subtitle_text_appearance(&app.player.session),
        before
    );
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        saved_before
    );
}

#[test]
fn retired_external_subtitle_save_does_not_overwrite_nonexternal_geometry() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let old_source = TrackCommitSource::capture(&app.player.session);
    let candidate = subtitle_settings::Edit::Size(SubtitleSize::Large)
        .candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    through_content(
        &mut app,
        PlayerReq::CommitTrack(TrackCommit::SubtitleSize(SubtitleSize::Large)),
    );
    drop(capture_revoked_worker(&mut app));

    route::forget_external_source(&mut app.player.session);
    app.bridge.sync_subtitle_preferences(&app.player.session);
    // Stand in for the new Plex route's already-published profile preferences.
    route::restore_subtitle_size(SubtitleSize::Small);
    route::restore_subtitle_position(SubtitlePosition::High);
    player::restore_subtitle_tone(SubtitleTone::Grey);
    assert!(route::native_subtitle_preferences(&app.player.session).is_none());
    assert!(!old_source.matches(&app.player.session));

    deliver_model_saved(&mut app, candidate.clone());
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, candidate.settings);
    assert_eq!(route::subtitle_size(), SubtitleSize::Small);
    assert_eq!(route::subtitle_position(), SubtitlePosition::High);
    assert_eq!(player::subtitle_tone(), SubtitleTone::Grey);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
}

#[test]
fn addressed_content_requires_the_actual_emitter_to_own_the_request() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    let request = content_request_color(&mut app, TextColor::Yellow);
    let before = route::subtitle_text_appearance(&app.player.session);
    let saved_before = app.ott.as_ref().unwrap().saved.settings.clone();

    // Even a genuine pending request may not be forwarded by another emitter.
    through_content_from(
        &mut app,
        MachineId::Nav,
        PlayerReq::CommitTrackAddressed(request.clone()),
    );
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().subtitle_save.is_none());
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, saved_before);
    assert_eq!(route::subtitle_text_appearance(&app.player.session), before);
    assert!(bridge::player_overlay_mut(&mut app.pages)
        .unwrap()
        .accepts_track_commit(&request));

    // Refusing the wrong emitter must not consume the rightful owner's request.
    through_content(&mut app, PlayerReq::CommitTrackAddressed(request));
    assert!(app.ott.as_ref().unwrap().saving);
    let failure = capture_revoked_worker(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(failure)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(route::subtitle_text_appearance(&app.player.session), before);
}

#[path = "ott_subtitle_remote_tests.rs"]
mod remote;

fn protect_native_track_settings(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.entropy = Some([9; 16]);
    runtime.saved.settings.insert("parentPin".into(),json!(
        "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    runtime.saved.settings.insert("psOptions".into(),json!(1));
    runtime.parental_confirmed();
}

#[test]
fn direct_native_track_menu_admission_requires_current_parental_settings_access() {
    let _serial=plx_base::testlock::serial();
    let _session=plx_plex::plex::session::TempSession::new("ott-native-settings-protected");
    let _globals=StyleGlobals::capture();
    let mut app=native_settings_fixture();
    protect_native_track_settings(&mut app);
    let ticket=route::play_request_ticket();
    let mut runtime=app.ott.take().unwrap();
    assert!(native_controls::open(&mut runtime,&mut app,1).is_err());
    assert!(runtime.native_controls.is_none());
    assert!(!runtime.saving);
    assert!(route::play_request_is_current(ticket));
    app.ott=Some(runtime);
    settle_native_controls(&mut app);
    assert!(matches!(app.route(),AppArg::Ott));
    assert!(!bridge::player_overlay_up(&app.pages));
}

#[test]
fn revoked_parental_settings_access_retires_pending_native_track_menu() {
    let _serial=plx_base::testlock::serial();
    let _session=plx_plex::plex::session::TempSession::new("ott-native-settings-revoked");
    let _globals=StyleGlobals::capture();
    let mut app=native_settings_fixture();
    native_settings_action(&mut app,OttAction::EditSetting("native.subtitles".into()));
    assert!(app.ott.as_ref().unwrap().native_controls.is_some(),"must revoke while presentation is deferred");
    protect_native_track_settings(&mut app);
    let ticket=route::play_request_ticket();
    settle_native_controls(&mut app);
    let runtime=app.ott.as_ref().unwrap();
    assert!(runtime.native_controls.is_none());
    assert!(!bridge::player_overlay_up(&app.pages));
    assert!(!runtime.saving);
    assert!(route::play_request_is_current(ticket));
    assert!(runtime.status.contains("Unlock parental access"));
}

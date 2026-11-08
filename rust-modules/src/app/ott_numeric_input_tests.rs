//! Numeric keys enter through the real application queue; fixtures perform no media IO.
use super::*;
use crate::app::events;
use plx_machine::machine::{InputEvent, InputKind, Key, Source as InputSource, TextEdit};

fn torrent_number_row(id: &str, action: OttAction, title: &str) -> OttRow {
    OttRow {
        current_programme: None, content_title: Some(title.into()), guide: None,
        guide_available: false, records_available: false, media: None,
        id: id.into(), title: title.into(), detail: String::new(), description: String::new(),
        poster: None, action, enabled: true, favorite: false,
    }
}

fn torrent_number_view() -> OttView {
    let mut view = OttView::default();
    view.page_id = "torrent-files".into();
    view.rows = [4, 19].into_iter().map(|index| {
        let id = format!("torrent.file:23:lease-1:{index}");
        torrent_number_row(&id, OttAction::Play(id.clone()), "Film")
    }).collect();
    view
}

#[test]
fn torrent_numbers_use_visible_ordinals_and_exact_sparse_file_action() {
    let mut view = torrent_number_view();
    view.rows.insert(1, torrent_number_row("torrent.retry", OttAction::EditSetting("torrent.retry".into()), "Retry"));
    let rows = super::super::numeric_input::torrent_numbers(&view);
    assert_eq!(rows.resolve("1").unwrap().0, "torrent.file:23:lease-1:4");
    assert_eq!(rows.resolve("2").unwrap().0, "torrent.file:23:lease-1:19");
    assert_eq!(rows.resolve("3"), None);
    assert_eq!(rows.resolve("19"), None, "backend file index is not the visible number");
}

#[test]
fn torrent_numbers_do_not_renumber_disabled_content_or_adopt_other_pages() {
    let mut view = torrent_number_view();
    view.rows[0].enabled = false;
    let rows = super::super::numeric_input::torrent_numbers(&view);
    assert_eq!(rows.resolve("1"), None);
    assert_eq!(rows.resolve("2").unwrap().0, "torrent.file:23:lease-1:19");
    view.page_id = "channels".into();
    assert_eq!(super::super::numeric_input::torrent_numbers(&view).resolve("2"), None);
}

#[test]
fn torrent_result_number_preserves_action_prefix_instead_of_raw_row_id() {
    let id = "torznab:8:home:result-1";
    let action = format!("torrent:{id}");
    let mut view = OttView::default();
    view.page_id = "torrent-results".into();
    view.rows = vec![torrent_number_row(id, OttAction::Play(action.clone()), "Film")];
    assert_eq!(super::super::numeric_input::torrent_numbers(&view).resolve("1").unwrap().0, action);
    view.rows[0].action = OttAction::Play(id.into());
    assert_eq!(super::super::numeric_input::torrent_numbers(&view).resolve("1"), None);
}

#[test]
fn torrent_number_context_rejects_changed_session_selection_and_configuration() {
    let original = torrent_number_view();
    let settings = BTreeMap::new();
    let fingerprint = super::super::numeric_input::torrent_context(&original, &settings).unwrap();
    for change in 0..5 {
        let mut view = original.clone();
        let mut settings = settings.clone();
        match change {
            0 => { view.rows[0].id = "torrent.file:24:lease-2:4".into();
                view.rows[0].action = OttAction::Play(view.rows[0].id.clone()); }
            1 => view.rows[0].enabled = false,
            2 => view.rows.swap(0, 1),
            3 => { settings.insert("nativeCompanionRevision".into(), json!(3)); }
            _ => { settings.insert("nativeCompanion".into(), json!({"enabled":false})); }
        }
        assert_ne!(super::super::numeric_input::torrent_context(&view, &settings).unwrap(), fingerprint, "change {change}");
    }
    let mut progress = original;
    progress.status = "Buffering".into();
    progress.revision += 1;
    assert_eq!(super::super::numeric_input::torrent_context(&progress, &settings).unwrap(), fingerprint,
        "unchanged content does not gain a different identity from progress text");
}

fn mounted() -> App {
    let mut app = app_fixture();
    app.running = true;
    let profile = ProviderProfile { id: "numeric-fixture".into(), name: "Numeric".into(),
        kind: ProviderKind::M3u, source: "https://fixture.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    let catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",One\nhttps://fixture.invalid/one.ts\n\
         #EXTINF:-1 group-title=\"News\",Two\nhttps://fixture.invalid/two.m3u8\n\
         #EXTINF:-1 group-title=\"Film\",Three\nhttps://fixture.invalid/three.m3u8\n", &profile).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id);
    runtime.catalog = catalog;
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.continuity.stop(runtime.generation);
    runtime.page = "channels";
    runtime.sync_saved_context();
    runtime.changed();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app.inputs.clear();
    app
}

fn ingest(app: &mut App, bytes: [u8; 128]) {
    app.ev = bytes;
    let mut frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: clock::now(), dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "ott",
    };
    unsafe { run::ingest_sdl_event_with_window(app, &mut frame, |_| panic!("numeric input must not restore a window")); }
}

#[test]
fn numeric_channel_ingress_owns_down_and_up_without_starting_early() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let before = app.ott.as_ref().unwrap().navigation_epoch;
    ingest(&mut app, events::encode_key(b'2' as u32, 0, true));
    assert!(app.inputs.is_empty(), "channel number must enter the common numeric owner, not the legacy key queue");
    ingest(&mut app, events::encode_key(b'2' as u32, 0, false));
    assert!(app.inputs.is_empty(), "the numeric owner must retire its release");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none(), "a number must wait for the two-second multi-digit window");
    assert_eq!(runtime.navigation_epoch, before);
}

fn tap(app: &mut App, digit: u8) {
    ingest(app, events::encode_key(u32::from(digit), 0, true));
    ingest(app, events::encode_key(u32::from(digit), 0, false));
}
fn number_tick(app: &mut App, now: u64) {
    let mut runtime = app.ott.take().unwrap();
    super::super::numeric_input::tick(&mut runtime, app, now);
    app.ott = Some(runtime);
}
fn requested(app: &App) -> Option<String> {
    app.ott.as_ref().unwrap().play_target.as_ref().map(|target| target.id.clone())
}
fn item(app: &App, index: usize) -> String { app.ott.as_ref().unwrap().catalog.entries[index].id.clone() }

#[test]
fn numeric_channel_uses_canonical_resolver_at_deadline_exactly_once() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let start = app.ott.as_ref().unwrap().now_ms();
    let wanted = item(&app, 1);
    let navigation = app.ott.as_ref().unwrap().navigation_epoch;
    tap(&mut app, b'2');
    number_tick(&mut app, start + 1_999);
    assert_eq!(requested(&app), None);
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), Some(wanted));
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation + 1);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    number_tick(&mut app, start + 9_000);
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, epoch);
    assert!(app.ott.as_ref().unwrap().play_target.as_ref().unwrap().advance_on_failure,
        "numeric selection must retain the ordinary UI Play continuation policy");
}

#[test]
fn numeric_channel_category_keeps_original_catalogue_numbers() {
    let _lock = plx_base::testlock::serial();
    for (key, plays) in [(b'1', false), (b'3', true)] {
        let mut app = mounted();
        app.ott.as_mut().unwrap().saved.category = "Film".into();
        let mut runtime = app.ott.take().unwrap(); runtime.changed(); runtime.publish(&mut app);
        app.ott = Some(runtime); settle_pages(&mut app);
        let start = app.ott.as_ref().unwrap().now_ms();
        let wanted = item(&app, 2);
        tap(&mut app, key);
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), plays.then_some(wanted), "category rows must not be renumbered");
    }
}

#[test]
fn numeric_channel_repeat_does_not_append_or_extend_the_window() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let start = app.ott.as_ref().unwrap().now_ms();
    let wanted = item(&app, 1);
    ingest(&mut app, events::encode_key(b'2' as u32, 0, true));
    ingest(&mut app, events::encode_key_repeat(b'2' as u32, 0));
    ingest(&mut app, events::encode_key(b'2' as u32, 0, false));
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), Some(wanted));
    assert!(app.inputs.is_empty());
}

#[test]
fn numeric_channel_source_navigation_and_late_context_changes_cancel() {
    let _lock = plx_base::testlock::serial();
    for change in 0..4 {
        let mut app = mounted();
        let start = app.ott.as_ref().unwrap().now_ms();
        tap(&mut app, b'2');
        let runtime = app.ott.as_mut().unwrap();
        match change {
            0 => runtime.generation += 1,
            1 => runtime.navigation_epoch += 1,
            2 => runtime.catalog_revision += 1,
            _ => runtime.saved.category = "Film".into(),
        }
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), None, "stale number selected after context change {change}");
    }
}

#[test]
fn numeric_channel_other_input_at_expiry_wins_and_keeps_its_handler() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let start = app.ott.as_ref().unwrap().now_ms();
    tap(&mut app, b'2');
    ingest(&mut app, events::encode_key(plx_ui::consts::SDLK_DOWN, 0, true));
    assert!(!app.inputs.is_empty(), "ordinary navigation must retain its owner");
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), None);
}

#[test]
fn numeric_channel_leading_zero_does_not_begin_selection() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let start = app.ott.as_ref().unwrap().now_ms();
    tap(&mut app, b'0');
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), None);
    assert!(!app.inputs.is_empty(), "zero retains any existing transport/screen meaning");
}

fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

// Submit through the real editor so its pending owner accepts the Runtime ACK.
// Injecting SubmitSearch directly leaves the visible editor correctly open.
fn search_via_editor(app: &mut App, value: &str) {
    ui(app, OttAction::BeginSearch);
    assert!(bridge::text_owns_input(&app.pages));
    let tick = next_tick(app);
    let input = [TextEdit::Clear, TextEdit::Commit(value.into())].into_iter()
        .map(|edit| InputEvent { at: tick, source: InputSource::Script,
            kind: InputKind::Text(edit) }).collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    settle_pages(app);
    for key in [Key::Down, Key::Ok] {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
        runtime_tick(app);
        settle_pages(app);
    }
    assert!(app.ott.as_ref().unwrap().edit_session.is_none(), "Search must accept the submitted draft");
    assert!(!bridge::text_owns_input(&app.pages), "Search ACK must return input to the catalogue");
    assert_eq!(app.ott.as_ref().unwrap().search_view().unwrap().query, value);
}

#[test]
fn numeric_channel_search_keeps_canonical_numbers_and_excludes_hidden_rows() {
    let _lock = plx_base::testlock::serial();
    for (number, plays) in [(b'1', false), (b'3', true)] {
        let mut app = mounted();
        search_via_editor(&mut app, "Three");
        let start = app.ott.as_ref().unwrap().now_ms();
        let wanted = item(&app, 2);
        tap(&mut app, number);
        assert!(app.inputs.is_empty(), "the filtered catalogue must own both numeric edges");
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), plays.then_some(wanted));
    }
}

#[test]
fn numeric_channel_does_not_steal_editor_or_settings_digits() {
    let _lock = plx_base::testlock::serial();
    for action in [OttAction::BeginSearch, OttAction::ShowSettings] {
        let mut app = mounted();
        ui(&mut app, action);
        app.inputs.clear();
        tap(&mut app, b'2');
        assert!(!app.inputs.is_empty());
        let start = app.ott.as_ref().unwrap().now_ms();
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), None);
    }
}

struct ResetPlayer;
impl Drop for ResetPlayer {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn playing(live: bool) -> App {
    let mut app = mounted();
    let mut entry = app.ott.as_ref().unwrap().catalog.entries[0].clone();
    entry.duration_ms = if live { None } else { Some(60_000) };
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(&mut app.player.session,
        &plx_media::ff::inspect::MediaInfo {
            tracks: Default::default(), resolved_source: None, hls_playback: None,
            video_codec: "h264".into(), audio_codec: "aac".into(), width: 1280, height: 720,
            fps: 24.0, duration_ns: 60_000_000_000, audio_channels: 2, audio_sample_rate: 48_000,
            audio_bit_depth: 16, dovi: Default::default(),
        }, plx_media::ff::ExternalSource::Http("https://fixture.invalid/one.ts".into()),
        "One", "News", plx_platform::devcaps::assumed_shared()).unwrap();
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.saved.profiles.profiles[0].clone();
    runtime.selected = Some(entry.id.clone());
    runtime.playing = Some(Playing { channel_capture: None, kiosk_selection: None, archive: None, transient_owner: None,
        local_selection: None, identity: plx_media::route::playback_identity(&app.player.session),
        profile, selection_id: entry.id.clone(), entry,
        source: plx_media::route::external_source(&app.player.session), advance_on_failure: false,
        requires_server_lease: false });
    mount_player(&mut app);
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    plx_media::player::TX.commit_paused(false);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    app.inputs.clear();
    app
}

#[test]
fn numeric_channel_bare_live_selects_but_bare_vod_keeps_transport_digits() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(true);
    let start = app.ott.as_ref().unwrap().now_ms();
    let wanted = item(&app, 1);
    tap(&mut app, b'2');
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), Some(wanted));
    for digit in [b'0', b'4', b'6', b'7', b'8', b'9'] {
        let mut app = playing(false);
        let start = app.ott.as_ref().unwrap().now_ms();
        tap(&mut app, digit);
        assert!(!app.inputs.is_empty(), "VOD key {digit} lost its prior input owner");
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), None);
    }
}

#[test]
fn numeric_channel_favorites_take_precedence_over_current_vod() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(false);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites = plx_data::ott::favorites::Document::parse(br#"{"version":1,"entries":[
        {"title":"A film","origin":{"provider":"vportal","profile":"Films","sourceAlias":"Films","subprofile":null},"locator":{"kind":"vportal-media","idField":"id","id":"99"}},
        {"title":"Two","origin":{"provider":"m3u","profile":"Numeric","sourceAlias":"Numeric","subprofile":null},"locator":{"kind":"channel-name","name":"Two","group":"News"}}
    ]}"#).unwrap();
    runtime.saved.favorites_selected = true;
    runtime.page = "favorites";
    let wanted = format!("favorite:{}", runtime.saved.favorites.entries()[1].identity());
    let start = runtime.now_ms();
    tap(&mut app, b'2');
    assert!(app.inputs.is_empty(), "Favorites numeric owner must precede VOD shortcuts");
    number_tick(&mut app, start + 2_000);
    assert_eq!(app.ott.as_ref().unwrap().selected.as_deref(), Some(wanted.as_str()));
}

#[cfg(target_os = "macos")]
#[test]
fn numeric_channel_and_mute_keep_same_batch_order() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for number_first in [true, false] {
        let mut app = playing(true);
        let start = app.ott.as_ref().unwrap().now_ms();
        let mut m = events::encode_key(b'm' as u32, 0, true);
        m[8..12].copy_from_slice(&1_u32.to_ne_bytes()); // Physical Mac, not synthetic wcode.
        let mut up = m; up[0..4].copy_from_slice(&0x301_u32.to_ne_bytes()); up[12] = 0;
        if number_first { tap(&mut app, b'2'); }
        ingest(&mut app, m); ingest(&mut app, up);
        if !number_first { tap(&mut app, b'2'); }
        assert!(app.player.volume.desired().muted());
        settle_pages(&mut app); // Draining screen messages must not reverse input order.
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app).is_some(), !number_first);
    }
}

#[test]
fn numeric_channel_rejects_unrendered_replacement_until_snapshot_delivery() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.category = "Film".into(); runtime.changed();
    let start = runtime.now_ms();
    runtime.publish(&mut app); // queued; the old list still owns the visible screen
    app.ott = Some(runtime);
    tap(&mut app, b'3');
    assert!(!app.inputs.is_empty(), "new Runtime list is not yet the rendered input owner");
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), None);
    app.inputs.clear(); settle_pages(&mut app);
    tap(&mut app, b'3');
    assert!(app.inputs.is_empty());
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), Some(item(&app, 2)));
}

#[test]
fn numeric_channel_collects_four_digits_and_ignores_the_fifth() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let mut runtime = app.ott.take().unwrap();
    let template = runtime.catalog.entries[0].clone();
    runtime.catalog.entries = (1..=1_234).map(|number| {
        let mut row = template.clone(); row.id = format!("numeric-{number}");
        row.title = format!("Channel {number}"); row
    }).collect();
    runtime.catalog_revision += 1;
    runtime.changed(); runtime.publish(&mut app);
    let start = runtime.now_ms();
    app.ott = Some(runtime); settle_pages(&mut app);
    for digit in b"12345" { tap(&mut app, *digit); }
    number_tick(&mut app, start + 1_999);
    assert_eq!(requested(&app), None);
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app).as_deref(), Some("numeric-1234"));
}

#[test]
fn numeric_channel_late_kiosk_refusal_keeps_release_owned() {
    let _lock = plx_base::testlock::serial();
    let mut app = mounted();
    let start = app.ott.as_ref().unwrap().now_ms();
    ingest(&mut app, events::encode_key(b'2' as u32, 0, true));
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!(null));
    ingest(&mut app, events::encode_key(b'2' as u32, 0, false));
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), None);
    assert!(app.inputs.is_empty());
    assert_eq!(app.down_sym, 0);
}

#[test]
fn numeric_channel_archive_retains_transport_digits_despite_live_entry() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(true);
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.entry.live = Some(plx_data::ott::guide::ChannelMetadata {
        tvg_name: "One".into(), guide_shift_seconds: 0,
        catchup: Some(plx_data::ott::archive::CatchupSource {
            mode: plx_data::ott::archive::CatchupMode::Append,
            source: "?start={utc}&end={utcend}".into(), retention_seconds: 1000,
            correction_seconds: 0,
        }),
    });
    playing.archive = Some(archive_play::Intent::new(&playing.profile, &playing.entry,
        plx_data::ott::archive::ArchiveInterval { start: 1000, end: 1100 }, 1500).unwrap());
    assert!(guide_ui::is_live(&playing.profile, &playing.entry), "archive keeps original channel identity");
    let start = runtime.now_ms();
    for digit in b"046789" { tap(&mut app, *digit); }
    assert_eq!(app.inputs.len(), 12, "unconfigured archive digits keep their existing transport owner");
    number_tick(&mut app, start + 2_000);
    assert_eq!(requested(&app), None);
}


fn strict_numeric_pin(runtime: &mut Runtime) {
    let origin = json!({"provider":"m3u","profile":"Numeric","subprofile":null,"sourceAlias":"Numeric"});
    runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":true,"origin":origin,
        "target":{"kind":"item","entry":{"title":"One","origin":origin,
            "locator":{"kind":"channel","channelId":"numeric-one"}}}}));
    assert_eq!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true }));
}

#[test]
fn numeric_seek13_uses_existing_finite_seek_and_owns_release_without_channel_selection() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        for (digit, target) in [(b'1', 5_000_000_000), (b'3', 35_000_000_000)] {
            let mut app = playing(false);
            app.bridge.apply_ui_mode(mode);
            plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
            plx_media::player::SHARED.playpos_ns.store(20_000_000_000, Ordering::Relaxed);
            let source = plx_media::route::playback_identity(&app.player.session);
            let navigation = app.ott.as_ref().unwrap().navigation_epoch;
            tap(&mut app, digit);
            assert_eq!(plx_media::player::seek_pending(), target);
            assert!(app.inputs.is_empty(), "one numeric transport owner must consume both edges");
            assert_eq!(requested(&app), None);
            assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation);
            assert_eq!(plx_media::route::playback_identity(&app.player.session), source);
        }
    }
}

#[test]
fn numeric_seek13_repeat_does_not_repeat_and_retired_source_cannot_borrow_release() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(false);
    plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
    plx_media::player::SHARED.playpos_ns.store(20_000_000_000, Ordering::Relaxed);
    ingest(&mut app, events::encode_key(b'3' as u32, 0, true));
    assert_eq!(plx_media::player::seek_pending(), 35_000_000_000);
    ingest(&mut app, events::encode_key_repeat(b'3' as u32, 0));
    assert_eq!(plx_media::player::seek_pending(), 35_000_000_000);
    app.ott.as_mut().unwrap().playing = None;
    ingest(&mut app, events::encode_key(b'3' as u32, 0, false));
    assert!(app.inputs.is_empty(), "retired source still retires its held release");
    assert_eq!(app.down_sym, 0);
    tap(&mut app, b'3');
    assert!(!app.inputs.is_empty(), "a new press cannot borrow the former source");
    assert_eq!(plx_media::player::seek_pending(), 35_000_000_000);
}

#[test]
fn numeric_seek13_preserves_hud_editor_modal_write_kiosk_and_unknown_timeline_guards() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for blocked in 0..6 {
        let mut app = playing(false);
        plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
        plx_media::player::SHARED.playpos_ns.store(20_000_000_000, Ordering::Relaxed);
        match blocked {
            0 => {
                let now = clock::now();
                let hud = &mut bridge::player_mut(&mut app.pages).unwrap().hud;
                hud.note_fresh_press(&app.player.session, now, false);
                hud.extend(now, 60_000);
                assert!(hud.visible(&app.player.session, now, false));
            }
            1 => { app.ott.as_mut().unwrap().saving = true; }
            2 => { strict_numeric_pin(app.ott.as_mut().unwrap()); }
            3 => {
                let mut runtime = app.ott.take().unwrap();
                runtime.reply = app.pages.top_page();
                handle_action(&mut runtime, &mut app, OttAction::EditSetting("profile.name".into())).unwrap();
                assert!(runtime.edit_session.is_some());
                app.ott = Some(runtime);
            }
            4 => {
                bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(), &mut app.pages,
                    plx_screens::player::overlay::OverlayKind::More { page: None });
                settle_pages(&mut app);
                assert!(app.pages.surface_up());
                app.inputs.clear();
            }
            _ => { plx_media::player::SHARED.duration_ns.store(0, Ordering::Relaxed); }
        }
        tap(&mut app, b'3');
        assert_eq!(plx_media::player::seek_pending(), -1, "blocked case {blocked}");
        assert_eq!(requested(&app), None);
    }
}


#[test]
fn catalogue_volume_action_uses_shared_executor_and_preserves_pending_controls() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(false);
    let mut runtime = app.ott.take().unwrap();
    native_controls::open(&mut runtime, &mut app, 0).unwrap();
    assert!(runtime.native_controls.is_some());
    let navigation = runtime.navigation_epoch;
    let source = plx_media::route::playback_identity(&app.player.session);
    plx_media::player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
        plx_media::player::volume::DesiredVolume::new(50, false).unwrap());
    handle_action(&mut runtime, &mut app, OttAction::VolumeStep(1)).unwrap();
    assert_eq!(app.player.volume.desired().level(), 55);
    handle_action(&mut runtime, &mut app, OttAction::VolumeStep(-1)).unwrap();
    assert_eq!(app.player.volume.desired().level(), 50);
    assert!(runtime.native_controls.is_some(), "volume is not navigation or track-menu cancellation");
    assert_eq!(runtime.navigation_epoch, navigation);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), source);
    assert!(handle_action(&mut runtime, &mut app, OttAction::VolumeStep(0)).is_err());
    assert_eq!(app.player.volume.desired().level(), 50);
    app.ott = Some(runtime);
}

#[test]
fn catalogue_volume_action_rejects_editor_kiosk_and_absent_source_without_mutation() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for blocked in 0..3 {
        let mut app = playing(false);
        let mut runtime = app.ott.take().unwrap();
        runtime.reply = app.pages.top_page();
        match blocked {
            0 => {
                handle_action(&mut runtime, &mut app, OttAction::EditSetting("profile.name".into())).unwrap();
                assert!(runtime.edit_session.is_some());
            }
            1 => { strict_numeric_pin(&mut runtime); }
            _ => { runtime.playing = None; }
        }
        let editor = runtime.edit_session.as_ref().map(|editor| editor.id);
        let navigation = runtime.navigation_epoch;
        let volume = app.player.volume.desired();
        assert!(handle_action(&mut runtime, &mut app, OttAction::VolumeStep(1)).is_err());
        assert_eq!(app.player.volume.desired(), volume);
        assert_eq!(runtime.edit_session.as_ref().map(|editor| editor.id), editor);
        assert_eq!(runtime.navigation_epoch, navigation);
        app.ott = Some(runtime);
    }
}


#[test]
fn numeric_seek13_uses_accepted_duration_and_existing_timeline_clamps() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for (seconds, digit, target) in [(20, b'3', 40_000_000_000), (3600, b'3', 57_000_000_000), (3600, b'1', 0)] {
        let mut app = playing(false);
        app.ott.as_mut().unwrap().classic_preferences =
            plx_appkit::classic_settings::Preferences::from_values(25, 0, 0, seconds).unwrap();
        plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
        plx_media::player::SHARED.playpos_ns.store(20_000_000_000, Ordering::Relaxed);
        tap(&mut app, digit);
        assert_eq!(plx_media::player::seek_pending(), target);
        assert!(app.inputs.is_empty());
    }
}

#[test]
fn numeric_seek13_never_steals_live_or_mixed_favorites_number_entry() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for favorites in [false, true] {
        for digit in [b'1', b'3'] {
            let mut app = playing(!favorites);
            if favorites {
                let runtime = app.ott.as_mut().unwrap();
                runtime.saved.favorites_selected = true;
                runtime.page = "favorites";
            }
            tap(&mut app, digit);
            assert!(app.inputs.is_empty(), "the canonical numeric owner consumes this digit");
            assert_eq!(plx_media::player::seek_pending(), -1);
            assert_eq!(requested(&app), None, "number entry still waits for its deadline");
        }
    }
}

#[test]
fn numeric_seek13_archive_uses_finite_timeline_even_with_live_channel_metadata() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    let mut app = playing(true);
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.entry.live = Some(plx_data::ott::guide::ChannelMetadata {
        tvg_name: "One".into(), guide_shift_seconds: 0,
        catchup: Some(plx_data::ott::archive::CatchupSource {
            mode: plx_data::ott::archive::CatchupMode::Append,
            source: "?start={utc}&end={utcend}".into(), retention_seconds: 1000,
            correction_seconds: 0,
        }),
    });
    playing.archive = Some(archive_play::Intent::new(&playing.profile, &playing.entry,
        plx_data::ott::archive::ArchiveInterval { start: 1000, end: 1100 }, 1500).unwrap());
    plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
    plx_media::player::SHARED.playpos_ns.store(20_000_000_000, Ordering::Relaxed);
    tap(&mut app, b'3');
    assert_eq!(plx_media::player::seek_pending(), 35_000_000_000);
    assert!(app.inputs.is_empty());
    assert_eq!(requested(&app), None);
}


fn classic_list_mode(app: &mut App, mode: plx_machine::machine::UiMode) {
    let mut runtime = app.ott.take().unwrap();
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(if mode == plx_machine::machine::UiMode::Classic { 0 } else { 1 }));
    app.bridge.apply_ui_mode(mode);
    runtime.changed();
    runtime.publish(app);
    app.ott = Some(runtime);
    settle_pages(app);
}

#[test]
fn classic_list_number_selects_visible_category_and_search_position() {
    let _lock = plx_base::testlock::serial();
    for filtered in [false, true] {
        let mut app = mounted();
        classic_list_mode(&mut app, plx_machine::machine::UiMode::Classic);
        if filtered { search_via_editor(&mut app, "Three"); }
        else {
            app.ott.as_mut().unwrap().saved.category = "Film".into();
            classic_list_mode(&mut app, plx_machine::machine::UiMode::Classic);
        }
        let wanted = item(&app, 2);
        let canonical = commands::numbered_channels(app.ott.as_ref().unwrap());
        assert_eq!(canonical.resolve("3").unwrap().0, wanted, "remote/canonical projection stays unchanged");
        assert!(canonical.resolve("1").is_none());
        let start = app.ott.as_ref().unwrap().now_ms();
        tap(&mut app, b'1');
        assert!(app.inputs.is_empty());
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), Some(wanted));
    }
}

#[test]
fn classic_list_number_uses_current_organized_order_and_rejects_changed_owner() {
    let _lock = plx_base::testlock::serial();
    for change in [None, Some("mode"), Some("category"), Some("order")] {
        let mut app = mounted();
        let wanted = item(&app, 2);
        {
            let runtime = app.ott.as_mut().unwrap();
            let profile = runtime.effective_profile().unwrap().clone();
            let source = catalog_organization::Source::new(&profile).unwrap();
            let mut document = catalog_organization::Document::read(&runtime.saved.settings).unwrap();
            // Alphabetical order places Three ahead of Two without changing raw indices.
            document.apply(&source, &runtime.catalog, catalog_organization::Operation::SetAlphabetical { enabled: true }).unwrap();
            document.write_into(&mut runtime.saved.settings).unwrap();
        }
        classic_list_mode(&mut app, plx_machine::machine::UiMode::Classic);
        let start = app.ott.as_ref().unwrap().now_ms();
        tap(&mut app, b'2');
        match change {
            Some("mode") => classic_list_mode(&mut app, plx_machine::machine::UiMode::Modern),
            Some("category") => {
                app.ott.as_mut().unwrap().saved.category = "News".into();
                classic_list_mode(&mut app, plx_machine::machine::UiMode::Classic);
            }
            Some("order") => { app.ott.as_mut().unwrap().catalog_revision += 1; }
            _ => {}
        }
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), if change.is_none() { Some(wanted) } else { None }, "{change:?}");
    }
}

#[test]
fn classic_mode_bare_video_keeps_canonical_category_numbers() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetPlayer;
    for (number, plays) in [(b'1', false), (b'3', true)] {
        let mut app = playing(true);
        app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
        app.ott.as_mut().unwrap().saved.category = "Film".into();
        let start = app.ott.as_ref().unwrap().now_ms();
        let wanted = item(&app, 2);
        tap(&mut app, number);
        number_tick(&mut app, start + 2_000);
        assert_eq!(requested(&app), plays.then_some(wanted));
    }
}


fn classic_categories() -> App {
    let mut app = mounted();
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.saved.profiles.profiles[0].clone();
    let mut m3u = String::from("#EXTM3U\n");
    for index in 0..10 {
        m3u.push_str(&format!("#EXTINF:-1 group-title=\"Category {index}\",Channel {index}\nhttps://fixture.invalid/{index}.ts\n"));
    }
    runtime.catalog = plx_data::ott::parse_m3u(&m3u, &profile).unwrap();
    // Replacing the mounted catalogue must retire its cached organization projection.
    runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
    runtime.entropy = Some([7; 16]);
    runtime.sync_saved_context();
    app.ott = Some(runtime);
    classic_list_mode(&mut app, plx_machine::machine::UiMode::Classic);
    let mut runtime = app.ott.take().unwrap();
    apply_ui_action(&mut runtime, &mut app, OttAction::ShowCategories, true, false, false);
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    settle_pages(&mut app);
    app.inputs.clear();
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    assert!(!app.ott.as_ref().unwrap().saving);
    app
}

#[test]
fn classic_category_digits_enter_real_selection_once_for_all_favorites_and_ninth() {
    fn diagnostic(app: &App, number: u8) -> String {
        let runtime = app.ott.as_ref().unwrap();
        let target = bridge::ott_ui_target(&app.pages);
        let screen = target.and_then(|instance| app.pages.nav.instance(instance))
            .and_then(|instance| instance.screen.as_any())
            .and_then(|screen| screen.downcast_ref::<plx_screens::ott::OttScreen>());
        // Read-only fixture diagnostics; the sibling numeric owner's held set
        // stays private; report down_sym and the queue without inferring that set.
        format!(
            "digit={} page={:?} revision={} generation={} navigation={} reply={:?} target={:?} \
             reply_matches={} mode={:?} screen_context={:?} category_action={:?} \
             running={} playback_allowed={} loaded={} uncertain={} saving={} edit={} setup={} \
             clock={} play_work_allowed={} navigation_policy={:?} pending_navigation={} \
             pointer_arm={} press_active={} down_sym={} down_matches_digit={} pending_category={} status={:?} input_queue={:?}",
            char::from(number), runtime.page, runtime.revision, runtime.generation,
            runtime.navigation_epoch, runtime.reply, target,
            target.is_some() && runtime.reply == target, runtime.interface_mode(),
            screen.map(|screen| screen.channel_number_context("categories", runtime.revision)),
            screen.and_then(|screen| screen.category_number_action(number - b'0')),
            app.running, playback_allowed(app), runtime.loaded, runtime.persistence_uncertain,
            runtime.saving, runtime.edit_session.is_some(), runtime.setup.is_some(),
            runtime.clock_sample.is_some(), runtime.play_work_allowed(), crate::app::ott_runtime::input_policy::navigation(runtime),
            app.pages.has_pending_navigation(), app.pages.input.arm.is_some(),
            app.pages.input.press.is_active(), app.down_sym, app.down_sym == u32::from(number),
            runtime.pending_category_choice.is_some(),
            runtime.status, app.inputs,
        )
    }

    let _serial = plx_base::testlock::serial();
    for number in [b'1', b'2', b'9'] {
        let mut app = classic_categories();
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let generation = app.ott.as_ref().unwrap().play_generation;
        let navigation = app.ott.as_ref().unwrap().navigation_epoch;
        let before_input = diagnostic(&app, number);
        ingest(&mut app, events::encode_key(u32::from(number), 0, true));
        let after_down = diagnostic(&app, number);
        let admitted_navigation = app.ott.as_ref().unwrap().navigation_epoch;
        assert_eq!(admitted_navigation, navigation + 1, "before: {before_input}\nafter Down: {after_down}");
        assert!(app.ott.as_ref().unwrap().saving, "before: {before_input}\nafter Down: {after_down}");
        assert!(app.ott.as_ref().unwrap().pending_category_choice.is_some(),
            "before: {before_input}\nafter Down: {after_down}");
        ingest(&mut app, events::encode_key_repeat(u32::from(number), 0));
        ingest(&mut app, events::encode_key(u32::from(number), 0, false));
        let after_keys = diagnostic(&app, number);
        assert!(app.inputs.is_empty(), "before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, admitted_navigation,
            "before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
        assert_eq!(app.ott.as_ref().unwrap().play_generation, generation,
            "before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
        assert_eq!(requested(&app), None,
            "before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before,
            "selection remains behind the ordinary durable acknowledgement; before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
        // Join the existing revoked writer's completion; never write fixture data.
        let job = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("writer completion: {error}; before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}"));
        assert!(matches!(job, Job::Saved(Err(_), None, false, _)
            | Job::PublicSaved(Err(_), None, _)),
            "before: {before_input}\nafter Down: {after_down}\nafter Repeat/Up: {after_keys}");
    }
}

#[test]
fn classic_category_raw_digits_preserve_modifiers_modern_and_stale_render_guards() {
    let _serial = plx_base::testlock::serial();
    for boundary in 0..5 {
        let mut app = classic_categories();
        let mut bytes = events::encode_key(b'1' as u32, 0, true);
        match boundary {
            0 => { bytes[8..12].copy_from_slice(&1u32.to_ne_bytes()); bytes[24..26].copy_from_slice(&1u16.to_ne_bytes()); }
            1 => classic_list_mode(&mut app, plx_machine::machine::UiMode::Modern),
            2 => app.ott.as_mut().unwrap().revision += 1,
            3 => { bytes = events::encode_key(b'0' as u32, 0, true); }
            _ => { app.ott.as_mut().unwrap().navigation_epoch += 1; }
        }
        let navigation = app.ott.as_ref().unwrap().navigation_epoch;
        ingest(&mut app, bytes);
        assert!(!app.ott.as_ref().unwrap().saving, "boundary {boundary}");
        assert!(app.ott.as_ref().unwrap().pending_category_choice.is_none());
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation);
        assert_eq!(requested(&app), None);
    }
}


// Keep whole-Runtime move temporaries out of the test frame while its nested
// App fixture is constructed, as in lifecycle::assert_initial_retry_stays_blocked.
#[inline(never)]
fn seed_numeric_category_favorites(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.saved.profiles.profiles[0].clone();
    let local = selection::capture(&profile, &runtime.catalog.entries[0]).unwrap();
    let other_provider = Favorite {
        title: "Library film".into(),
        origin: plx_data::ott::favorites::Origin { provider: "plex".into(),
            profile: "Library".into(), subprofile: None, source_alias: "Server".into() },
        locator: plx_data::ott::favorites::Locator::Plex {
            server_id: "synthetic-server".into(), rating_key: "42".into() },
    };
    runtime.saved.favorites.toggle(local).unwrap();
    runtime.saved.favorites.toggle(other_provider).unwrap();
    runtime.changed();
    runtime.publish(app);
    app.ott = Some(runtime);
    settle_pages(app);
}

#[inline(never)]
fn reopen_numeric_categories(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    apply_ui_action(&mut runtime, app, OttAction::ShowCategories, true, false, false);
    runtime.publish(app);
    app.ott = Some(runtime);
    settle_pages(app);
}

/// Model only the existing Saved-success settlement boundary. The real writer
/// remains revoked: this test does not claim a durable write. Hosted coverage
/// must separately accept an actual writer result and reopen persisted state.
#[test]
fn modeled_successful_save_settles_classic_category_digits_and_global_favorites() {
    let _serial = plx_base::testlock::serial();
    let mut app = classic_categories();
    seed_numeric_category_favorites(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let profiles = runtime.saved.profiles.clone();
    let favorites = runtime.saved.favorites.clone();
    let favorite_ids: Vec<_> = favorites.entries().iter()
        .map(|favorite| format!("favorite:{}", favorite.identity())).collect();
    let all_ids: Vec<_> = runtime.catalog.entries.iter().map(|entry| entry.id.clone()).collect();
    assert_eq!(all_ids.len(), 10);
    assert!(runtime.organization_projection().is_some(), "use the actual saved-provider projection");
    let ninth_category = runtime.organization_categories().into_iter()
        .find(|category| category.title == "Category 6").unwrap().id;
    let ninth_item = runtime.catalog.entries.iter()
        .find(|entry| entry.title == "Channel 6").unwrap().id.clone();
    let generation = runtime.generation;
    let catalog_revision = runtime.catalog_revision;
    let play_generation = runtime.play_generation;
    let playback_epoch = runtime.playback_epoch;

    // Ninth -> Favorites -> All makes every committed transition observable.
    // All/Favorites occupy the first two visible rows in this default fixture.
    for digit in [b'9', b'2', b'1'] {
        reopen_numeric_categories(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        let view = runtime.view();
        assert_eq!(runtime.page, "categories");
        assert_eq!(view.rows[0].id, "all");
        assert_eq!(view.rows[1].id, "favorites");
        assert_eq!(view.rows[8].id, ninth_category);
        assert_eq!(view.rows[8].title, "Category 6");
        let owner = bridge::ott_ui_target(&app.pages).unwrap();
        assert_eq!(runtime.reply, Some(owner));
        let screen = app.pages.nav.instance(owner).unwrap().screen.as_any().unwrap()
            .downcast_ref::<plx_screens::ott::OttScreen>().unwrap();
        assert!(screen.channel_number_context(&view.page_id, view.revision));
        assert!(screen.category_number_action(digit - b'0').is_some());
        let category = match digit { b'9' => Some(ninth_category.as_str()), b'2' => None, _ => Some("") };
        let candidate = runtime.category_choice_candidate(category).unwrap();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let navigation = runtime.navigation_epoch;
        let old_category = runtime.saved.category.clone();

        ingest(&mut app, events::encode_key(u32::from(digit), 0, true));
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation + 1);
        ingest(&mut app, events::encode_key_repeat(u32::from(digit), 0));
        ingest(&mut app, events::encode_key(u32::from(digit), 0, false));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.navigation_epoch, navigation + 1, "one digit, one selection");
        assert!(app.inputs.is_empty());
        assert!(runtime.saving && runtime.pending_category_choice.is_some());
        assert_eq!(runtime.page, "categories", "do not leave before settlement");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(requested(&app), None);
        // Same explicit modeling seam as category_profile::stage/deliver. Join
        // the revoked worker first; no successful persistence is inferred from it.
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Job::Saved(Err(_), None, false, _)));
        assert!(runtime.rx.try_recv().is_err(), "no repeated writer request");
        runtime.tx.send(Job::Saved(Ok(candidate), None, false, json!({"status":"ok"})))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        settle_pages(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving && runtime.pending_category_choice.is_none());
        assert_eq!(runtime.navigation_epoch, navigation + 1);
        assert_eq!(runtime.saved.profiles, profiles);
        assert_eq!(runtime.saved.favorites, favorites, "selection never rewrites the global document");
        assert_eq!((runtime.generation, runtime.catalog_revision, runtime.play_generation, runtime.playback_epoch),
            (generation, catalog_revision, play_generation, playback_epoch));
        assert!(runtime.playing.is_none());
        assert_eq!(requested(&app), None, "category choice never autoplays");
        let view = runtime.view();
        let ids: Vec<_> = view.rows.iter().filter_map(|row| match &row.action {
            OttAction::Play(id) => Some(id.clone()), _ => None,
        }).collect();
        match digit {
            b'9' => {
                assert_eq!(runtime.page, "channels");
                assert_eq!(runtime.saved.category, ninth_category);
                assert!(!runtime.saved.favorites_selected);
                assert_eq!(ids, vec![ninth_item.clone()]);
            }
            b'2' => {
                assert_eq!(runtime.page, "favorites");
                assert!(runtime.saved.favorites_selected);
                assert_eq!(runtime.saved.category, old_category);
                assert_eq!(ids, favorite_ids, "include both active M3U and foreign Plex origin");
                assert_eq!(view.rows[0].detail, "m3u · Numeric");
                assert_eq!(view.rows[1].detail, "plex · Library");
            }
            _ => {
                assert_eq!(runtime.page, "channels");
                assert!(!runtime.saved.favorites_selected);
                assert!(runtime.saved.category.is_empty());
                assert_eq!(ids, all_ids);
            }
        }
        let screen = app.pages.nav.instance(owner).unwrap().screen.as_any().unwrap()
            .downcast_ref::<plx_screens::ott::OttScreen>().unwrap();
        assert_eq!(bridge::ott_ui_target(&app.pages), Some(owner));
        assert!(screen.channel_number_context(&view.page_id, view.revision),
            "the acknowledged view reaches the same actual mounted screen");
        assert!(screen.accepts_channel_number());
    }
}

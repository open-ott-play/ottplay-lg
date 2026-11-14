//! Inventory projection and keyed refresh, not a decoder test.
use super::*;
use plx_media::route;
use plx_data::{metadata, stores::metadata::{MetadataStore, MetadataCmd}};

fn store(audio: usize, subtitles: usize) -> MetadataStore {
    let mut item = metadata::PlayingItem::with_subs((0..subtitles).map(|i| metadata::Stream {
        id: i as i64 + 10, index: i as i64, codec: "srt".into(), ..Default::default()
    }).collect());
    item.audio = (0..audio).map(|i| metadata::Stream {
        id: i as i64 + 1, index: i as i64, codec: "aac".into(), ..Default::default()
    }).collect();
    let mut store = MetadataStore::default();
    store.run(MetadataCmd::InstallPlaying(Some(item)));
    store
}
fn plex() -> route::PlaybackSession {
    let mut ps = route::PlaybackSession::IDLE;
    route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
    // Production publishes the request before resolve; apply_plan retains it with the landing.
    route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
    ps
}
#[test]
fn playing_inventory_controls_the_two_menu_entries_independently() {
    let _lock = plx_base::testlock::serial();
    let ps = plex();
    for (audio, subs, expected) in [(0, 0, (false, false)), (1, 1, (false, true)), (2, 0, (true, false)), (2, 1, (true, true))] {
        let meta = store(audio, subs);
        let value = TrackMenuAvailability::for_playback(&ps, meta.view());
        assert_eq!((value.audio, value.subtitles), expected);
    }
    let meta = store(2, 1);
    assert_eq!(TrackMenuAvailability::for_playback(&route::PlaybackSession::IDLE, meta.view()), TrackMenuAvailability::default(),
        "unbound old Plex metadata is not a current inventory");
    let mut installed_only = route::PlaybackSession::IDLE;
    route::install_plex_playback_for_test(&mut installed_only, plx_plex::plex::ServerId::from_raw(0), "rk");
    assert_eq!(TrackMenuAvailability::for_playback(&installed_only, meta.view()), TrackMenuAvailability::default(),
        "an installed identity alone does not establish the current request");
    let mut replacing = plex();
    route::install_plex_request_for_test(&mut replacing, plx_plex::plex::ServerId::from_raw(0), "replacement");
    assert_eq!(TrackMenuAvailability::for_playback(&replacing, meta.view()), TrackMenuAvailability::default(),
        "an incoming request retires the previous inventory before its plan lands");
}
#[test]
fn supplied_inventory_refresh_preserves_keys_and_conjoins_menu_preferences() {
    let _lock = plx_base::testlock::serial();
    struct Restore(crate::menu_visibility::Visibility);
    impl Drop for Restore { fn drop(&mut self) { crate::menu_visibility::project(self.0); } }
    let _restore = Restore(crate::menu_visibility::projected());
    crate::menu_visibility::project(Default::default());
    let ps = plex();
    let meta = store(2, 1);
    let mut menu = MoreMenuState::new(&ps).with_track_availability(&ps, TrackMenuAvailability::for_playback(&ps, meta.view()));
    let audio = Action::OpenTracks(crate::player_hud::command::TrackTab::Audio);
    let subs = Action::OpenTracks(crate::player_hud::command::TrackTab::Subtitles);
    assert!(menu.focus_key(audio.key().0));
    let empty = MetadataStore::default();
    assert!(menu.set_track_availability(&ps, TrackMenuAvailability::for_playback(&ps, empty.view())));
    assert!(!menu.focus_key(audio.key().0), "removed key cannot activate a neighbour");
    assert!(!menu.focus_key(subs.key().0));
    assert!(!menu.set_track_availability(&ps, TrackMenuAvailability::default()), "unchanged empty inventory is idle");
    assert!(menu.set_track_availability(&ps, TrackMenuAvailability::for_playback(&ps, meta.view())));
    assert!(menu.focus_key(audio.key().0));
    let mut hidden = crate::menu_visibility::Visibility::default(); hidden.hide(crate::menu_visibility::Item::Audio);
    crate::menu_visibility::project(hidden); menu.refresh(&ps);
    assert!(!menu.focus_key(audio.key().0)); assert!(menu.focus_key(subs.key().0));
}
#[test]
fn hls_audio_choices_ignore_unrelated_plex_subtitles() {
    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let playback = plx_media::ff::hls_presentation::HlsPlayback::fixture_for_test();
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &playback.media_info_for_test(),
        plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    let meta = store(3, 2);
    let value = TrackMenuAvailability::for_playback(&ps, meta.view());
    assert!(value.audio); assert!(!value.subtitles);
    assert!(!TrackMenuAvailability::from_counts(1, 0).audio);
    assert!(TrackMenuAvailability::from_counts(2, 0).audio);
}

#[test]
fn actual_more_opens_only_available_hls_subtitle_inventory() {
    use crate::player_hud::command::TrackTab;
    use crate::track_menu::{TrackMenuState, TrackRow};
    use plx_media::ff::hls_presentation::HlsPlayback;

    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    struct Restore(crate::menu_visibility::Visibility);
    impl Drop for Restore { fn drop(&mut self) { crate::menu_visibility::project(self.0); } }
    let _restore = Restore(crate::menu_visibility::projected());
    crate::menu_visibility::project(Default::default());
    let meta = store(3, 2);
    let [without_clock, bad_uri] = HlsPlayback::unavailable_subtitle_fixtures_for_test();
    for (case, playback, with_subtitles) in [
        ("no subtitle group", HlsPlayback::fixture_for_test(), false),
        ("declared subtitles without clock", without_clock, false),
        ("declared subtitles with bad URI", bad_uri, false),
        ("valid subtitle candidates", HlsPlayback::fixture_with_subtitles_for_test(), true),
    ] {
        let mut ps = route::PlaybackSession::IDLE;
        route::reset_player_control_for_test(&ps);
        route::request_external_source(&mut ps, &playback.media_info_for_test(),
            plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
            "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
        route::reset_player_control_for_test(&ps);
        let catalog = route::hls_track_catalog(&ps).unwrap();
        assert_eq!(catalog.subtitle_rows().is_empty(), !with_subtitles, "{case}");
        let identity = route::playback_identity(&ps);
        let worker = route::worker_ticket();
        let mut menu = MoreMenuState::new(&ps)
            .with_track_availability(&ps, TrackMenuAvailability::for_playback(&ps, meta.view()));
        let audio = Action::OpenTracks(TrackTab::Audio);
        assert!(menu.focus_key(audio.key().0));
        assert_eq!(menu.on_ok(&ps), MoreOk::Action(audio));
        let subtitles = Action::OpenTracks(TrackTab::Subtitles);
        assert_eq!(menu.focus_key(subtitles.key().0), with_subtitles,
            "{case}: More must offer the current HLS inventory, never retained Plex subtitles");
        if with_subtitles {
            let MoreOk::Action(Action::OpenTracks(tab)) = menu.on_ok(&ps) else {
                panic!("More must emit the actual track-panel entry action");
            };
            assert_eq!(tab, TrackTab::Subtitles);
            let mut tracks = TrackMenuState::new(&ps, meta.view(), tab.index(), Vec::new());
            assert!(tracks.focus_id(TrackRow::SubOff));
            assert!(tracks.focus_id(TrackRow::HlsSubtitle(0)));
            assert!(tracks.focus_id(TrackRow::Style));
        }
        assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
        assert_eq!(route::playback_identity(&ps), identity);
        assert_eq!(route::worker_ticket(), worker);
    }
}

#[test]
fn external_empty_inventory_never_borrows_old_plex_tracks() {
    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut info = plx_media::ff::hls_presentation::HlsPlayback::fixture_for_test().media_info_for_test();
    info.hls_playback = None;
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &info,
        plx_media::ff::ExternalSource::Http("https://example.invalid/movie.mp4".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    assert!(route::external_tracks(&ps).is_some());
    let meta = store(3, 2);
    assert_eq!(TrackMenuAvailability::for_playback(&ps, meta.view()), TrackMenuAvailability::default());
}

#[test]
fn local_sidecar_discovery_adds_subtitles_without_borrowing_plex() {
    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut info = plx_media::ff::hls_presentation::HlsPlayback::fixture_for_test().media_info_for_test();
    info.hls_playback = None;
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &info,
        plx_media::ff::ExternalSource::Local(std::path::PathBuf::from("/fixture/movie.mp4")),
        "Fixture", "Local", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    let source = route::playback_identity(&ps).unwrap();
    let catalog = route::external_tracks(&ps).unwrap().clone();
    let meta = store(3, 0);
    assert_eq!(TrackMenuAvailability::for_playback(&ps, meta.view()), TrackMenuAvailability::default());
    route::install_external_local_subtitles(&mut ps, &source, &catalog,
        vec![route::LocalSubtitleTrack { target: route::LocalSubtitleTarget::new(),
            format: plx_media::player::sidecar::SidecarFormat::Srt,
            title: "English".into(), language: "en".into() }], true, 128, 64).unwrap();
    let offered = TrackMenuAvailability::for_playback(&ps, meta.view());
    assert!(!offered.audio); assert!(offered.subtitles);
}

fn live_embedded_audio(count: usize) -> route::PlaybackSession {
    use plx_media::ff::{hls_observed::{Scope, Snapshot}, hls_presentation::HlsPlayback};
    let playback = HlsPlayback::fixture_live_with_subtitles_for_test();
    let scope = Scope::new("https://example.invalid/master.m3u8",
        "https://example.invalid/video.m3u8", 1, 0).unwrap();
    let observed = Snapshot::fixture_for_test(scope);
    let mut info = playback.media_info_for_test();
    info.tracks = observed.inventory().clone();
    info.tracks.audio.truncate(count);
    for (row, language) in info.tracks.audio.iter_mut().zip(["eng", "fra"]) {
        row.language = language.into();
    }
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &info,
        plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    ps
}

#[test]
fn more_live_hls_uses_embedded_audio_and_manifest_subtitle_choices_from_the_actual_picker() {
    use crate::{player_hud::command::TrackTab, track_menu::{TrackCommit, TrackMenuState, TrackOk, TrackRow}};
    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let mut ps = live_embedded_audio(2);
    let meta = store(5, 5); // Unrelated retained Plex metadata must not supply these choices.
    let catalog = route::hls_track_catalog(&ps).unwrap();
    assert!(catalog.rows().is_empty(), "live fixture has no separate manifest audio renditions");
    assert_eq!(catalog.embedded_tracks().unwrap().inventory().audio.len(), 2);
    let available = TrackMenuAvailability::for_playback(&ps, meta.view());
    assert!(available.audio && available.subtitles);
    let mut more = MoreMenuState::new(&ps).with_track_availability(&ps, available);
    let audio = Action::OpenTracks(TrackTab::Audio);
    assert!(more.focus_key(audio.key().0));
    assert_eq!(more.on_ok(&ps), MoreOk::Action(audio));
    let mut tracks = TrackMenuState::new(&ps, meta.view(), TrackTab::Audio.index(), Vec::new());
    assert_eq!(tracks.active_audio(), 0);
    assert!(tracks.focus_id(TrackRow::Audio(1)));
    let TrackOk::Commit { commit: TrackCommit::External(choice), .. } = tracks.on_ok_addressed(meta.view())
        else { panic!("the offered Audio menu must produce an actual embedded-track choice") };
    assert_eq!(route::apply_external_track(&mut ps, &choice), Ok(true));
    let selected = route::external_tracks(&ps).unwrap();
    assert_eq!(selected.selected_audio(), Some(1));
    assert_eq!(selected.inventory().audio[1].language, "fra");
    assert_eq!(TrackMenuState::new(&ps, meta.view(), 0, Vec::new()).active_audio(), 1);
    // Settle the inert native audio-reload request before selecting subtitles;
    // this fixture does not run an engine or consume its control queue.
    route::reset_player_control_for_test(&ps);

    let subtitles = Action::OpenTracks(TrackTab::Subtitles);
    assert!(more.focus_key(subtitles.key().0));
    assert_eq!(more.on_ok(&ps), MoreOk::Action(subtitles));
    tracks = TrackMenuState::new(&ps, meta.view(), TrackTab::Subtitles.index(), Vec::new());
    assert!(tracks.focus_id(TrackRow::HlsSubtitle(1)));
    let TrackOk::Commit { commit: TrackCommit::Hls(choice), .. } = tracks.on_ok_addressed(meta.view())
        else { panic!("the offered subtitle menu must produce a real manifest choice") };
    let request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
    let prepared = request.prepared_for_test();
    assert_eq!(route::complete_hls_track_request(&mut ps, &request, prepared), Ok(true));
    let catalog = route::hls_track_catalog(&ps).unwrap();
    assert!(catalog.subtitle_rows()[1].selected);
    assert_eq!(catalog.subtitle_rows()[1].language, "es");
}

#[test]
fn more_refreshes_live_embedded_inventory_without_borrowing_retired_track_addresses() {
    use crate::{player_hud::command::TrackTab, track_menu::{TrackCommit, TrackMenuState, TrackOk, TrackRow}};
    let _lock = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let meta = store(5, 5);
    let initial = live_embedded_audio(1);
    let mut more = MoreMenuState::new(&initial)
        .with_track_availability(&initial, TrackMenuAvailability::for_playback(&initial, meta.view()));
    let audio = Action::OpenTracks(TrackTab::Audio);
    assert!(!more.focus_key(audio.key().0), "one embedded audio track offers no alternative language");
    let updated = live_embedded_audio(2);
    assert!(more.set_track_availability(&updated, TrackMenuAvailability::for_playback(&updated, meta.view())));
    assert!(more.focus_key(audio.key().0));
    let mut tracks = TrackMenuState::new(&updated, meta.view(), 0, Vec::new());
    assert!(tracks.focus_id(TrackRow::Audio(1)));
    let TrackOk::Commit { commit: TrackCommit::External(stale), .. } = tracks.on_ok_addressed(meta.view())
        else { panic!("actual addressed audio choice") };
    let mut replacement = live_embedded_audio(0);
    assert!(route::apply_external_track(&mut replacement, &stale).is_err(),
        "same URL cannot transfer an old track choice to replacement playback");
    assert!(more.set_track_availability(&replacement,
        TrackMenuAvailability::for_playback(&replacement, meta.view())));
    assert!(!more.focus_key(audio.key().0));
    assert_eq!(route::external_tracks(&replacement).unwrap().selected_audio(), None);
}

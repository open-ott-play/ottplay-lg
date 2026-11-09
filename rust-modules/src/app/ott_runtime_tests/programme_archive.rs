//! Real owned-playback footer publication; no resolver, device or renderer runs.
use super::*;
use plx_data::ott::{archive::{ArchiveInterval, CatchupMode, CatchupSource}, epg::Programme, guide::ChannelMetadata};

fn archive_fixture(with_metadata: bool) -> App {
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.wall_clock_ms = 1_700_000_000_000;
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile.kind = ProviderKind::M3u;
    playing.profile.source = "https://media.invalid/live.m3u".into();
    playing.entry.title = "Fixture channel".into();
    playing.entry.description = "Unrelated currently live programme".into();
    playing.entry.live = Some(ChannelMetadata { guide_id: None, guide_feed: String::new(),
        tvg_name: "Fixture channel".into(), guide_shift_seconds: 0,
        catchup: Some(CatchupSource { mode: CatchupMode::Append,
            source: "?start={utc}&duration={duration}".into(),
            retention_seconds: 86400, correction_seconds: 0 }),
    });
    let interval = ArchiveInterval { start: 1_699_999_400, end: 1_699_999_700 };
    let mut intent = archive_play::Intent::new(&playing.profile, &playing.entry, interval, 1_700_000_000).unwrap();
    if with_metadata {
        intent = intent.with_programme(&Programme { channel: "epg-one".into(), start: interval.start, end: interval.end,
            title: "Archived bulletin".into(), description: "Complete archived description. Second sentence retained.".into() }).unwrap();
    }
    playing.archive = Some(intent);
    app
}

#[test]
fn archive_footer_uses_retained_programme_and_never_live_successor() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(true);
    publish(&mut app);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title, "Archived bulletin");
    assert!(now.summary.ends_with("Second sentence retained."));
    assert_eq!(now.ott.as_ref().unwrap().collection_label, "Fixture channel");
    assert!(now.ott.as_ref().unwrap().next.is_none());
    assert!(now.ott.as_ref().unwrap().confirmed_start_unix_ms.is_none());
    assert!(now.ott.as_ref().unwrap().art.is_some());
    assert_eq!(now.dur_ms, 600000, "confirmed source duration wins over the scheduled interval");
}

#[test]
fn archive_footer_confirmed_clock_survives_seek_and_does_not_republish_each_frame() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(true);
    player::swap_state_for_test(player::PlaybackState::Playing);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    super::super::super::observe_programme(&mut app, 15_112_000);
    publish(&mut app);
    let archive = app.bridge.metadata_view().now_playing().unwrap().ott.as_ref().unwrap().archive;
    assert!(archive.is_some());
    let first = app.bridge.metadata_mut().gen();
    player::SHARED.playpos_ns.store(30_000_000_000, Ordering::Release);
    super::super::super::observe_programme(&mut app, 20_000_000);
    publish(&mut app);
    assert_eq!(app.bridge.metadata_mut().gen(), first);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().ott.as_ref().unwrap().archive, archive);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().ott.as_ref().unwrap().confirmed_start_unix_ms, Some(15_112_000));
}

#[test]
fn missing_archive_description_does_not_borrow_live_epg() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(false);
    publish(&mut app);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert!(now.summary.is_empty());
    assert_eq!(now.title, "Fixture channel");
    assert!(now.ott.as_ref().unwrap().next.is_none());
}

#[test]
fn archive_footer_uses_interval_until_native_duration_arrives() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(true);
    player::SHARED.duration_ns.store(0, Ordering::Release);
    publish(&mut app);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().dur_ms, 300_000);
    player::SHARED.duration_ns.store(302_000_000_000, Ordering::Release);
    publish(&mut app);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().dur_ms, 302_000);
}

#[test]
fn stale_archive_identity_cannot_replace_new_native_playback_metadata() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(true);
    publish(&mut app);
    assert!(app.bridge.metadata_view().now_playing().unwrap().ott.as_ref().unwrap().archive.is_some());
    install(&mut app); // Same URL, new native generation.
    let mut new = app.bridge.metadata_view().now_playing().unwrap().clone();
    new.title = "New native video".into();
    new.ott = None;
    app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(new)));
    publish(&mut app);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title, "New native video");
    assert!(now.ott.is_none());
}


#[test]
fn archive_context_is_captured_even_without_programme_metadata_or_browse_focus() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for with_metadata in [false, true] {
        let mut app = archive_fixture(with_metadata);
        app.ott.as_mut().unwrap().selected = Some("b".into());
        publish(&mut app);
        let now = app.bridge.metadata_view().now_playing().unwrap();
        let context = now.ott.as_ref().unwrap();
        assert_eq!(context.archive, Some(plx_data::metadata::OttArchivePlaying {
            start_s: 1_699_999_400, end_s: 1_699_999_700,
        }));
        assert!(context.live.is_none() && context.next.is_none());
        assert_eq!(context.collection_label, "Fixture channel");
        assert_eq!(now.summary.is_empty(), !with_metadata);
    }
}

#[test]
fn changed_archive_interval_republishes_even_with_identical_title_and_duration() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = archive_fixture(true);
    publish(&mut app);
    let first = app.bridge.metadata_mut().gen();
    let interval = ArchiveInterval { start: 1_699_999_460, end: 1_699_999_760 };
    let playing = app.ott.as_mut().unwrap().playing.as_mut().unwrap();
    let mut programme = playing.archive.as_ref().unwrap().programme().unwrap();
    programme.start = interval.start;
    programme.end = interval.end;
    playing.archive = Some(archive_play::Intent::new(&playing.profile, &playing.entry,
        interval, 1_700_000_000).unwrap().with_programme(&programme).unwrap());
    // Model the accepted owner's new interval; the presentation cache must
    // include it even though all old visible title/duration fields match.
    publish(&mut app);
    assert_ne!(app.bridge.metadata_mut().gen(), first);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title, "Archived bulletin");
    assert_eq!(now.dur_ms, 600_000);
    assert_eq!(now.ott.as_ref().unwrap().archive, Some(plx_data::metadata::OttArchivePlaying {
        start_s: interval.start, end_s: interval.end,
    }));
}

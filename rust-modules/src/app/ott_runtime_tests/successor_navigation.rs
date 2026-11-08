//! The real OTT admission/continuity and Bridge navigation with only decoder
//! resource acceptance substituted. No decoder, worker, network or saved write.
use super::*;

#[derive(Default)]
struct AcceptedResources { starts: usize, reject_next: bool }

impl playback::PlaybackResources for AcceptedResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external source only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool {
        self.starts += 1;
        !std::mem::take(&mut self.reject_next)
    }
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn setup(local: bool, two: bool, complete: bool)
    -> (local_integration::TempDir, Box<App>, AcceptedResources)
{
    let (root_dir, mut app, _) = local_integration::setup();
    let runtime = app.ott.as_mut().unwrap();
    let first = runtime.catalog.entries[0].clone();
    if two {
        let mut second = first;
        second.id = "second".into(); second.title = "Second".into();
        second.media = MediaLocator::Local { relative_path: "second.mp4".into() };
        runtime.catalog.entries.push(second);
    }
    if !local {
        runtime.transient = Default::default();
        for entry in &mut runtime.catalog.entries {
            entry.media = MediaLocator::Http { url: format!("https://media.invalid/{}", entry.id) };
        }
    }
    runtime.catalog_complete = complete;
    // Appending catalogue rows alone does not publish them to the admitted
    // Local session's order. Use the same update hook as a real scan result.
    transient::accepted_catalog_update(runtime);
    let expected_order: Vec<String> = if two { vec!["first".into(), "second".into()] }
        else { vec!["first".into()] };
    assert_eq!(runtime.ordered_ids(), expected_order, "fixture publishes its full playback order");
    // Keep the existing worker seams closed. Admissions below receive inspected
    // fixture metadata directly, and no Runtime tick occurs except after Back.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.saving = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    app.refresh_hubs = false;
    let mut resources = AcceptedResources::default();
    app.ott.as_mut().unwrap().play_at("first", 0).unwrap();
    admit_pending(&mut app, &mut resources);
    (root_dir, app, resources)
}

fn inspected_source(runtime: &Runtime, entry: &CatalogEntry) -> ReadySource {
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    let source = match &entry.media {
        MediaLocator::Local { relative_path } => {
            let root = runtime.local_inventory.sources[0].descriptor();
            plx_media::ff::ExternalSource::Local(
                std::path::Path::new(root.canonical_path.as_deref().unwrap()).join(relative_path))
        }
        MediaLocator::Http { url } => plx_media::ff::ExternalSource::Http(
            plx_data::ott::http_url(url).unwrap()),
        _ => panic!("external fixture source"),
    };
    ReadySource::External(source, info, None)
}

fn admit_pending(app: &mut App, resources: &mut AcceptedResources) {
    try_admit_pending(app, resources).unwrap();
}

fn try_admit_pending(app: &mut App, resources: &mut AcceptedResources) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.clone().expect("real continuity selected a successor");
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries.iter().find(|entry| entry.id == request.id).unwrap().clone();
    runtime.rebind_native_admission(app, &profile, &entry).unwrap();
    let source = inspected_source(&runtime, &entry);
    let _ = runtime.play_ticket.take();
    let _ = runtime.resolver_next.take();
    let result = start_resolved_with(&mut runtime, app, profile, entry, source, resources);
    let _ = runtime.take_completed_play_target();
    app.ott = Some(runtime);
    settle_pages(app);
    result
}

fn assert_owned_player(app: &App, id: &str) {
    assert_owned_player_preserving_refresh(app, id, false);
}

fn assert_owned_player_preserving_refresh(app: &App, id: &str, refresh_before: bool) {
    assert!(matches!(app.route(), AppArg::Player), "admission must survive real navigation commits");
    assert!(bridge::player(&app.pages).is_some(), "the replacement must retain its page owner");
    let playing = app.ott.as_ref().unwrap().playing.as_ref().unwrap();
    assert_eq!(playing.selection_id, id);
    assert!(playing_matches(playing, app));
    assert_eq!(app.refresh_hubs, refresh_before, "a replacement is not an exit requesting a hub refresh");
    assert!(app.player.pending_exit.is_none(), "replacement cannot create an explicit exit receipt");
}

#[test]
fn complete_single_local_item_retains_player_across_two_eofs() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, mut resources) = setup(true, false, true);
    let origin = bridge::player(&app.pages).unwrap().origin.unwrap();
    for _ in 0..2 {
        let before = plx_media::route::playback_identity(&app.player.session);
        assert!(finish_catalog_playback(&mut app));
        admit_pending(&mut app, &mut resources);
        assert_owned_player(&app, "first");
        assert!(plx_media::route::playback_identity(&app.player.session) != before);
        assert!(bridge::player(&app.pages).unwrap().origin.unwrap().entry == origin.entry);
    }
    assert_eq!(resources.starts, 3);
    exit_through_content(&mut app);
    assert!(app.pages.nav.top_page().unwrap().id == origin.entry);
}

#[test]
fn complete_two_local_items_advance_and_wrap_with_a_page_owner() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, mut resources) = setup(true, true, true);
    for expected in ["second", "first"] {
        assert!(finish_catalog_playback(&mut app));
        assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id, expected);
        admit_pending(&mut app, &mut resources);
        assert_owned_player(&app, expected);
    }
    assert_eq!(resources.starts, 3);
}

#[test]
fn m3u_external_successor_retains_the_original_return_target() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, mut resources) = setup(false, true, true);
    let origin = bridge::player(&app.pages).unwrap().origin.unwrap();
    assert!(finish_catalog_playback(&mut app));
    admit_pending(&mut app, &mut resources);
    assert_owned_player(&app, "second");
    assert!(bridge::player(&app.pages).unwrap().origin.unwrap().entry == origin.entry);
}

#[test]
fn back_during_successor_resolution_rejects_the_late_admission() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, _resources) = setup(true, true, true);
    let origin = bridge::player(&app.pages).unwrap().origin.unwrap();
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    let generation = runtime.play_generation;
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries[1].clone();
    let source = inspected_source(runtime, &entry);
    exit_through_content(&mut app);
    app.ott.as_ref().unwrap().tx.send(Job::Play(generation, profile, Ok((entry, source)))).unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(app.pages.nav.top_page().unwrap().id == origin.entry);
    assert!(bridge::player(&app.pages).is_none());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn partial_local_eof_waits_without_wrapping_or_leaving_the_player() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, resources) = setup(true, false, false);
    assert!(finish_catalog_playback(&mut app));
    settle_pages(&mut app);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    assert!(matches!(app.route(), AppArg::Player));
    assert!(!app.refresh_hubs);
    assert_eq!(resources.starts, 1);
}

#[test]
fn explicit_exit_still_returns_and_retires_the_native_source() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, _) = setup(true, false, true);
    let origin = bridge::player(&app.pages).unwrap().origin.unwrap();
    exit_through_content(&mut app);
    assert!(app.pages.nav.top_page().unwrap().id == origin.entry);
    assert!(bridge::player(&app.pages).is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(app.refresh_hubs);
}

#[test]
fn refused_successor_preparation_retires_old_owner_and_keeps_back_available() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_root, mut app, mut resources) = setup(true, true, true);
    let origin = bridge::player(&app.pages).unwrap().origin.unwrap();
    let previous = plx_media::route::playback_identity(&app.player.session);
    assert!(finish_catalog_playback(&mut app));
    resources.reject_next = true;
    assert_eq!(try_admit_pending(&mut app, &mut resources), Err("Native playback could not start"));
    assert!(app.ott.as_ref().unwrap().playing.is_none(), "no previous item may remain reported as playing");
    assert!(plx_media::route::playback_identity(&app.player.session) != previous);
    assert!(matches!(app.route(), AppArg::Player), "failure must retain an escapable Player owner");
    assert!(bridge::player(&app.pages).unwrap().origin.unwrap().entry == origin.entry);
    assert!(!app.refresh_hubs);
    assert!(app.player.pending_exit.is_none());
    exit_through_content(&mut app);
    assert!(app.pages.nav.top_page().unwrap().id == origin.entry);
    assert!(bridge::player(&app.pages).is_none());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(app.refresh_hubs);
}

#[path = "strict_folder_favorites.rs"]
mod strict_folder_favorites;

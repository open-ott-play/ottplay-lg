//! Real route ownership with injected native presentation observations. This
//! proves metadata projection, not hardware presentation or wall-clock precision.
use super::*;
use plx_media::{player, route};
#[path = "programme_local_root_label.rs"]
mod local_root_label;
struct Globals(u8);
impl Drop for Globals {
    fn drop(&mut self) {
        player::SHARED.reset_session();
        player::TX.reset();
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
        player::restore_state_for_test(self.0);
    }
}
fn install(app: &mut App) {
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    route::reset_player_control_for_test(&app.player.session);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    info.duration_ns = 600_000_000_000;
    route::request_external_source(
        &mut app.player.session,
        &info,
        plx_media::ff::ExternalSource::Http("https://media.invalid/programme.mkv".into()),
        "Current file",
        "Collection",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters
        .player
        .install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED
        .duration_ns
        .store(600_000_000_000, Ordering::Release);
    player::SHARED
        .playpos_ns
        .store(7_000_000_000, Ordering::Release);
}
fn fixture() -> App {
    let mut app = app_fixture();
    // The shared lifecycle fixture starts stopped; these tests model an active presentation loop.
    app.running = true;
    install(&mut app);
    assert!(super::super::playback_allowed(&app));
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
    let profile = synthetic_profile();
    let entries = [
        ("c", "Current file"),
        ("a", "Next file"),
        ("b", "Last file"),
    ]
    .into_iter()
    .map(|(id, title)| {
        let mut row = synthetic_entry();
        row.id = id.into();
        row.title = title.into();
        row.category = "Collection".into();
        row.description = "Initial description".into();
        row.poster = "https://media.invalid/poster.jpg".into();
        row.media = MediaLocator::Http {
            url: "https://media.invalid/programme.mkv".into(),
        };
        row.duration_ms = Some(600_000);
        row
    })
    .collect::<Vec<_>>();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saving = true;
    runtime.write_permit.revoke();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::VPortal;
    runtime.catalog = Catalog { hierarchy: None,
        entries: entries.clone(),
    };
    runtime.catalog_complete = true;
    runtime.continuity = continuity::Continuity::new(runtime.generation, None);
    runtime.continuity.user_selected(runtime.generation, "c");
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
        identity: route::playback_identity(&app.player.session),
        profile,
        entry: entries[0].clone(),
        selection_id: "c".into(),
        source: route::external_source(&app.player.session),
        advance_on_failure: true,
        requires_server_lease: false,
    });
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.plex_refresh_pending = true;
    app
}
fn publish(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    super::super::publish_programme(&mut runtime, app, true);
    app.ott = Some(runtime);
}
fn projection(app: &App) -> plx_data::metadata::OttNowPlaying {
    app.bridge
        .metadata_view()
        .now_playing()
        .unwrap()
        .ott
        .clone()
        .unwrap()
}
#[test]
fn programme_confirmed_clock_rejects_outgoing_latch_during_new_admission() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = fixture();
    let id = route::playback_identity(&app.player.session).unwrap();
    player::SHARED.seen_frame.store(true, Ordering::Release);
    player::SHARED.start_queued.store(true, Ordering::Release);
    super::super::observe_programme(&mut app, 15_112_000);
    assert_eq!(app.player.programme.started_at(&id), None);
    player::SHARED.start_queued.store(false, Ordering::Release);
    super::super::observe_programme(&mut app, 15_112_000);
    assert_eq!(app.player.programme.started_at(&id), Some(15_112_000));
    player::SHARED.seen_frame.store(false, Ordering::Release);
    super::super::observe_programme(&mut app, 20_000_000);
    assert_eq!(app.player.programme.started_at(&id), Some(15_112_000));
    install(&mut app);
    let new_id = route::playback_identity(&app.player.session).unwrap();
    assert_ne!(id, new_id);
    assert!(!player::seen_frame());
    super::super::observe_programme(&mut app, 21_000_000);
    assert_eq!(app.player.programme.started_at(&new_id), None);
}
#[test]
fn programme_projects_folder_file_actual_start_and_same_policy_successor() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = fixture();
    publish(&mut app);
    let before = projection(&app);
    assert_eq!(before.confirmed_start_unix_ms, None);
    assert_eq!(before.next.unwrap().estimated_start_unix_ms, None);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    super::super::observe_programme(&mut app, 15_112_000);
    publish(&mut app);
    let current = app.bridge.metadata_view().now_playing().unwrap();
    let p = current.ott.as_ref().unwrap();
    assert_eq!(current.title, "Current file");
    assert_eq!(p.collection_label, "Collection");
    assert_eq!(p.confirmed_start_unix_ms, Some(15_112_000));
    let next = p.next.as_ref().unwrap();
    assert_eq!(next.title, "Next file");
    assert_eq!(next.estimated_start_unix_ms, Some(15_712_000));
    assert!(matches!(
        p.art,
        Some(plx_data::metadata::MediaArt::Http { .. })
    ));
    assert_eq!(
        player::SHARED.playpos_ns.load(Ordering::Acquire),
        7_000_000_000
    );
    let runtime = app.ott.as_mut().unwrap();
    let ids = runtime.ordered_ids();
    let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
    assert!(
        matches!(runtime.continuity.ended(runtime.generation,"c",&refs,true),continuity::Decision::Start{entry_id,..}if entry_id=="a")
    );
}
#[test]
fn programme_omits_unknown_duration_and_incomplete_tail_and_reuses_catalog_art() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = fixture();
    player::SHARED.seen_frame.store(true, Ordering::Release);
    super::super::observe_programme(&mut app, 15_112_000);
    player::SHARED.duration_ns.store(0, Ordering::Release);
    app.ott
        .as_mut()
        .unwrap()
        .playing
        .as_mut()
        .unwrap()
        .entry
        .duration_ms = None;
    publish(&mut app);
    assert_eq!(projection(&app).next.unwrap().estimated_start_unix_ms, None);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries.truncate(1);
    runtime.catalog_complete = false;
    runtime.catalog.entries[0].description =
        "Full description from the current source. Second paragraph remains available.".into();
    runtime.catalog.entries[0].poster = "https://media.invalid/updated.jpg".into();
    publish(&mut app);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert!(now.ott.as_ref().unwrap().next.is_none());
    assert!(now.summary.contains("Second paragraph"));
    assert!(
        matches!(&now.ott.as_ref().unwrap().art,Some(plx_data::metadata::MediaArt::Http{url})if url.ends_with("updated.jpg"))
    );
}
#[test]
fn programme_stale_metadata_cannot_change_a_new_same_url_playback_or_its_catalog() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = fixture();
    let old = route::playback_identity(&app.player.session).unwrap();
    install(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.identity = route::playback_identity(&app.player.session);
    runtime.metadata_playback_identity = Some(old);
    runtime.metadata_generation = 77;
    runtime.metadata_target = Some((playing.profile.id.clone(), playing.entry.id.clone()));
    let mut stale = playing.entry.clone();
    stale.description = "Stale playback metadata".into();
    runtime
        .metadata_tx
        .send(super::super::super::ott_metadata::Event::Preview(super::super::super::ott_metadata::Preview {
            generation: 77,
            profile_id: playing.profile.id.clone(),
            entry_id: playing.entry.id.clone(),
            entry: Some(stale),
            result: Ok(json!({"description":"Stale playback metadata"})),
        }))
        .unwrap();
    runtime_tick(&mut app);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .playing
            .as_ref()
            .unwrap()
            .entry
            .description,
        "Initial description"
    );
    assert_eq!(
        app.ott.as_ref().unwrap().catalog.entries[0].description,
        "Initial description"
    );
}

// The route fixture represents the already-installed Plex plan; the metadata
// commands and addressed Detail mailbox/pump below are real. No resolver runs.
struct PlexRegistry;
impl Drop for PlexRegistry {
    fn drop(&mut self) { plx_plex::plex::reset_servers_for_test(); }
}
fn plex_fixture() -> (App, plx_plex::plex::ServerId) {
    plx_plex::plex::reset_servers_for_test();
    let sid=plx_plex::plex::register_for_test("footer-machine", "127.0.0.1", 1, "fixture-token", "fixture-client");
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut app=app_fixture();
    route::install_plex_identity_for_test(&mut app.player.session,sid,"b");
    let mut profile=synthetic_profile();
    profile.kind=ProviderKind::Plex; profile.source="footer-machine".into();
    let mut entry=synthetic_entry();
    entry.id="b".into(); entry.title="Catalogue B".into(); entry.category="Collection B".into();
    entry.description="Catalogue summary B".into(); entry.duration_ms=Some(90_000);
    entry.media=MediaLocator::Plex { server_id:profile.source.clone(),rating_key:"b".into() };
    let runtime=app.ott.as_mut().unwrap();
    runtime.loaded=true; runtime.saving=true; runtime.write_permit.revoke();
    runtime.saved.profiles.profiles=vec![profile.clone()];
    runtime.saved.profiles.active_profile=Some(profile.id.clone());
    runtime.provider=ProviderKind::Plex; runtime.catalog=Catalog{ hierarchy: None,entries:vec![entry.clone()]};
    runtime.catalog_complete=true;
    runtime.playing=Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        identity:route::playback_identity(&app.player.session), transient_owner:None,local_selection:None,
        profile,entry,selection_id:"b".into(),source:None,advance_on_failure:true,requires_server_lease:false,
    });
    assert!(super::super::playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(),&app));
    // An explicit show-page caption remains legitimate native state; do not
    // clear it globally just to make this OTT projection safe.
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(plx_data::metadata::NowPlaying {
        ott:None,is_episode:true,is_real_episode:true,title:"Previous show A".into(),ep_title:"Previous episode A".into(),
        season:1,index:2,summary:"Previous summary A".into(),year:2024,dur_ms:30_000,rating:"PG".into(),
        thumb:"/previous.jpg".into(),detail_rk:"show-a".into(),
    }))));
    let mut leaf=plx_data::metadata::PlayingItem::with_subs(vec![]);
    leaf.sid=sid; leaf.rk="b".into(); leaf.kind="episode".into();
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(leaf))));
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().title,"Previous show A",
        "premise: native caption retention itself stays unchanged");
    (app,sid)
}
fn land_plex_detail(app:&mut App,sid:plx_plex::plex::ServerId,rk:&str,kind:&str,title:&str) {
    land_plex_detail_with_thumb(app, sid, rk, kind, title, "/current.jpg");
}
fn land_plex_detail_with_thumb(app:&mut App,sid:plx_plex::plex::ServerId,rk:&str,kind:&str,title:&str,thumb:&str) {
    use plx_machine::job::Port;
    let store=app.bridge.metadata_mut();
    let gen=plx_data::metadata::begin_detail_for_test(store.adapter_ref(),sid,rk);
    store.detail_port().done(plx_machine::machine::Addr {
        to:plx_machine::machine::MachineId::Store(plx_data::stores::StoreId::Metadata.ord()),
        req:plx_machine::machine::RequestId(gen),
    }, ((sid,rk.into()),Some(plx_data::metadata::Detail {
        sid,rk:rk.into(),kind:kind.into(),title:title.into(),show_title:"Current show B".into(),
        show_rk:"show-b".into(),season:3,index:4,summary:"Current detail summary B".into(),
        year:2026,dur_ms:90_000,rating:"TV-PG".into(),thumb:thumb.into(),..Default::default()
    })));
    assert!(store.pump_detail_with_gate(&Default::default()));
    assert!(!store.has_jobs(),"empty GUID must not request alternate sources");
}
#[test]
fn programme_plex_never_adopts_retained_native_caption_without_matching_detail() {
    let _serial=plx_base::testlock::serial();
    let _globals=Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry=PlexRegistry;
    let (mut app,sid)=plex_fixture();
    let _identity = admit_same_plex_leaf(&mut app, sid);
    assert!(app.bridge.metadata_view().current().is_none());
    publish(&mut app);
    let now=app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title,"Catalogue B");
    assert!(!now.is_episode); assert!(now.ep_title.is_empty()); assert!(now.detail_rk.is_empty());
    assert_eq!(now.ott.as_ref().unwrap().collection_label,"Collection B");
    // The same item key on a foreign server is still unrelated browsing data.
    let foreign=plx_plex::plex::ServerId::from_raw(sid.raw().wrapping_add(1));
    land_plex_detail(&mut app,foreign,"b","episode","Foreign episode");
    let mut runtime=app.ott.take().unwrap();
    super::super::publish_programme(&mut runtime,&mut app,false);
    app.ott=Some(runtime);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().title,"Catalogue B");
    land_plex_detail(&mut app,sid,"other","movie","Unrelated movie");
    publish(&mut app);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().title,"Catalogue B");
}
#[test]
fn programme_plex_late_matching_detail_refreshes_without_runtime_revision() {
    let _serial=plx_base::testlock::serial();
    let _globals=Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry=PlexRegistry;
    let (mut app,sid)=plex_fixture();
    publish(&mut app);
    let revision=app.ott.as_ref().unwrap().revision;
    land_plex_detail(&mut app,sid,"b","episode","Current episode B");
    // The metadata ownership guard correctly leaves the OTT descriptor alone.
    // Only Runtime may replace its projection after verifying the exact leaf.
    assert!(app.bridge.metadata_view().now_playing().unwrap().ott.is_some());
    assert_eq!(app.ott.as_ref().unwrap().revision,revision);
    let mut runtime=app.ott.take().unwrap();
    super::super::publish_programme(&mut runtime,&mut app,false);
    app.ott=Some(runtime);
    let now=app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title,"Current show B"); assert_eq!(now.ep_title,"Current episode B");
    assert!(now.is_episode && now.is_real_episode); assert_eq!((now.season,now.index),(3,4));
    assert_eq!(now.detail_rk,"show-b"); assert_eq!(now.ott.as_ref().unwrap().collection_label,"Collection B");
    let store_generation=app.bridge.metadata_mut().gen();
    let mut runtime=app.ott.take().unwrap();
    super::super::publish_programme(&mut runtime,&mut app,false);
    app.ott=Some(runtime);
    assert_eq!(app.bridge.metadata_mut().gen(),store_generation,"an unchanged frame must not republish its own update");
}


fn publish_without_runtime_change(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    super::super::publish_programme(&mut runtime, app, false);
    app.ott = Some(runtime);
}

fn admit_same_plex_leaf(app: &mut App, sid: plx_plex::plex::ServerId) -> route::PlaybackIdentity {
    route::install_plex_playback_for_test(&mut app.player.session, sid, "b");
    let identity = route::playback_identity(&app.player.session).expect("admitted logical playback");
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = Some(identity.clone());
    assert!(super::super::playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), app));
    identity
}

fn assert_leaf_projection_eq(actual: &plx_data::metadata::NowPlaying, expected: &plx_data::metadata::NowPlaying) {
    assert_eq!((actual.is_episode, actual.is_real_episode, actual.title.as_str(), actual.ep_title.as_str(),
        actual.season, actual.index, actual.summary.as_str(), actual.year, actual.dur_ms,
        actual.rating.as_str(), actual.thumb.as_str(), actual.detail_rk.as_str()),
        (expected.is_episode, expected.is_real_episode, expected.title.as_str(), expected.ep_title.as_str(),
        expected.season, expected.index, expected.summary.as_str(), expected.year, expected.dur_ms,
        expected.rating.as_str(), expected.thumb.as_str(), expected.detail_rk.as_str()));
}

#[test]
fn programme_plex_retains_attested_leaf_after_unrelated_and_foreign_detail() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = PlexRegistry;
    let (mut app, sid) = plex_fixture();
    let identity = admit_same_plex_leaf(&mut app, sid);
    publish(&mut app);
    land_plex_detail(&mut app, sid, "b", "episode", "Current episode B");
    publish_without_runtime_change(&mut app);
    let enriched = app.bridge.metadata_view().now_playing().unwrap().clone();
    assert_eq!(enriched.ep_title, "Current episode B");
    assert_eq!(enriched.rating, "TV-PG");
    let runtime_revision = app.ott.as_ref().unwrap().revision;
    let foreign = plx_plex::plex::ServerId::from_raw(sid.raw().wrapping_add(1));
    for (browse_sid, rk) in [(sid, "c"), (foreign, "b")] {
        land_plex_detail(&mut app, browse_sid, rk, "movie", "Unrelated detail");
        assert_leaf_projection_eq(app.bridge.metadata_view().now_playing().unwrap(), &enriched);
        assert!(app.bridge.metadata_view().current_leaf_now_playing(sid, "b").is_none());
        publish_without_runtime_change(&mut app);
        assert_leaf_projection_eq(app.bridge.metadata_view().now_playing().unwrap(), &enriched);
        assert!(app.bridge.metadata_view().now_playing().unwrap().ott.is_some());
        assert_eq!(route::playback_identity(&app.player.session), Some(identity.clone()));
        assert_eq!(app.ott.as_ref().unwrap().revision, runtime_revision);
    }
}

#[test]
fn programme_plex_does_not_reuse_attested_leaf_for_a_new_admission_of_the_same_item() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = PlexRegistry;
    let (mut app, sid) = plex_fixture();
    let old_identity = admit_same_plex_leaf(&mut app, sid);
    publish(&mut app);
    land_plex_detail(&mut app, sid, "b", "episode", "Old admission episode B");
    publish_without_runtime_change(&mut app);
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().ep_title, "Old admission episode B");
    land_plex_detail(&mut app, sid, "c", "movie", "Unrelated detail");
    publish_without_runtime_change(&mut app);
    let metadata_generation = app.bridge.metadata_mut().gen();
    let identity = admit_same_plex_leaf(&mut app, sid);
    assert_eq!(app.bridge.metadata_mut().gen(), metadata_generation,
        "a new native identity must invalidate the witness even without a metadata revision");
    assert_ne!(identity, old_identity, "same item replay is a new accepted lifetime");
    assert!(app.bridge.metadata_view().current_leaf_now_playing(sid, "b").is_none());
    publish_without_runtime_change(&mut app);
    let now = app.bridge.metadata_view().now_playing().unwrap();
    assert_eq!(now.title, "Catalogue B");
    assert!(now.ep_title.is_empty() && now.detail_rk.is_empty() && now.rating.is_empty() && now.thumb.is_empty());
    assert!(!now.is_episode && !now.is_real_episode);
    assert_eq!((now.season, now.index, now.year), (0, 0, 0));
    assert!(now.ott.is_some());
    assert_eq!(route::playback_identity(&app.player.session), Some(identity));
}


#[test]
fn programme_plex_matching_leaf_art_fills_empty_catalog_poster_and_survives_browsing() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = PlexRegistry;
    let (mut app, sid) = plex_fixture();
    let _identity = admit_same_plex_leaf(&mut app, sid);
    assert!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().entry.poster.is_empty());
    publish(&mut app);
    assert!(projection(&app).art.is_none(), "retained unverified native thumb must not seed artwork");
    land_plex_detail(&mut app, sid, "b", "episode", "Current episode B");
    publish_without_runtime_change(&mut app);
    let expected = Some(plx_data::metadata::MediaArt::Plex { server: sid.raw(), key: "/current.jpg".into() });
    assert_eq!(projection(&app).art, expected);
    land_plex_detail(&mut app, sid, "c", "movie", "Unrelated detail");
    publish_without_runtime_change(&mut app);
    assert_eq!(projection(&app).art, expected);
    let _new_identity = admit_same_plex_leaf(&mut app, sid);
    publish_without_runtime_change(&mut app);
    assert!(projection(&app).art.is_none(), "a new admission needs its own leaf witness");
}

#[test]
fn programme_plex_foreign_leaf_cannot_supply_art_and_http_thumb_never_inherits_plex_credentials() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = PlexRegistry;
    let (mut app, sid) = plex_fixture();
    let _identity = admit_same_plex_leaf(&mut app, sid);
    publish(&mut app);
    let foreign = plx_plex::plex::ServerId::from_raw(sid.raw().wrapping_add(1));
    land_plex_detail(&mut app, foreign, "b", "episode", "Foreign episode");
    publish_without_runtime_change(&mut app);
    assert!(projection(&app).art.is_none());
    land_plex_detail_with_thumb(&mut app, sid, "b", "episode", "Current episode B", "https://art.invalid/image.jpg");
    publish_without_runtime_change(&mut app);
    assert_eq!(projection(&app).art, Some(plx_data::metadata::MediaArt::Http {
        url: plx_data::ott::http_url("https://art.invalid/image.jpg").unwrap(),
    }));
    land_plex_detail_with_thumb(&mut app, sid, "b", "episode", "Current episode B", "//art.invalid/image.jpg");
    publish_without_runtime_change(&mut app);
    assert!(projection(&app).art.is_none(), "an authority-relative URL is not a Plex server path");
}

#[path = "programme_archive.rs"]
mod archive;

#[path = "programme_live_witness.rs"]
mod live_witness;
#[path = "programme_live_hud.rs"]
mod live_hud;

#[path = "programme_catalog_route.rs"]
mod catalog_route;

#[path = "programme_profile_edit.rs"]
mod profile_edit;

#[path = "programme_m3u_media.rs"]
mod m3u_media;

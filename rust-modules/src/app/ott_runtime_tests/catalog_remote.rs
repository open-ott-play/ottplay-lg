//! Real remote-command admission regressions. No resolver, network or decoder runs.
use super::*;

#[path = "progressive_playback.rs"]
mod progressive_playback;

#[path = "catalog_snapshot_progressive.rs"]
mod catalog_snapshot_progressive;

struct ResetMedia;
impl Drop for ResetMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

struct Fixture {
    app: Box<App>,
    serial: u64,
}

fn configure_archive(f: &mut Fixture) -> (ProviderProfile, CatalogEntry, archive_play::Intent) {
    use plx_data::ott::{archive::{ArchiveInterval, CatchupMode, CatchupSource}, guide::ChannelMetadata};
    let runtime = f.app.ott.as_mut().unwrap();
    let now = archive_play::epoch_seconds(runtime.wall_clock_ms).unwrap();
    let mut profile = synthetic_profile();
    profile.kind = ProviderKind::M3u;
    profile.source = "https://catalog.invalid/archive.m3u".into();
    let mut entry = synthetic_entry();
    entry.id = "archive-channel".into();
    entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
    entry.live = Some(ChannelMetadata { guide_id: None, guide_feed: String::new(),
        tvg_name: "Archive channel".into(), guide_shift_seconds: 0,
        catchup: Some(CatchupSource { mode: CatchupMode::Append,
            source: "?utc={utc}&end={utcend}".into(), retention_seconds: 86400,
            correction_seconds: 0 }),
    });
    let intent = archive_play::Intent::new(&profile, &entry,
        ArchiveInterval { start: now - 600, end: now - 60 }, now).unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![entry.clone()];
    (profile, entry, intent)
}

#[test]
fn archive_request_survives_resolver_queue_and_failure_cannot_start_live() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let (_, entry, intent) = configure_archive(&mut f);
    let runtime = f.app.ott.as_mut().unwrap();
    let mut request = PlaybackRequest::ordinary(&entry.id, 0);
    request.archive = Some(intent.clone());
    request.advance_on_failure = false;
    runtime.play_request(request).unwrap();
    let resolver = runtime.resolver_next.take().unwrap();
    assert_eq!(resolver.archive, Some(intent));
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    runtime.tx.send(Job::Play(resolver.generation, resolver.profile, Err("Archive unavailable")))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none());
    assert_eq!(runtime.status, "Archive unavailable");
}

#[test]
fn archive_is_exact_and_expired_completion_cannot_reach_decoder() {
    use plx_data::ott::archive::ArchiveInterval;
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let (profile, entry, intent) = configure_archive(&mut f);
    let mut runtime = f.app.ott.take().unwrap();
    let mut request = PlaybackRequest::ordinary(&entry.id, 0);
    request.archive = Some(intent);
    assert!(runtime.prepare_play_request(request.clone()).is_err());
    request.advance_on_failure = false;
    request.position_ms = 1000;
    assert!(runtime.prepare_play_request(request.clone()).is_err());
    request.position_ms = 0;
    let now = archive_play::epoch_seconds(runtime.wall_clock_ms).unwrap();
    request.archive = Some(archive_play::Intent::new(&profile, &entry,
        ArchiveInterval { start: now - 86401, end: now - 86301 }, now - 2).unwrap());
    // Model a request that was valid when its asynchronous resolver started.
    runtime.play_target = Some(request);
    let result = super::super::start_resolved(&mut runtime, &mut f.app, profile, entry,
        ReadySource::External(plx_media::ff::ExternalSource::Http("https://media.invalid/archive".into()),
            unsupported_info(), None));
    assert_eq!(result, Err("Archive interval is expired or unavailable"));
    assert!(!plx_media::route::has_url(&f.app.player.session));
    assert!(runtime.playing.is_none());
    f.app.ott = Some(runtime);
}

#[test]
fn archive_final_admission_rejects_changed_source_and_retires_candidate_lease() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let (profile, entry, intent) = configure_archive(&mut f);
    let mut runtime = f.app.ott.take().unwrap();
    let mut request = PlaybackRequest::ordinary(&entry.id, 0);
    request.archive = Some(intent);
    request.advance_on_failure = false;
    runtime.play_request(request).unwrap();
    runtime.saved.profiles.profiles[0].source = "https://catalog.invalid/replaced.m3u".into();
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let source = ReadySource::External(
        plx_media::ff::ExternalSource::Http("https://media.invalid/archive".into()),
        unsupported_info(), Some(lease));
    assert!(super::super::start_resolved(&mut runtime, &mut f.app, profile, entry, source).is_err());
    assert_eq!(released.recv_timeout(Duration::from_millis(100)), Ok(()));
    assert!(runtime.active_lease.as_ref().is_some_and(|lease| !lease.is_playback()));
    assert!(runtime.playing.is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
    f.app.ott = Some(runtime);
}

#[test]
fn archive_calendar_checks_use_raw_wall_clock_not_anchored_command_clock() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.wall_clock_ms = runtime.now_ms().saturating_add(172_800_000);
    let (profile, entry, intent) = configure_archive(&mut f);
    let runtime = f.app.ott.as_mut().unwrap();
    let mut request = PlaybackRequest::ordinary(&entry.id, 0);
    request.archive = Some(intent);
    request.advance_on_failure = false;
    runtime.play_request(request.clone()).unwrap();
    let source = ReadySource::External(
        plx_media::ff::ExternalSource::Http("https://media.invalid/archive".into()),
        unsupported_info(), None);
    assert!(runtime.validate_archive_admission(&request, &profile, &entry, &source).is_ok());
}

#[test]
fn archive_eof_stops_without_successor_or_live_resume_checkpoint() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    install_previous(&mut f, false, true);
    let (profile, entry, intent) = configure_archive(&mut f);
    let mut runtime = f.app.ott.take().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile;
    playing.entry = entry.clone();
    playing.selection_id = entry.id.clone();
    playing.archive = Some(intent);
    runtime.continuity.user_selected(runtime.generation, &entry.id);
    runtime.checkpoint(&f.app, true);
    assert!(runtime.checkpoints.is_empty());
    f.app.ott = Some(runtime);
    assert!(super::super::finish_catalog_playback(&mut f.app));
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    assert_eq!(runtime.status, "Archive ended");
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

fn install_previous(f: &mut Fixture, _leased: bool, _exact: bool) -> Playing {
    // Retain the exact canonical source that real external admission installs.
    let source = plx_media::ff::ExternalSource::Http(
        plx_data::ott::http_url("https://media.invalid/previous").unwrap(),
    );
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    plx_media::route::reset_player_control_for_test(&f.app.player.session);
    plx_media::route::request_external_source(
        &mut f.app.player.session,
        &info,
        source.clone(),
        "Previous",
        "Series",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    assert!(plx_media::route::playback_identity(&f.app.player.session).is_some());
    assert_eq!(plx_media::route::external_source(&f.app.player.session), Some(source.clone()));
    // Admission only: never call start_playback or open a decoder/socket.
    let mut entry = synthetic_entry();
    entry.id = "previous".into();
    let previous = Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
        identity: plx_media::route::playback_identity(&f.app.player.session),
        profile: synthetic_profile(),
        entry,
        selection_id: "previous".into(),
        source: Some(source),
        advance_on_failure: !_exact,
        requires_server_lease: _leased,
    };
    f.app.ott.as_mut().unwrap().playing = Some(previous.clone());
    assert!(playing_matches(&previous, &f.app));
    previous
}
fn retire_previous_for_replacement(f: &mut Fixture) {
    playback::replace_player(
        &mut f.app.player,
        &mut f.app.adapters.player,
        &mut f.app.repause_at,
        &mut f.app.refresh_hubs,
        &mut f.app.pages,
    );
    assert!(!plx_media::route::has_url(&f.app.player.session));
}
impl Fixture {
    fn new(count: usize) -> Self {
        let mut app = Box::new(app_fixture());
        // Model the running application; the common inert rig defaults to stopped.
        app.running = true;
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.provider = ProviderKind::VPortal;
        runtime.saved.profiles.profiles = vec![synthetic_profile()];
        runtime.saved.profiles.active_profile = Some("portal-fixture".into());
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
        runtime.resolver_busy = true; // Retain real queued requests; do not spawn IO.
        runtime.catalog.entries = (0..count)
            .map(|i| {
                let mut entry = synthetic_entry();
                entry.id = format!("video-{i}");
                entry.title = format!("Video {i}");
                entry.category = if i % 2 == 0 { "Series" } else { "Films" }.into();
                entry
            })
            .collect();
        Self { app, serial: 0 }
    }
    fn request(&mut self, action: &str, params: Value) -> Option<Value> {
        self.serial += 1;
        let mut runtime = self.app.ott.take().unwrap();
        let now = runtime.now_ms();
        let body = json!({
            "commands": [], "server_time": 1000,
            "requests": [{
                "id": format!("{:032x}", self.serial), "expires_at": 1020,
                "action": action, "params": params,
            }],
        }).to_string();
        let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
        assert_eq!(incoming.len(), 1, "one raw request must be admitted");
        let inv = incoming.pop().unwrap();
        assert_eq!(inv.deadline_ms, now + 20_000);
        assert!(runtime.remote.invocation_is_current(&inv));
        let response = commands::command(&mut runtime, &mut self.app, &inv);
        if let Some(response) = &response {
            runtime.remote.complete(inv, response.clone());
        }
        self.app.ott = Some(runtime);
        response
    }
    fn runtime_id(&self) -> String {
        self.app.ott.as_ref().unwrap().remote.runtime().to_owned()
    }
    fn categories(&mut self) -> Value {
        let response = self
            .request(
                "categories",
                json!({
                    "op":"list", "runtime":self.runtime_id(), "offset":0,"limit":200
                }),
            )
            .expect("listing responds synchronously");
        assert_eq!(
            response["status"], "ok",
            "native must implement the advertised catalog path"
        );
        response["data"].clone()
    }
    fn channels(&mut self, catalog: &str, category: &str, offset: usize) -> Value {
        let response = self
            .request(
                "channels",
                json!({
                    "runtime":self.runtime_id(), "catalog":catalog,"category":category,
                    "offset":offset,"limit":200
                }),
            )
            .unwrap();
        assert_eq!(response["status"], "ok");
        response["data"].clone()
    }
}

#[test]
fn catalog_remote_pages_cross_legacy_limit_without_starting_io() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(10_201);
    let caps = f.request("capabilities", json!({})).unwrap();
    assert_eq!(caps["data"]["categories"]["operations"], json!(["list", "select"]));
    let local: Value = serde_json::from_str(include_str!("../fixtures/local-media-v1.json")).unwrap();
    let session: Value = serde_json::from_str(include_str!("../fixtures/local-media-session-v1.json")).unwrap();
    assert_eq!(caps["data"]["local_media"], local["capability"]);
    assert_eq!(caps["data"]["local_media_session"], session["capability"]);
    assert_eq!(caps["data"]["local_media_progressive"], local["progressive_capability"]);
    assert!(
        caps["data"].get("random").is_none(),
        "legacy random policy is not negotiated"
    );
    assert_eq!(caps["data"]["catalog_random"],
        json!({"version":2,"scope":"receipt","request_field":"receipt_random"}));
    let cats = f.categories();
    let first = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert_eq!(first["total"], 10_201);
    assert_eq!(first["channels"].as_array().unwrap().len(), 200);
    let tail = f.channels(first["catalog"].as_str().unwrap(), "all", 10_200);
    assert_eq!(
        tail["channels"],
        json!([{"id":"video-10200","name":"Video 10200","number":10201}])
    );
    assert_eq!(tail["has_more"], false);
    assert_eq!(tail["catalog"], first["catalog"]);
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_explicit_id_uses_receipt_category_not_current_ui_category() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    let cats = f.categories();
    let series = cats["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "Series")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = f.channels(cats["catalog"].as_str().unwrap(), &series, 0);
    f.app.ott.as_mut().unwrap().saved.category = "Films".into();
    assert!(
        f.request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-2"})
        )
        .is_none(),
        "accepted play replies only after actual native start admission"
    );
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .unwrap()
            .entry
            .as_ref()
            .unwrap()
            .id,
        "video-2"
    );
    assert_eq!(
        runtime.saved.category, "Films",
        "one-shot play must not change browsing"
    );
}

#[test]
fn catalog_remote_stale_source_and_category_receipt_never_start_playback() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    let cats = f.categories();
    let rejected = f
        .request(
            "play_catalog",
            json!({"catalog":cats["catalog"],"id":"video-0"}),
        )
        .unwrap();
    assert_ne!(
        rejected["status"], "ok",
        "list-only receipt is not a playback capability"
    );
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    f.app.ott.as_mut().unwrap().saved.profiles.profiles[0].source =
        "portal::[key:fixture]https://changed.invalid/nas/api".into();
    let rejected = f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"}),
        )
        .unwrap();
    assert_ne!(rejected["status"], "ok");
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_loading_is_explicit_and_append_preserves_read_pages() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    f.app.ott.as_mut().unwrap().catalog_loading = true;
    f.app.ott.as_mut().unwrap().catalog_complete = false;
    let cats = f.categories();
    assert_eq!(cats["loading"], true);
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert_eq!(page["loading"], true);
    f.app.ott.as_mut().unwrap().catalog_revision += 1;
    let response = f
        .request(
            "channels",
            json!({"runtime":f.runtime_id(),
        "catalog":page["catalog"],"category":"all","offset":0,"limit":200}),
        )
        .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["total"], page["total"]);
    assert_eq!(response["data"]["channels"], page["channels"]);
    assert_eq!(response["data"]["loading"], true);
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_search_preserves_unicode_caseless_contract() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(3);
    for (entry, name) in f
        .app
        .ott
        .as_mut()
        .unwrap()
        .catalog
        .entries
        .iter_mut()
        .zip(["Straße", "CAFÉ", "ΟΣ"])
    {
        entry.title = name.into();
    }
    let cats = f.categories();
    for (query, id) in [
        ("STRASSE", "video-0"),
        ("cafe\u{301}", "video-1"),
        ("ος", "video-2"),
    ] {
        let response = f
            .request(
                "channels",
                json!({"runtime":f.runtime_id(),
            "catalog":cats["catalog"],"category":"all","offset":0,"limit":200,"search":query}),
            )
            .unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["channels"][0]["id"], id);
        assert_eq!(response["data"]["total"], 1);
    }
}

#[test]
fn catalog_remote_global_favorites_keep_their_own_identity_and_order() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(3);
    let favorite: plx_data::ott::favorites::Favorite = serde_json::from_value(json!({
        "title":"Saved film", "origin":{"provider":"vportal","profile":"Fixture",
            "subprofile":null,"sourceAlias":"Fixture"},
        "locator":{"kind":"vportal-media","idField":"id","id":"1"}
    }))
    .unwrap();
    let expected = format!("favorite:{}", favorite.identity());
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.saved.favorites_selected = true;
    let cats = f.categories();
    assert_eq!(cats["provider"], "favorites");
    assert_eq!(cats["selected"], "all");
    assert_eq!(cats["categories"][0]["count"], 1);
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert_eq!(page["channels"][0]["number"], 1);
    assert_eq!(page["channels"][0]["name"], "Saved film");
    let wire_id = page["channels"][0]["id"].as_str().unwrap();
    assert!(wire_id.starts_with("fav:"));
    assert!(!wire_id.contains("locator"));
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":wire_id})
        )
        .is_none());
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.selected.as_deref(), Some(expected.as_str()));
    assert!(runtime.resolver_next.as_ref().unwrap().favorite.is_some());
    assert!(runtime.saved.favorites_selected);
    assert_eq!(
        commands::play_response(runtime, runtime.pending_play.as_ref().unwrap())["data"]["channel"],
        page["channels"][0],
        "the dispatch receipt retains the original favorite wire identity"
    );
}

#[test]
fn catalog_remote_repeat_source_check_precedes_native_start() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    // The resolver captured the old profile. A later source edit cannot grant
    // that old request permission to install a native source.
    runtime.saved.profiles.profiles[0].source =
        "portal::[key:fixture]https://changed.invalid/nas/api".into();
    runtime
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Ok((
                synthetic_entry(),
                ReadySource::External(
                    plx_media::ff::ExternalSource::Http("https://media.invalid/video".into()),
                    unsupported_info(),
                    None,
                ),
            )),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().playing.is_none());
    assert!(f
        .app
        .bridge
        .metadata_view()
        .now_playing()
        .is_none_or(|info| info.title != "MUST NOT BE INSTALLED AFTER EXIT"));
}

#[test]
fn catalog_remote_read_in_standby_does_not_allow_play_or_random() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(2);
    f.app.ott_standby = true;
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    let response = f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"}),
        )
        .unwrap();
    assert_eq!(response["status"], "rejected");
    let response = f
        .request(
            "play_catalog",
            json!({"catalog":cats["catalog"],"random":{}}),
        )
        .unwrap();
    assert_eq!(response["status"], "unsupported");
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_receipt_cannot_widen_its_category_or_search() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    let cats = f.categories();
    let series = cats["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "Series")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = f
        .request(
            "channels",
            json!({"runtime":f.runtime_id(),
        "catalog":cats["catalog"],"category":series,"offset":0,"limit":1,"search":"Video 2"}),
        )
        .unwrap();
    assert_eq!(response["status"], "ok");
    let receipt = response["data"]["catalog"].clone();
    for id in ["video-0", "video-1"] {
        let rejected = f
            .request("play_catalog", json!({"catalog":receipt,"id":id}))
            .unwrap();
        assert_eq!(rejected["status"], "rejected");
    }
    let rejected = f
        .request(
            "channels",
            json!({"runtime":f.runtime_id(),
        "catalog":receipt,"category":"all","offset":0,"limit":200,"search":"Video 2"}),
        )
        .unwrap();
    assert_eq!(rejected["data"]["error"], "catalog_changed");
    let rejected = f
        .request(
            "channels",
            json!({"runtime":f.runtime_id(),
        "catalog":receipt,"category":series,"offset":0,"limit":200}),
        )
        .unwrap();
    assert_eq!(rejected["data"]["error"], "catalog_changed");
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_legacy_search_remains_available_without_receipts() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    f.app.ott.as_mut().unwrap().saved.category = "Films".into();
    let result = f.request("channels", json!({"search":"Video"})).unwrap();
    assert_eq!(
        result["data"],
        json!({"channels":[
            {"id":"video-1","name":"Video 1","number":2},
            {"id":"video-3","name":"Video 3","number":4}
        ]})
    );
}

#[test]
fn catalog_remote_failed_exact_request_cannot_resume_as_a_different_later_file() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    runtime
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Err("Fixture cannot decode"),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    // One failed item cannot cause IO even in the broken implementation. The
    // pure subsequent discovery below proves that the failed request retired
    // its auto-advance authority rather than waiting for an unrelated file.
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_mut().unwrap();
    assert!(runtime.resolver_next.is_none());
    let decision = runtime.continuity.catalog_updated(
        runtime.generation,
        &["video-0", "unrelated-later-file"],
        true,
    );
    assert!(matches!(decision, continuity::Decision::Idle));
    assert!(runtime.pending_play.is_none());
}

#[test]
fn catalog_remote_failure_retains_unleased_previous_but_retires_closed_leased_picture() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for leased in [false, true] {
        let mut f = Fixture::new(1);
        let previous = install_previous(&mut f, leased, false);
        let (lease, released, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
        if leased {
            f.app.ott.as_mut().unwrap().active_lease = Some(lease.into());
        } else {
            drop(lease);
        }
        let cats = f.categories();
        let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
        assert!(f
            .request(
                "play_catalog",
                json!({"catalog":page["catalog"],"id":"video-0"})
            )
            .is_none());
        let runtime = f.app.ott.as_mut().unwrap();
        let request = runtime.resolver_next.take().unwrap();
        if leased {
            // Model the mandatory completed stop before the worker emits its
            // result. No real lease worker, decoder or server is involved.
            let previous = runtime.active_lease.take().unwrap();
            previous.begin_release();
            assert_eq!(released.recv_timeout(Duration::from_millis(100)), Ok(()));
            let _ = finished.send(());
            drop(previous);
        }
        runtime
            .tx
            .send(Job::Play(
                request.generation,
                request.profile,
                Err("Fixture cannot resolve"),
            ))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut f.app);
        if leased {
            assert!(f.app.ott.as_ref().unwrap().playing.is_none());
            assert!(!playing_matches(&previous, &f.app));
            assert!(plx_media::route::external_source(&f.app.player.session).is_none(),
                "an ended lease must not leave a resumable native source");
            assert!(!plx_media::route::has_url(&f.app.player.session));
        } else {
            assert!(f.app.ott.as_ref().unwrap().playing.is_some());
            assert!(playing_matches(&previous, &f.app));
        }
        plx_media::route::cancel_play(&mut f.app.player.session);
    }
}

#[test]
fn catalog_remote_rejected_before_worker_keeps_existing_lease_and_picture() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, true, false);
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    f.app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    let cats = f.categories();
    let response = f
        .request(
            "play_catalog",
            json!({"catalog":cats["catalog"],"id":"video-0"}),
        )
        .unwrap();
    assert_eq!(response["status"], "rejected");
    assert!(matches!(
        released.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(playing_matches(&previous, &f.app));
    assert!(f.app.ott.as_ref().unwrap().active_lease.is_some());
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_native_refusal_after_replacement_does_not_restore_destroyed_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    install_previous(&mut f, false, false);
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    runtime
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Ok((
                request.entry.unwrap(),
                ReadySource::External(
                    plx_media::ff::ExternalSource::Http("https://media.invalid/refused".into()),
                    unsupported_info(),
                    Some(lease),
                ),
            )),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_mut().unwrap();
    assert!(runtime.playing.is_none());
    assert!(runtime.pending_play.is_none());
    assert_eq!(released.recv_timeout(Duration::from_millis(100)), Ok(()));
    assert!(matches!(
        runtime.continuity.catalog_updated(
            runtime.generation,
            &["video-0", "unrelated-later-file"],
            true
        ),
        continuity::Decision::Idle
    ));
}

#[test]
fn catalog_remote_unrepresentable_metadata_keeps_legacy_search_route() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    f.app.ott.as_mut().unwrap().catalog.entries[0].title = "x".repeat(513);
    let caps = f.request("capabilities", json!({})).unwrap();
    assert!(caps["data"].get("categories").is_none());
    let legacy = f.request("channels", json!({"search":"x"})).unwrap();
    assert_eq!(legacy["status"], "ok");
    assert_eq!(
        legacy["data"]["channels"][0]["name"]
            .as_str()
            .unwrap()
            .len(),
        513
    );
}

#[test]
fn catalog_remote_terminal_scan_failure_is_not_reported_as_endless_loading() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.catalog_complete = false;
    runtime.catalog_loading = true;
    runtime.continuity.stop(runtime.generation);
    let cats = f.categories();
    let runtime = f.app.ott.as_mut().unwrap();
    runtime
        .catalog_tx
        .send(CatalogEvent::Finished(
            runtime.generation,
            Err("Fixture scan failed"),
        ))
        .unwrap_or_else(|_| panic!("catalog fixture receiver disappeared"));
    runtime_tick(&mut f.app);
    let caps = f.request("capabilities", json!({})).unwrap();
    assert!(caps["data"].get("categories").is_none());
    let result = f
        .request(
            "channels",
            json!({"runtime":f.runtime_id(),
        "catalog":cats["catalog"],"category":"all","offset":0,"limit":200}),
        )
        .unwrap();
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["data"]["error"], "catalog_incomplete");
    assert!(result["data"].get("loading").is_none());
}

#[test]
fn catalog_remote_failed_initial_launch_does_not_hide_global_favorites() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(0);
    let runtime = f.app.ott.as_mut().unwrap();
    // The state committed when no catalog worker could be launched.
    runtime.catalog_loading = false;
    runtime.catalog_complete = false;
    let caps = f.request("capabilities", json!({})).unwrap();
    assert!(caps["data"].get("categories").is_none());
    let response = f
        .request(
            "categories",
            json!({"op":"list","runtime":f.runtime_id(),"offset":0,"limit":200}),
        )
        .unwrap();
    assert_eq!(response["data"]["error"], "catalog_incomplete");
    f.app.ott.as_mut().unwrap().saved.favorites_selected = true;
    let favorites = f.categories();
    assert_eq!(favorites["provider"], "favorites");
    assert_eq!(favorites["loading"], false);
}

#[test]
fn catalog_remote_delayed_external_failure_after_ack_does_not_auto_next() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, false, true);
    let runtime = f.app.ott.as_mut().unwrap();
    // Synthetic committed exact owner after the outer dispatch reply. This is
    // deliberately not a decoder/PlayLanding end-to-end test.
    runtime
        .continuity
        .user_selected(runtime.generation, &previous.selection_id);
    runtime.pending_play = None;
    runtime.play_target = None;
    plx_media::route::refuse_for_test(
        &mut f.app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert!(playing_matches(&previous, &f.app));
    runtime_tick(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
    assert!(f.app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn catalog_remote_old_failure_does_not_cancel_a_newer_native_source() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let old = install_previous(&mut f, true, true);
    let newer = plx_media::ff::ExternalSource::Http(
        plx_data::ott::http_url("https://media.invalid/newer").unwrap(),
    );
    retire_previous_for_replacement(&mut f);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    plx_media::route::request_external_source(
        &mut f.app.player.session,
        &info,
        newer.clone(),
        "Newer",
        "Other",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let newer_identity = plx_media::route::playback_identity(&f.app.player.session);
    assert_ne!(newer_identity, old.identity);
    // The retained old owner required a now-ended lease, but its cleanup must
    // not retire the different source already installed by native admission.
    let mut runtime = f.app.ott.take().unwrap();
    assert!(runtime.active_lease.is_none());
    runtime.retire_closed_source(&mut f.app);
    f.app.ott = Some(runtime);
    assert_eq!(plx_media::route::playback_identity(&f.app.player.session), newer_identity);
    assert_eq!(plx_media::route::external_source(&f.app.player.session), Some(newer.clone()));
    assert!(plx_media::route::has_url(&f.app.player.session));
    plx_media::route::refuse_for_test(
        &mut f.app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert!(!playing_matches(&old, &f.app));
    runtime_tick(&mut f.app);
    assert_eq!(
        plx_media::route::external_source(&f.app.player.session),
        Some(newer)
    );
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_normal_eof_still_advances_after_exact_selection() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let current = install_previous(&mut f, false, true);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.saving = true; // No persistence IO; this test owns only EOS admission.
    runtime
        .continuity
        .user_selected(runtime.generation, &current.selection_id);
    assert!(super::super::finish_catalog_playback(&mut f.app));
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_some());
}

#[test]
fn catalog_remote_delayed_plex_refusal_after_ack_does_not_auto_next() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    struct ResetRegistry;
    impl Drop for ResetRegistry {
        fn drop(&mut self) {
            plx_plex::plex::reset_servers_for_test();
        }
    }
    let _registry = ResetRegistry;
    plx_plex::plex::reset_servers_for_test();
    let sid = plx_plex::plex::register_for_test(
        "catalog-fixture",
        "127.0.0.1",
        1,
        "fixture-token",
        "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut f = Fixture::new(1);
    let mut committed = install_previous(&mut f, false, true);
    plx_media::route::cancel_play(&mut f.app.player.session);
    committed.profile.kind = ProviderKind::Plex;
    committed.profile.source = "catalog-fixture".into();
    committed.entry.media = MediaLocator::Plex {
        server_id: "catalog-fixture".into(),
        rating_key: "42".into(),
    };
    committed.source = None;
    let runtime = f.app.ott.as_mut().unwrap();
    runtime
        .continuity
        .user_selected(runtime.generation, &committed.selection_id);
    runtime.playing = Some(committed.clone());
    runtime.pending_play = None;
    runtime.play_target = None;
    plx_media::route::install_plex_identity_for_test(&mut f.app.player.session, sid, "42");
    plx_media::route::refuse_for_test(
        &mut f.app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert!(playing_matches(&committed, &f.app));
    runtime_tick(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().playing.is_none());
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_superseding_request_remembers_previous_lease_was_closed() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    install_previous(&mut f, true, false);
    let (lease, released, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    f.app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let first = runtime.resolver_next.take().unwrap();
    let held = runtime.active_lease.take().unwrap();
    held.begin_release();
    assert_eq!(released.recv_timeout(Duration::from_millis(100)), Ok(()));
    let _ = finished.send(());
    drop(held);
    // A second request arrives while the old worker owns/has closed the lease.
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let second = runtime.resolver_next.take().unwrap();
    runtime
        .tx
        .send(Job::Play(
            first.generation,
            first.profile,
            Err("Superseded"),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime
        .tx
        .send(Job::Play(
            second.generation,
            second.profile,
            Err("Cannot resolve"),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().playing.is_none());
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn catalog_remote_candidate_failure_with_restored_owned_lease_preserves_it() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, true, false);
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    f.app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    // The original lease is still/restored in its owned slot; no stop occurred.
    runtime
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Err("Candidate was not opened"),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut f.app);
    assert!(playing_matches(&previous, &f.app));
    assert!(f.app.ott.as_ref().unwrap().playing.is_some());
    assert!(matches!(
        released.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn catalog_remote_superseded_same_source_retry_keeps_new_native_request() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, true, false);
    let (lease, _released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    f.app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({"catalog":page["catalog"],"id":"video-0"})
        )
        .is_none());
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    drop(runtime.active_lease.take()); // Old worker handed off its mandatory stop.
    retire_previous_for_replacement(&mut f);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    plx_media::route::request_external_source(
        &mut f.app.player.session,
        &info,
        previous.source.clone().unwrap(),
        "Retry",
        "Series",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let newer = plx_media::route::play_request_ticket();
    f.app
        .ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Err("Old request failed"),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut f.app);
    assert!(plx_media::route::play_request_is_current(newer));
    assert_eq!(
        plx_media::route::external_source(&f.app.player.session),
        previous.source
    );
    assert!(f.app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn catalog_remote_managed_wake_preserves_exact_failure_policy_and_latest_position() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for exact in [true, false] {
        let mut f = Fixture::new(1);
        let current = install_previous(&mut f, true, exact);
        let (lease, released, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.catalog.entries = vec![current.entry.clone()];
        runtime
            .continuity
            .user_selected(runtime.generation, &current.selection_id);
        runtime.active_lease = Some(lease.into());
        runtime.saving = true; // Isolate playback policy from the checkpoint writer.
        plx_media::player::SHARED
            .playpos_ns
            .store(4_000_000_000, Ordering::Relaxed);
        f.app.ott_standby = true;
        super::super::suspend(&mut f.app);
        assert_eq!(released.recv_timeout(Duration::from_millis(100)), Ok(()));
        let _ = finished.send(());
        assert_eq!(
            f.app
                .ott
                .as_ref()
                .unwrap()
                .resume_pending
                .as_ref()
                .map(|target| target.position_ms),
            Some(4_000)
        );
        f.app.ott_standby = false;
        super::super::resume(&mut f.app);
        runtime_tick(&mut f.app);
        let runtime = f.app.ott.as_mut().unwrap();
        let request = runtime
            .resolver_next
            .take()
            .expect("wake must re-resolve the managed source");
        assert!(
            runtime.pending_play.is_none(),
            "wake does not resurrect an expired RPC"
        );
        runtime
            .tx
            .send(Job::Play(
                request.generation,
                request.profile,
                Err("Wake resolution failed"),
            ))
            .unwrap_or_else(|_| panic!("receiver"));
        runtime_tick(&mut f.app);
        let runtime = f.app.ott.as_mut().unwrap();
        // Only the failed current file existed during tick: even a broken policy
        // cannot spawn IO. New discovery is supplied to the pure policy below.
        let next = runtime.continuity.catalog_updated(
            runtime.generation,
            &["previous", "later-file"],
            true,
        );
        assert_eq!(matches!(next, continuity::Decision::Idle), exact);
    }
}

#[test]
fn catalog_remote_new_selection_retires_parked_managed_resume() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let current = install_previous(&mut f, true, true);
    let (lease, _released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.active_lease = Some(lease.into());
    runtime.saving = true;
    runtime
        .continuity
        .user_selected(runtime.generation, &current.selection_id);
    super::super::suspend(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().resume_pending.is_some());
    // Explicit selection happens after foreground admission and worker rearm,
    // before a tick can replay the parked source. Suspended work stays denied.
    let mut runtime = f.app.ott.take().unwrap();
    assert!(!runtime.play_work_allowed());
    runtime.sync_source_lifecycle(&mut f.app);
    runtime.rearm_source_work(&mut f.app);
    assert!(runtime.play_work_allowed());
    assert!(runtime.resume_pending.is_some());
    f.app.ott = Some(runtime);
    assert!(f.request("play", json!({"query":"Video 0"})).is_none());
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.resume_pending.is_none());
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .unwrap()
            .entry
            .as_ref()
            .unwrap()
            .id,
        "video-0"
    );
}


#[test]
fn catalog_same_url_new_native_play_retires_old_logical_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, false, true);
    retire_previous_for_replacement(&mut f);
    let current = install_previous(&mut f, false, false);
    assert_eq!(previous.source, current.source, "only logical ownership differs");
    assert!(!playing_matches(&previous, &f.app));
    assert!(playing_superseded(&previous, &f.app));
    let ticket = plx_media::route::play_request_ticket();
    let mut runtime = f.app.ott.take().unwrap();
    runtime.playing = Some(previous);
    runtime.reconcile_owner(&mut f.app);
    assert!(runtime.playing.is_none());
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(plx_media::route::has_url(&f.app.player.session));
    f.app.ott = Some(runtime);
}

#[test]
fn catalog_parked_resume_cannot_reclaim_same_url_new_native_play() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, true, true);
    let (lease, _released, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.catalog.entries = vec![previous.entry.clone()];
    runtime.active_lease = Some(lease.into());
    runtime.saving = true;
    f.app.ott_standby = true;
    super::super::suspend(&mut f.app);
    let _ = finished.send(());
    assert!(f.app.ott.as_ref().unwrap().resume_pending.is_some());
    // The real native fresh-admission path, deliberately outside the OTT owner.
    retire_previous_for_replacement(&mut f);
    let current = install_previous(&mut f, false, false);
    assert_eq!(previous.source, current.source);
    f.app.ott.as_mut().unwrap().playing = None;
    let ticket = plx_media::route::play_request_ticket();
    f.app.ott_standby = false;
    super::super::resume(&mut f.app);
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.resume_pending.is_none());
    assert!(runtime.resolver_next.is_none(), "old resume must not resolve or select again");
    assert!(runtime.play_target.is_none());
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(plx_media::route::has_url(&f.app.player.session));
}


#[test]
fn catalog_exit_from_same_url_new_native_play_cannot_checkpoint_old_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, false, true);
    let checkpoint_key = previous.profile.id.clone();
    retire_previous_for_replacement(&mut f);
    let current = install_previous(&mut f, false, false);
    assert_eq!(previous.source, current.source, "addresses cannot distinguish the two plays");
    {
        let runtime = f.app.ott.as_mut().unwrap();
        // The new native admission bypassed OTT; its old owner has not ticked yet.
        runtime.playing = Some(previous);
        runtime.saving = true;
    }
    mount_player(&mut f.app);
    plx_media::player::SHARED.playpos_ns.store(25_000_000_000, Ordering::Relaxed);
    exit_through_content(&mut f.app);
    let exit = f.app.player.pending_exit.as_ref().expect("real Exit captured its receipt");
    assert_eq!(exit.position_ns, 25_000_000_000);
    assert_eq!(exit.source, current.source);
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(!runtime.checkpoints.contains_key(&checkpoint_key),
        "a later native play of the same URL must not save into the old OTT owner");
    assert!(f.app.player.pending_exit.is_none());
}

#[test]
fn catalog_matching_exit_checkpoints_its_owner_after_a_new_native_admission() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let previous = install_previous(&mut f, false, true);
    let checkpoint_key = previous.profile.id.clone();
    f.app.ott.as_mut().unwrap().saving = true;
    mount_player(&mut f.app);
    plx_media::player::SHARED.playpos_ns.store(37_000_000_000, Ordering::Relaxed);
    exit_through_content(&mut f.app);
    assert_eq!(f.app.player.pending_exit.as_ref().unwrap().position_ns, 37_000_000_000);
    // The receipt belongs to the exited play even if another native play has
    // already been admitted before the OTT consumer drains that receipt.
    let current = install_previous(&mut f, false, false);
    assert_eq!(previous.source, current.source);
    f.app.ott.as_mut().unwrap().playing = Some(previous.clone());
    let ticket = plx_media::route::play_request_ticket();
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    let checkpoint = runtime.checkpoints.get(&checkpoint_key)
        .expect("the matching receipt must survive a later current-route token");
    assert_eq!(checkpoint.entry_id, previous.selection_id);
    assert_eq!(checkpoint.position_ms, 37_000);
    assert!(f.app.player.pending_exit.is_none());
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(plx_media::route::has_url(&f.app.player.session));
}

// Hold the actual resolver request while the existing catalog receiver advances.
// Synthetic unsupported media deliberately stops at native admission, before
// start_playback can create a decoder or socket. Installed NowPlaying proves
// the exact result reached that boundary instead of being rejected as stale.
fn pending_append_fixture() -> (Fixture, Value, ResolveRequest) {
    let mut f = Fixture::new(1);
    {
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
    }
    let categories = f.categories();
    let page = f.channels(categories["catalog"].as_str().unwrap(), "all", 0);
    let admission = f.request("play_catalog", json!({
        "catalog":page["catalog"], "id":"video-0"
    }));
    assert!(admission.is_none(), "expected queued playback, got {admission:?}");
    let request = f.app.ott.as_mut().unwrap().resolver_next.take().unwrap();
    (f, page, request)
}

fn pending_append_result(f: &mut Fixture, request: ResolveRequest) {
    let entry = request.entry.expect("exact selected entry captured by the resolver");
    f.app.ott.as_ref().unwrap().tx.send(Job::Play(
        request.generation, request.profile, Ok((entry, ReadySource::External(
            plx_media::ff::ExternalSource::Http("https://media.invalid/selected".into()),
            unsupported_info(), None,
        ))),
    )).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut f.app);
}

#[test]
fn catalog_pending_append_reaches_native_admission_and_preserves_read_pages() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for completes in [false, true] {
        let (mut f, page, request) = pending_append_fixture();
        let mut later = synthetic_entry();
        later.id = "unrelated-discovered-file".into();
        later.title = "Later discovered file".into();
        let runtime = f.app.ott.as_ref().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
            Catalog { hierarchy: None, entries: vec![later] })).unwrap_or_else(|_| panic!("catalog receiver"));
        if completes {
            runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(()))).unwrap_or_else(|_| panic!("catalog receiver"));
        }
        runtime_tick(&mut f.app);
        assert_eq!(f.app.ott.as_ref().unwrap().catalog.entries.len(), 2,
            "the unrelated row passed through the real background merge");
        let retained = f.request("channels", json!({
            "runtime":f.runtime_id(), "catalog":page["catalog"],
            "category":"all", "offset":0, "limit":200
        })).unwrap();
        assert_eq!(retained["status"], "ok");
        assert_eq!(retained["data"]["channels"], page["channels"],
            "the immutable read cursor must not expand to include a later row");
        assert_eq!(retained["data"]["total"], page["total"]);
        assert_eq!(retained["data"]["loading"], page["loading"]);
        pending_append_result(&mut f, request);
        assert_eq!(f.app.ott.as_ref().unwrap().status, "The native decoder cannot open this source",
            "a stable exact target reaches native admission after an unrelated append or scan completion");
        assert_eq!(f.app.bridge.metadata_view().now_playing().map(|info| info.title.as_str()),
            Some("Video 0"));
        assert!(f.app.ott.as_ref().unwrap().pending_play.is_none());
        assert!(!plx_media::route::has_url(&f.app.player.session));
    }
}

#[test]
fn catalog_pending_append_does_not_authorize_a_changed_selected_entry_or_source() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for change in ["entry", "source", "generation", "missing"] {
        let (mut f, _page, request) = pending_append_fixture();
        let runtime = f.app.ott.as_mut().unwrap();
        match change {
            "entry" => {
                let mut replacement = runtime.catalog.entries[0].clone();
                replacement.media = MediaLocator::VPortal { request: json!({"id":999}) };
                runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
                    Catalog { hierarchy: None, entries: vec![replacement] })).unwrap_or_else(|_| panic!("catalog receiver"));
            }
            "source" => runtime.saved.profiles.profiles[0].source =
                "portal::[key:fixture]https://changed.invalid/nas/api".into(),
            "generation" => runtime.generation = runtime.generation.wrapping_add(1),
            "missing" => {
                runtime.catalog.entries.clear();
                runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
            }
            _ => unreachable!(),
        }
        runtime_tick(&mut f.app);
        pending_append_result(&mut f, request);
        assert_eq!(f.app.ott.as_ref().unwrap().status, "Catalog changed before playback admission",
            "{change} must not reuse an earlier exact selection");
        assert!(f.app.bridge.metadata_view().now_playing().is_none());
        assert!(!plx_media::route::has_url(&f.app.player.session));
    }
}

#[test]
fn catalog_pending_append_does_not_override_a_new_native_playback_identity() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut f, _page, request) = pending_append_fixture();
    let current = install_previous(&mut f, false, false);
    let identity = plx_media::route::playback_identity(&f.app.player.session);
    // Model the newer source with its current position already persisted. The
    // ordinary end-of-tick checkpoint must not start an unrelated real writer
    // and replace the admission error with "Saving…" in this no-IO fixture.
    let position_ms = playback::intended_pos(&f.app.player.session).max(0) as u64 / 1_000_000;
    f.app.ott.as_mut().unwrap().saved.resume.insert(current.profile.id.clone(), SavedResume {
        entry_id: current.selection_id.clone(),
        position_ms,
        source_fingerprint: profile_fingerprint(&current.profile),
    });
    assert!(!f.app.ott.as_ref().unwrap().saving);
    pending_append_result(&mut f, request);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.status, "Playback request superseded");
    assert!(!runtime.saving, "the fixture must not launch the unrelated checkpoint writer");
    assert_eq!(plx_media::route::playback_identity(&f.app.player.session), identity);
    assert!(playing_matches(&current, &f.app));
    assert!(runtime.pending_play.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn catalog_pending_append_allows_presentation_enrichment_of_the_selected_row() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut f, _page, request) = pending_append_fixture();
    let runtime = f.app.ott.as_mut().unwrap();
    let mut enriched = runtime.catalog.entries[0].clone();
    enriched.title = "Enriched display title".into();
    enriched.category = "Enriched display folder".into();
    enriched.description = "Full newly loaded description".into();
    enriched.poster = "https://art.invalid/poster.jpg".into();
    enriched.duration_ms = Some(123_000);
    enriched.provider_id = Some("epg-enrichment-id".into());
    enriched.provider_id_field = Some("id".into());
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
        Catalog { hierarchy: None, entries: vec![enriched] })).unwrap_or_else(|_| panic!("catalog receiver"));
    runtime_tick(&mut f.app);
    assert_eq!(f.app.ott.as_ref().unwrap().catalog.entries[0].description,
        "Full newly loaded description", "enrichment was merged by the real receiver");
    pending_append_result(&mut f, request);
    assert_eq!(f.app.ott.as_ref().unwrap().status, "The native decoder cannot open this source",
        "presentation/EPG fields cannot revoke an unchanged exact media locator");
    assert_eq!(f.app.bridge.metadata_view().now_playing().map(|info| info.title.as_str()),
        Some("Video 0"), "the original resolved item reaches native admission");
}



#[test]
fn local_capability_probes_accept_only_true_extension_flags_without_starting_io() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(0);
    let saved = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    for params in [
        json!({"local_media_session":true}),
        json!({"local_media_progressive":true}),
        json!({"local_media_session":true,"local_media_progressive":true}),
    ] {
        let response = f.request("capabilities", params).unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["local_media"]["version"], 1);
        assert_eq!(response["data"]["local_media_session"], json!({"version":1}));
        assert_eq!(response["data"]["local_media_progressive"], json!({"version":1}));
    }
    for key in ["local_media_session", "local_media_progressive"] {
        for value in [json!(false), Value::Null, json!(1), json!("true"), json!([]), json!({})] {
            let mut params = json!({});
            params[key] = value;
            assert_eq!(f.request("capabilities", params).unwrap()["status"], "rejected");
        }
    }
    assert_eq!(
        f.request("capabilities", json!({"local_media_session":true,"extra":true})).unwrap()["status"],
        "rejected"
    );
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
}


#[test]
fn ordinary_capabilities_advertise_plex_state_v2_and_preserve_legacy_runtime() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(0);
    let runtime_id = f.runtime_id();
    let saved = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    let response = f.request("capabilities", json!({})).expect("ordinary capabilities response");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["version"], 1);
    assert_eq!(response["data"]["player"]["runtime"], runtime_id);
    assert!(!runtime_id.is_empty());
    assert_eq!(response["data"]["plex_queue"], json!({
        "version":1, "operations":["play","preview","status","next","previous","stop"],
        "max_items":500,
    }));
    assert_eq!(response["data"]["plex_queue_state_v2"], json!({
        "version":2, "operations":["play","status","next","previous","stop"],
        "max_write_items":500, "max_state_items":1000, "max_state_bytes":32768,
    }), "ordinary capabilities must advertise the implemented Plex state v2 contract");
    let state = f.request("plex_queue", json!({"op":"status","state_version":2,"runtime":runtime_id}))
        .expect("implemented state v2 status response");
    assert_eq!(state["status"], "ok");
    assert_eq!(state["data"]["version"], 2);
    assert_eq!(state["data"]["runtime"], runtime_id);
    assert_eq!(state["data"]["state"], "idle");
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
    assert_eq!(f.runtime_id(), runtime_id);
}

#[test]
fn relay_plex_state_v2_probe_flag_is_not_a_native_capability_parameter() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(0);
    let runtime_id = f.runtime_id();
    let saved = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    for params in [
        json!({"plex_queue_state_v2":true}),
        json!({"plex_queue_state_v2":false}),
        json!({"plex_queue_state_v2":null}),
        json!({"plex_queue_state_v2":true,"local_media_session":true}),
    ] {
        assert_eq!(f.request("capabilities", params).expect("native refusal")["status"],
            "rejected", "the relay consumes its probe flag before native dispatch");
    }
    let response = f.request("capabilities", json!({})).expect("ordinary request still works");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["player"]["runtime"], runtime_id);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
}

#[path = "local_sources_pending.rs"]
mod local_sources_pending;


#[test]
fn category_select_capability_matches_the_existing_controller_contract() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    let caps = f.request("capabilities", json!({})).unwrap();
    assert_eq!(caps["data"]["categories"],
        json!({"version":1,"operations":["list","select"],"maxPageSize":200}));
}

#[test]
fn category_select_admits_one_existing_writer_without_starting_media() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    {
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.entropy = Some([1;16]);
        runtime.write_permit.revoke();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
    }
    let categories = f.categories();
    let id = categories["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "Series").unwrap()["id"].clone();
    let before = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    let generation = f.app.ott.as_ref().unwrap().play_generation;
    let response = f.request("categories", json!({"op":"select","runtime":f.runtime_id(),
        "catalog":categories["catalog"],"category":id}));
    assert!(response.is_none(), "select must wait for the existing durable writer: {response:?}");
    let runtime = f.app.ott.as_mut().unwrap();
    assert!(runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    let response = runtime.remote.response_for_id_for_test(&format!("{:032x}", f.serial)).unwrap();
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"], json!({"version":1,"runtime":f.runtime_id(),"op":"select",
        "state":"error","error":"unavailable"}));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn category_select_is_not_classified_as_read_only_under_strict_kiosk() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    let runtime = f.app.ott.as_mut().unwrap();
    let origin = json!({"provider":"m3u","profile":"Fixture","subprofile":null,"sourceAlias":"Fixture source"});
    runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":true,
        "origin":origin,"target":{"kind":"item","entry":{"title":"Fixture","origin":origin,
        "locator":{"kind":"channel","channelId":"fixture"}}}}));
    assert_eq!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true }));
    let list = runtime.remote.request_for_test("categories",json!({"op":"list"}));
    // The one-shot helper uses a fixed ID. A second request must not reuse the
    // still-pending list ID, which the real delivery protocol correctly deduplicates.
    let mut select = runtime.remote.raw_poll_for_test(&json!({
        "commands": [], "server_time": 1000,
        "requests": [{"id":"1123456789abcdef0123456789abcdef", "action":"categories",
            "params":{"op":"select"}, "expires_at":1020}]
    }).to_string(), 100);
    assert_eq!(select.len(), 1, "a distinct select request must reach the policy gate");
    let select = select.remove(0);
    assert!(crate::app::ott_runtime::input_policy::remote(runtime, &list).is_ok());
    assert!(crate::app::ott_runtime::input_policy::remote(runtime, &select).is_err(),
        "mutating select must not inherit the categories list bypass");
}


// The actual Saved writer runs with a revoked permit. Tests intercept its
// pre-IO failure, then inject a modeled durable acknowledgement or uncertainty.
// They qualify admission/settlement, not protected-storage durability.
fn category_mutation_fixture() -> Fixture {
    let mut f = Fixture::new(4);
    let r = f.app.ott.as_mut().unwrap();
    r.entropy = Some([1; 16]);
    r.write_permit.revoke();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.plex_refresh_pending = true;
    r.continuity.stop(r.generation);
    r.page = "categories";
    f
}
fn stage_remote_category(f: &mut Fixture, name: &str) -> (Saved, Invocation, Value, Value) {
    let categories = f.categories();
    let wire = categories["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == name).unwrap()["id"].clone();
    let internal = if name == "All" { "" } else { name };
    let candidate = f.app.ott.as_ref().unwrap().category_choice_candidate(Some(internal)).unwrap();
    let params = json!({"op":"select", "runtime":f.runtime_id(),
        "catalog":categories["catalog"], "category":wire});
    assert!(f.request("categories", params).is_none(), "must await Saved");
    let r = f.app.ott.as_mut().unwrap();
    assert!(r.saving && r.pending_category_choice.is_some());
    let Job::Saved(Err(_), Some(inv), false, response) =
        r.rx.recv_timeout(Duration::from_secs(1)).unwrap() else { panic!("revoked Saved writer"); };
    let expected = json!({"version":1,"runtime":f.runtime_id(),"op":"select",
        "catalog":categories["catalog"],"provider":categories["provider"],
        "selected":wire,"dispatched":true});
    assert_eq!(response["data"], expected, "exact retained receipt payload");
    (candidate, inv, response, expected)
}
fn settle_remote_category(f: &mut Fixture, candidate: Saved, inv: Invocation, response: Value) -> Value {
    let id = inv.id.clone();
    f.app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(candidate), Some(inv), false, response))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(&mut f.app);
    f.app.ott.as_ref().unwrap().remote.response_for_id_for_test(&id).unwrap()
}

#[test]
fn category_select_settles_exact_receipt_and_all_without_changing_playback() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = category_mutation_fixture();
    let previous = install_previous(&mut f, false, true);
    // The unchanged playing item's bookmark is already durable. This keeps a
    // periodic checkpoint from starting an unrelated write after category ACK.
    let position_ms = playback::intended_pos(&f.app.player.session).max(0) as u64 / 1_000_000;
    f.app.ott.as_mut().unwrap().saved.resume.insert(previous.profile.id.clone(), SavedResume {
        entry_id: previous.selection_id.clone(), position_ms,
        source_fingerprint: profile_fingerprint(&previous.profile),
    });
    let identity = plx_media::route::playback_identity(&f.app.player.session);
    let source = plx_media::route::external_source(&f.app.player.session);
    let generation = f.app.ott.as_ref().unwrap().play_generation;
    let (candidate, inv, response, expected) = stage_remote_category(&mut f, "Series");
    assert!(f.app.ott.as_ref().unwrap().saved.category.is_empty());
    let outcome = settle_remote_category(&mut f, candidate, inv, response);
    assert_eq!(outcome["status"], "ok");
    assert_eq!(outcome["data"], expected);
    {
        let r = f.app.ott.as_ref().unwrap();
        assert_eq!(r.saved.category, "Series");
        assert_eq!(r.ordered_ids(), vec!["video-0", "video-2"]);
        assert_eq!(r.page, "channels");
        assert!(!r.saving && r.pending_category_choice.is_none());
        assert_eq!(r.play_generation, generation);
        assert_eq!(r.playing.as_ref().unwrap().entry.id, "previous");
        assert!(r.play_target.is_none() && r.resolver_next.is_none());
    }
    assert_eq!(plx_media::route::playback_identity(&f.app.player.session), identity);
    assert_eq!(plx_media::route::external_source(&f.app.player.session), source);
    let fresh = f.categories();
    assert_eq!(fresh["selected"], expected["selected"]);
    let page = f.channels(fresh["catalog"].as_str().unwrap(), fresh["selected"].as_str().unwrap(), 0);
    assert_eq!(page["total"], 2);
    let (candidate, inv, response, expected) = stage_remote_category(&mut f, "All");
    let outcome = settle_remote_category(&mut f, candidate, inv, response);
    assert_eq!(outcome["data"], expected);
    assert_eq!(outcome["status"], "ok");
    assert_eq!(f.app.ott.as_ref().unwrap().saved.category, "");
    assert_eq!(f.app.ott.as_ref().unwrap().ordered_ids().len(), 4);
    assert_eq!(plx_media::route::playback_identity(&f.app.player.session), identity);
}

#[test]
fn category_select_saved_choice_survives_serialization_and_profile_roundtrip() {
    let _lock = plx_base::testlock::serial();
    let mut f = category_mutation_fixture();
    let (candidate, inv, response, _) = stage_remote_category(&mut f, "Films");
    assert_eq!(settle_remote_category(&mut f, candidate, inv, response)["status"], "ok");
    let encoded = serde_json::to_vec(&f.app.ott.as_ref().unwrap().saved).unwrap();
    let restored: Saved = serde_json::from_slice(&encoded).unwrap();
    restored.validate().unwrap();
    let mut fresh = category_mutation_fixture();
    fresh.app.ott.as_mut().unwrap().saved = restored;
    let cats = fresh.categories();
    let films = cats["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "Films").unwrap()["id"].clone();
    assert_eq!(cats["selected"], films);
    let r = fresh.app.ott.as_mut().unwrap();
    let mut second = synthetic_profile();
    second.id = "second-profile".into(); second.name = "Second".into();
    second.source = "portal::[key:other]https://other.invalid/api".into();
    r.saved.profiles.profiles.push(second);
    let mut other = r.saved.clone();
    other.profiles.active_profile = Some("second-profile".into());
    category_state::prepare_profile_change(&r.saved, &mut other).unwrap();
    assert_eq!(other.category, "", "a distinct source must not inherit Films");
    r.saved = other;
    let mut back = r.saved.clone();
    back.profiles.active_profile = Some("portal-fixture".into());
    category_state::prepare_profile_change(&r.saved, &mut back).unwrap();
    assert_eq!(back.category, "Films");
}

#[test]
fn category_select_rejects_bad_runtime_receipt_membership_and_source_without_a_writer() {
    let _lock = plx_base::testlock::serial();
    for case in ["runtime", "receipt", "membership", "source", "removed", "channel-receipt", "extra"] {
        let mut f = category_mutation_fixture();
        let cats = f.categories();
        let series = cats["categories"].as_array().unwrap().iter()
            .find(|row| row["name"] == "Series").unwrap()["id"].clone();
        let mut params = json!({"op":"select","runtime":f.runtime_id(),
            "catalog":cats["catalog"],"category":series});
        let code = match case {
            "runtime" => { params["runtime"] = json!("other-runtime"); "runtime_mismatch" }
            "receipt" => { params["catalog"] = json!("catalog:unknown"); "catalog_changed" }
            "membership" => { params["category"] = json!("category:unknown"); "category_not_found" }
            "source" => {
                let r = f.app.ott.as_mut().unwrap();
                r.saved.profiles.profiles[0].source = "portal::[key:new]https://changed.invalid/api".into();
                "catalog_changed"
            }
            "removed" => {
                let r = f.app.ott.as_mut().unwrap();
                r.catalog.entries.retain(|entry| entry.category != "Series");
                r.catalog_revision = r.catalog_revision.wrapping_add(1);
                "catalog_changed"
            }
            "channel-receipt" => {
                let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
                params["catalog"] = page["catalog"].clone(); "catalog_changed"
            }
            _ => { params["extra"] = json!(true); "invalid_request" }
        };
        let before = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
        let requested_runtime = params["runtime"].clone();
        let response = f.request("categories", params).unwrap();
        assert_eq!(response["status"], "rejected", "{case}");
        assert_eq!(response["data"], json!({"version":1,"runtime":requested_runtime,
            "op":"select","state":"error","error":code}), "{case}");
        let r = f.app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
        assert!(!r.saving && r.pending_category_choice.is_none());
        assert!(r.play_target.is_none() && r.resolver_next.is_none());
    }
}

#[test]
fn category_select_distinguishes_incomplete_catalog_and_matches_ui_parental_policy() {
    let _lock = plx_base::testlock::serial();
    let mut f = category_mutation_fixture();
    {
        let r = f.app.ott.as_mut().unwrap();
        r.catalog_loading = true; r.catalog_complete = false;
    }
    let cats = f.categories();
    let response = f.request("categories", json!({"op":"select","runtime":f.runtime_id(),
        "catalog":cats["catalog"],"category":"category:not-yet-known"})).unwrap();
    assert_eq!(response["data"]["error"], "catalog_incomplete");
    {
        let r = f.app.ott.as_mut().unwrap();
        r.saved.settings.insert("parentPin".into(), json!(
            "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        r.saved.settings.insert("psOptions".into(), json!(1));
        r.retire_parental_source();
        assert!(r.require_parental(parental::Target::Settings).is_err());
    }
    // UI category navigation does not edit parental settings; remote must use
    // that same policy rather than demanding a Settings PIN merely to save.
    let (candidate, inv, response, _) = stage_remote_category(&mut f, "Series");
    assert_eq!(settle_remote_category(&mut f, candidate, inv, response)["status"], "ok");
    assert!(f.app.ott.as_ref().unwrap().edit_session.is_none());
}

#[test]
fn category_select_late_saved_cannot_reclaim_navigation_or_claim_dispatch() {
    let _lock = plx_base::testlock::serial();
    for case in ["navigation", "source-generation", "deadline"] {
        let mut f = category_mutation_fixture();
        let (candidate, mut inv, response, _) = stage_remote_category(&mut f, "Series");
        let r = f.app.ott.as_mut().unwrap();
        r.page = "profiles";
        match case {
            "navigation" => r.accepted_navigation(false),
            "source-generation" => r.generation = r.generation.wrapping_add(1),
            _ => inv.deadline_ms = 0, // Model a write acknowledgement after its deadline.
        }
        let outcome = settle_remote_category(&mut f, candidate, inv, response);
        assert_eq!(outcome["status"], "rejected", "{case}");
        assert_eq!(outcome["data"]["error"], if case == "deadline" { "unavailable" } else { "catalog_changed" });
        let r = f.app.ott.as_ref().unwrap();
        assert_eq!(r.page, "profiles", "{case}: late Saved cannot navigate");
        assert_eq!(r.saved.category, "Series", "already durable choice is not falsely rolled back");
        assert!(!r.saving && r.pending_category_choice.is_none());
        assert!(r.play_target.is_none() && r.resolver_next.is_none());
    }
}

#[test]
fn category_select_uncertain_saved_has_typed_error_and_no_browse_effect() {
    let _lock = plx_base::testlock::serial();
    let mut f = category_mutation_fixture();
    let (candidate, inv, _, _) = stage_remote_category(&mut f, "Series");
    let id = inv.id.clone();
    f.app.ott.as_ref().unwrap().tx.send(Job::Uncertain(Some(candidate), Some(inv)))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(&mut f.app);
    let r = f.app.ott.as_ref().unwrap();
    let response = r.remote.response_for_id_for_test(&id).unwrap();
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"], json!({"version":1,"runtime":f.runtime_id(),"op":"select",
        "state":"error","error":"unavailable"}));
    assert!(r.persistence_uncertain && r.pending_category_choice.is_none());
    assert_eq!(r.page, "categories");
    assert!(r.play_target.is_none() && r.resolver_next.is_none());
}


#[test]
fn category_select_favorites_all_retains_its_own_catalog_after_saved() {
    let _lock = plx_base::testlock::serial();
    let mut f = category_mutation_fixture();
    {
        let r = f.app.ott.as_mut().unwrap();
        let favorite = selection::capture(&synthetic_profile(), &synthetic_entry()).unwrap();
        r.saved.favorites.toggle(favorite).unwrap();
        r.saved.favorites_selected = true;
        r.saved.category = "Series".into();
    }
    let cats = f.categories();
    assert_eq!(cats["provider"], "favorites");
    assert_eq!(cats["categories"].as_array().unwrap().len(), 1);
    let before = f.app.ott.as_ref().unwrap().saved.favorites.clone();
    let candidate = f.app.ott.as_ref().unwrap().category_choice_candidate(None).unwrap();
    let params = json!({"op":"select","runtime":f.runtime_id(),
        "catalog":cats["catalog"],"category":"all"});
    assert!(f.request("categories", params).is_none());
    let Job::Saved(Err(_), Some(inv), false, response) = f.app.ott.as_ref().unwrap()
        .rx.recv_timeout(Duration::from_secs(1)).unwrap() else { panic!("revoked writer"); };
    let result = settle_remote_category(&mut f, candidate, inv, response);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"], json!({"version":1,"runtime":f.runtime_id(),
        "op":"select","catalog":cats["catalog"],"provider":"favorites",
        "selected":"all","dispatched":true}));
    let r = f.app.ott.as_ref().unwrap();
    assert!(r.saved.favorites_selected);
    assert_eq!(r.saved.category, "Series");
    assert_eq!(r.saved.favorites, before);
    assert_eq!(r.page, "favorites");
    assert_eq!(f.categories()["provider"], "favorites");
}

#[test]
fn category_select_expiry_and_strict_policy_use_exact_typed_errors_at_command_boundary() {
    let _lock = plx_base::testlock::serial();
    for expired in [false, true] {
        let mut f = category_mutation_fixture();
        let cats = f.categories();
        let mut r = f.app.ott.take().unwrap();
        let mut inv = r.remote.request_for_test("categories", json!({"op":"select",
            "runtime":r.remote.runtime(),"catalog":cats["catalog"],"category":"all"}));
        inv.deadline_ms = if expired { 0 } else { r.now_ms() + 10_000 };
        if !expired {
            let origin = json!({"provider":"m3u","profile":"Fixture","subprofile":null,
                "sourceAlias":"Fixture source"});
            r.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":true,
                "origin":origin,"target":{"kind":"item","entry":{"title":"Fixture",
                "origin":origin,"locator":{"kind":"channel","channelId":"fixture"}}}}));
            assert_eq!(r.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true }));
        }
        let saved = serde_json::to_value(&r.saved).unwrap();
        let response = commands::command(&mut r, &mut f.app, &inv).unwrap();
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"], json!({"version":1,"runtime":inv.params["runtime"],
            "op":"select","state":"error","error":if expired { "unavailable" } else { "restricted" }}));
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
        assert!(!r.saving && r.pending_category_choice.is_none());
        f.app.ott = Some(r);
    }
}

#[test]
fn category_select_completion_accepts_additive_batches_but_not_removed_category() {
    let _lock = plx_base::testlock::serial();
    for removed in [false, true] {
        let mut f = category_mutation_fixture();
        let (candidate, inv, response, _) = stage_remote_category(&mut f, "Series");
        let r = f.app.ott.as_mut().unwrap();
        if removed {
            r.catalog.entries.retain(|entry| entry.category != "Series");
        } else {
            let mut new = synthetic_entry(); new.id = "later-video".into();
            new.category = "Later".into(); r.catalog.entries.push(new);
        }
        r.catalog_revision = r.catalog_revision.wrapping_add(1);
        let outcome = settle_remote_category(&mut f, candidate, inv, response);
        assert_eq!(outcome["status"], if removed { "rejected" } else { "ok" });
        if removed {
            assert_eq!(outcome["data"]["error"], "catalog_changed");
            assert_eq!(f.app.ott.as_ref().unwrap().page, "categories");
        }
    }
}

#[test]
fn continuity_sync_refusal_clears_candidate_and_standby_cannot_restore_it() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1000);
    {
        let runtime = f.app.ott.as_mut().unwrap();
        // A source removed after queueing makes every prepare fail synchronously.
        // No resolver or decoder can run, even if the failure budget regresses.
        runtime.saved.profiles.profiles.clear();
        runtime.saved.profiles.active_profile = None;
        runtime.play_target = Some(PlaybackRequest::ordinary("video-0", 0));
        runtime.continuity.user_selected(runtime.generation, "video-0");
        runtime.apply_continuity(continuity::Decision::Start {
            entry_id: "video-0".into(), position_ms: 0, reason: continuity::Reason::Next,
        });
        assert!(runtime.play_target.is_none(), "a refused candidate must not become wake's resume_pending");
        assert!(runtime.resolver_next.is_none());
        assert!(!runtime.continuity.retry_due(runtime.generation, runtime.now_ms()));
        assert!(runtime.continuity.retry_due(runtime.generation, runtime.now_ms() + 1000));
    }
    f.app.ott_standby = true;
    super::super::suspend(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().resume_pending.is_none());
    f.app.ott_standby = false;
    super::super::resume(&mut f.app);
    let runtime = f.app.ott.as_mut().unwrap();
    let ids = runtime.ordered_ids();
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    assert_eq!(runtime.continuity.tick(runtime.generation, u64::MAX, &refs, true), continuity::Decision::Idle);
    assert_eq!(runtime.continuity.catalog_updated(runtime.generation, &refs, true), continuity::Decision::Idle);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resume_pending.is_none());
}

#[test]
fn continuity_async_failure_waits_on_frame_clock_and_stop_cancels_it() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1000);
    {
        let runtime = f.app.ott.as_mut().unwrap();
        let ids = runtime.ordered_ids();
        let refs: Vec<_> = ids.iter().map(String::as_str).collect();
        runtime.continuity.user_selected(runtime.generation, "video-0");
        // Model the first failed lookup, then feed the real Job::Play second
        // failure to the runtime. It must park, never launch the third resolver.
        let continuity::Decision::Start { entry_id, .. } = runtime.continuity.start_failed(
            runtime.generation, "video-0", &refs, true, runtime.now_ms()) else {
            panic!("an isolated failure must still skip once");
        };
        runtime.play_target = Some(PlaybackRequest::ordinary(&entry_id, 0));
        runtime.tx.send(Job::Play(runtime.play_generation, synthetic_profile(),
            Err("Fixture lookup refused"))).unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut f.app);
    {
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none());
        assert!(runtime.resolver_next.is_none());
        assert_eq!(runtime.status, "Fixture lookup refused");
        assert!(!runtime.continuity.retry_due(runtime.generation, runtime.now_ms() + 999));
        assert!(runtime.continuity.retry_due(runtime.generation, runtime.now_ms() + 1000));
    }
    let mut runtime = f.app.ott.take().unwrap();
    runtime.stop_catalog(&mut f.app);
    let ids = runtime.ordered_ids();
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    assert_eq!(runtime.continuity.tick(runtime.generation, u64::MAX, &refs, true), continuity::Decision::Idle);
    assert_eq!(runtime.continuity.catalog_updated(runtime.generation, &refs, true), continuity::Decision::Idle);
    assert!(runtime.play_target.is_none());
    f.app.ott = Some(runtime);
}

#[test]
fn receipt_random_v2_admits_complete_channel_receipt_only() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(4);
    f.app.ott.as_mut().unwrap().entropy = Some([7;16]);
    let cats = f.categories();
    let invalid = f.request("play_catalog", json!({"catalog":cats["catalog"],
        "receipt_random":{"version":2,"scope":"receipt"}})).unwrap();
    assert_eq!(invalid["status"], "rejected");
    let series = cats["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "Series").unwrap()["id"].as_str().unwrap();
    let page = f.channels(cats["catalog"].as_str().unwrap(), series, 0);
    let result = f.request("play_catalog", json!({"catalog":page["catalog"],
        "receipt_random":{"version":2,"scope":"receipt"}}));
    assert!(result.is_none(), "accepted playback is asynchronous: {result:?}");
    let runtime = f.app.ott.as_ref().unwrap();
    let chosen = runtime.play_target.as_ref().unwrap();
    assert!(chosen.id == "video-0" || chosen.id == "video-2");
    assert!(!chosen.advance_on_failure);
}

#[test]
fn receipt_random_v2_refuses_unknown_semantics_and_loading() {
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(2);
    f.app.ott.as_mut().unwrap().entropy = Some([7;16]);
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(), "all", 0);
    for options in [json!({"version":1,"scope":"receipt"}),
        json!({"version":2,"scope":"all"}),
        json!({"version":2,"scope":"receipt","search":"Video"})] {
        assert_eq!(f.request("play_catalog", json!({"catalog":page["catalog"],
            "receipt_random":options})).unwrap()["status"], "rejected");
    }
    f.app.ott.as_mut().unwrap().catalog_loading = true;
    assert_eq!(f.request("play_catalog", json!({"catalog":page["catalog"],
        "receipt_random":{"version":2,"scope":"receipt"}})).unwrap()["status"], "rejected");
    assert!(f.app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn private_archive_source_never_enters_catalog_guide_or_portable_favorites() {
    use plx_data::ott::{archive::{CatchupMode,CatchupSource},guide::{ChannelMetadata,Coverage,Context,Query,ResultSet}};
    let _lock = plx_base::testlock::serial();
    let mut f = Fixture::new(1);
    let secret = "private-archive-facts-token-should-never-export";
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.catalog.entries[0].live = Some(ChannelMetadata { guide_id:None,guide_feed:String::new(),tvg_name:String::new(),guide_shift_seconds:0,
        catchup:Some(CatchupSource { mode:CatchupMode::Provider,source:secret.into(),retention_seconds:86400,correction_seconds:0 }) });
    let favorite = selection::capture(&runtime.saved.profiles.profiles[0], &runtime.catalog.entries[0]).unwrap();
    assert!(!serde_json::to_string(&favorite).unwrap().contains(secret));
    let cats = f.categories();
    let page = f.channels(cats["catalog"].as_str().unwrap(),"all",0);
    assert!(!cats.to_string().contains(secret));
    assert!(!page.to_string().contains(secret));
    let result = ResultSet { context:Context {generation:1,profile_id:"profile".into(),entry_id:"item".into()},
        query:Query::Day {start:0,end:86400},coverage:Coverage::RequestedWindow,programmes:Vec::new() };
    // The public guide envelope serializes presentation facts, never ChannelMetadata.
    let wire = json!({"query":result.query,"coverage":result.coverage,"programmes":result.programmes});
    assert!(!wire.to_string().contains(secret));
}

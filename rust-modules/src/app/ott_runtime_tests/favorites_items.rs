//! Actual command/Delivery and Job::Play boundaries; no protected writer or resolver is run.
use super::super::favorites_items as items;
use super::super::playing_favorite;
use super::*;
use plx_data::ott::favorites::{Locator, Origin};

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn setup() -> App {
    let mut app = app_fixture();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saving = false;
    runtime.resume_source_work();
    runtime.resolver_busy = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    app
}
fn row(n: usize) -> Favorite {
    Favorite {
        title: format!("Film {n}"),
        origin: Origin {
            provider: "plex".into(),
            profile: "Home".into(),
            subprofile: None,
            source_alias: "Server".into(),
        },
        locator: Locator::Plex {
            server_id: "synthetic-server".into(),
            rating_key: n.to_string(),
        },
    }
}
fn store(runtime: &mut Runtime, rows: Vec<Favorite>) {
    runtime.saved.favorites = serde_json::from_value(json!({"version":1,"entries":rows})).unwrap();
    runtime.saved.favorite_items=Some(serde_json::from_value(json!({"version":1,"rows":runtime.saved.favorites.entries().iter().enumerate().map(|(i,f)|
        json!({"identity":plx_data::ott::favorites_wire::hash(f.identity().as_bytes()),"id":format!("00000000-0000-4000-8000-{:012x}",i+1)})).collect::<Vec<_>>()})).unwrap());
}
fn invoke(app: &mut App, serial: u64, action: &str, params: Value) -> (Invocation, Option<Value>) {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020}]})
    .to_string();
    let mut values = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(
        values.len(),
        1,
        "each request must cross Delivery with a unique id"
    );
    let inv = values.remove(0);
    assert!(runtime.remote.invocation_is_current(&inv));
    assert!(inv.deadline_ms > now);
    let result = commands::command(&mut runtime, app, &inv);
    if let Some(response) = &result {
        runtime.remote.complete(inv.clone(), response.clone());
    }
    app.ott = Some(runtime);
    (inv, result)
}
fn list(app: &mut App, serial: u64, cursor: Value, count: usize) -> Value {
    invoke(app,serial,items::ACTION,json!({"runtime":"fixture","op":"list","subject":"global","cursor":cursor,"max_entries":count})).1.unwrap()
}
fn play_params(page: &Value) -> Value {
    json!({"runtime":"fixture","op":"play","subject":"global","id":page["data"]["entries"][0]["id"],
        "expected_revision":page["data"]["revision"],"expected_context_revision":page["data"]["context_revision"]})
}
fn configure_channel(runtime: &mut Runtime) -> (ProviderProfile, CatalogEntry) {
    let mut profile = synthetic_profile();
    profile.kind = ProviderKind::M3u;
    profile.source = "https://catalog.invalid/list.m3u".into();
    profile.source_alias = "Channels".into();
    let mut entry = synthetic_entry();
    entry.id = "channel-42".into();
    entry.provider_id = Some("42".into());
    entry.media = MediaLocator::Http {
        url: "https://media.invalid/video.mp4".into(),
    };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![entry.clone()];
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    let f = selection::capture_bound(
        &selection::SelectionBindings::profiles_only(&runtime.saved.profiles),
        selection::SelectionOwner::Profile(&profile),
        &entry,
    )
    .unwrap();
    store(runtime, vec![f]);
    (profile, entry)
}
#[test]
fn capability_is_exact_and_empty_list_does_not_write_or_allocate() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let cap = invoke(&mut app, 1, "capabilities", json!({})).1.unwrap();
    assert_eq!(cap["data"]["favorites_global_items"], json!({"version":1}));
    let page = list(&mut app, 2, Value::Null, 100);
    assert_eq!(page["status"], "ok");
    assert_eq!(page["data"]["total"], 0);
    assert_eq!(page["data"]["entries"], json!([]));
    assert_eq!(page["data"]["next_cursor"], Value::Null);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(!r.saving);
    assert!(r.rx.try_recv().is_err());
}
#[test]
fn pagination_is_complete_stable_and_fenced_against_edits() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    store(app.ott.as_mut().unwrap(), (1..=205).map(row).collect());
    let first = list(&mut app, 1, Value::Null, 100);
    assert_eq!(first["data"]["entries"].as_array().unwrap().len(), 100);
    let second = list(&mut app, 2, first["data"]["next_cursor"].clone(), 100);
    assert_eq!(second["data"]["page_offset"], 100);
    let third = list(&mut app, 3, second["data"]["next_cursor"].clone(), 100);
    assert_eq!(third["data"]["entries"].as_array().unwrap().len(), 5);
    assert_eq!(third["data"]["next_cursor"], Value::Null);
    assert_eq!(first["data"]["revision"], third["data"]["revision"]);
    app.ott.as_mut().unwrap().catalog_revision += 1;
    assert_eq!(
        list(&mut app, 4, first["data"]["next_cursor"].clone(), 100)["data"]["error"],
        "context_changed"
    );
    store(app.ott.as_mut().unwrap(), vec![row(500)]);
    assert_eq!(
        list(&mut app, 5, first["data"]["next_cursor"].clone(), 100)["data"]["error"],
        "revision_conflict"
    );
}
#[test]
fn invalid_envelopes_and_legacy_ids_fail_without_changing_saved() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    app.ott.as_mut().unwrap().saved.favorites =
        serde_json::from_value(json!({"version":1,"entries":[row(1)]})).unwrap();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert_eq!(
        list(&mut app, 1, Value::Null, 100)["data"]["error"],
        "storage_unavailable"
    );
    assert_eq!(
        list(&mut app, 2, Value::Null, 101)["data"]["error"],
        "invalid_request"
    );
    let bad = invoke(
        &mut app,
        3,
        items::ACTION,
        json!({"version":1,"runtime":"fixture","op":"current","subject":"global"}),
    )
    .1
    .unwrap();
    assert_eq!(bad["data"]["error"], "invalid_request");
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
}
#[test]
fn exact_favorite_play_uses_existing_resolver_and_failure_never_skips() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    configure_channel(app.ott.as_mut().unwrap());
    let page = list(&mut app, 1, Value::Null, 100);
    let (inv, response) = invoke(&mut app, 2, items::ACTION, play_params(&page));
    assert!(response.is_none());
    let r = app.ott.as_mut().unwrap();
    assert!(!r.play_target.as_ref().unwrap().advance_on_failure);
    let work = r
        .resolver_next
        .take()
        .expect("real ordinary Favorite resolver request");
    r.tx.send(Job::Play(
        work.generation,
        work.profile,
        Err("fixture source unavailable"),
    ))
    .unwrap();
    runtime_tick(&mut app);
    let r = app.ott.as_ref().unwrap();
    let reply = r.remote.response_for_id_for_test(&inv.id).unwrap();
    assert_eq!(reply["status"], "rejected");
    assert_eq!(reply["data"]["error"], "playback_failed");
    assert_eq!(reply["data"]["op"], "play");
    assert!(r.resolver_next.is_none());
    assert!(r.play_target.is_none());
    assert!(!reply.to_string().contains("fixture source unavailable"));
}
#[test]
fn pending_cas_and_cancellation_have_typed_errors_before_native_admission() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for (changed, expected) in [(true, "context_changed"), (false, "cancelled")] {
        let mut app = setup();
        configure_channel(app.ott.as_mut().unwrap());
        let page = list(&mut app, 1, Value::Null, 100);
        let (inv, response) = invoke(&mut app, 2, items::ACTION, play_params(&page));
        assert!(response.is_none());
        let r = app.ott.as_mut().unwrap();
        let work = r.resolver_next.take().unwrap();
        if changed {
            r.catalog_revision += 1;
            r.tx.send(Job::Play(
                work.generation,
                work.profile,
                Err("would fail later"),
            ))
            .unwrap();
            runtime_tick(&mut app);
        } else {
            r.cancel_play_request();
        }
        let r = app.ott.as_ref().unwrap();
        let reply = r.remote.response_for_id_for_test(&inv.id).unwrap();
        assert_eq!(reply["data"]["error"], expected);
        assert!(!plx_media::route::has_url(&app.player.session));
    }
}
#[test]
fn current_is_not_selected_row_and_standby_is_explicit() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let (_, entry) = configure_channel(app.ott.as_mut().unwrap());
    app.ott.as_mut().unwrap().selected = Some(entry.id);
    let params = json!({"runtime":"fixture","op":"current","subject":"global"});
    let result = invoke(&mut app, 1, items::ACTION, params.clone())
        .1
        .unwrap();
    assert_eq!(result["data"]["entry"], Value::Null);
    assert_eq!(
        result["data"]["unavailable_reason"],
        "no_portable_current_item"
    );
    app.ott_standby = true;
    assert_eq!(
        invoke(&mut app, 2, items::ACTION, params).1.unwrap()["data"]["unavailable_reason"],
        "standby"
    );
}
#[test]
fn uncertain_id_migration_keeps_favorites_visible_and_blocks_writes_and_items() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let mut saved = Saved::default();
    saved.favorites = serde_json::from_value(json!({"version":1,"entries":[row(1)]})).unwrap();
    let document = saved.favorites.clone();
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::LoadedItemsUncertain(saved, [1; 16]))
        .unwrap();
    runtime_tick(&mut app);
    let r = app.ott.as_mut().unwrap();
    assert!(r.loaded);
    assert!(r.persistence_uncertain);
    assert_eq!(r.saved.favorites, document);
    assert!(r.save(r.saved.clone(), None, false).is_err());
    assert_eq!(
        list(&mut app, 1, Value::Null, 100)["data"]["error"],
        "storage_unavailable"
    );
}
#[test]
fn list_preserves_all_portable_kinds_and_reduces_pages_by_bytes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let mut rows = vec![row(1)];
    for (provider, locator) in [
        (
            "m3u",
            Locator::Channel {
                channel_id: "42".into(),
            },
        ),
        (
            "m3u",
            Locator::ChannelName {
                name: "Fallback".into(),
                group: None,
            },
        ),
        (
            "m3u",
            Locator::VPortal {
                id_field: "id".into(),
                id: "embedded".into(),
            },
        ),
        (
            "vportal",
            Locator::VPortal {
                id_field: "fid".into(),
                id: "standalone".into(),
            },
        ),
    ] {
        let mut f = row(2);
        f.origin.provider = provider.into();
        f.locator = locator;
        rows.push(f);
    }
    for n in 0..30 {
        let mut f = row(n + 2);
        f.origin.provider = "local".into();
        f.locator = Locator::Local {
            root_alias: "Offline".into(),
            relative_path: format!("{}/clip{n}.mp4", "é".repeat(4000)),
        };
        rows.push(f);
    }
    let expected = rows.clone();
    store(app.ott.as_mut().unwrap(), rows);
    let mut cursor = Value::Null;
    let mut found = Vec::new();
    let mut serial = 1;
    loop {
        let page = list(&mut app, serial, cursor, 100);
        serial += 1;
        assert_eq!(page["status"], "ok");
        assert!(page["data"].to_string().len() <= 65536);
        for row in page["data"]["entries"].as_array().unwrap() {
            found.push(serde_json::from_value::<Favorite>(row["entry"].clone()).unwrap());
        }
        cursor = page["data"]["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(serial < 20, "bounded fixture pages make progress");
    }
    assert_eq!(found, expected);
    assert!(
        serial > 2,
        "byte cap must reduce the requested hundred rows"
    );
}
#[test]
fn current_uses_admitted_native_source_and_does_not_toggle_favorites() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = local_integration::setup();
    app.running = true;
    local_integration::install(&mut app, None, "admitted");
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    let r = app.ott.as_mut().unwrap();
    r.saving = false;
    r.loaded = true;
    // Model the complete owner catalogue at admission, as in playing_favorite::ready.
    // Browsing remains in the unrelated, incomplete Local source.
    let playing = r.playing.as_ref().unwrap();
    assert!(!r.catalog_complete);
    assert!(r.effective_profile() != Some(&playing.profile));
    let complete_source = plx_data::ott::Catalog {
        hierarchy: None,
        entries: vec![playing.entry.clone()],
    };
    let bindings = selection::SelectionBindings::profiles_only(&r.saved.profiles);
    let proof = selection::capture_channel(
        &bindings,
        selection::SelectionOwner::Profile(&playing.profile),
        &complete_source.entries,
        &playing.entry,
    )
    .unwrap()
    .expect("the admitted M3u channel retains its complete owner catalogue proof");
    r.playing.as_mut().unwrap().channel_capture = Some(proof);
    r.selected = Some("different-browsed-row".into());
    let expected = playing_favorite::capture(r, r.playing.as_ref().unwrap()).unwrap();
    let before = serde_json::to_value(&r.saved).unwrap();
    let response = invoke(
        &mut app,
        1,
        items::ACTION,
        json!({"runtime":"fixture","op":"current","subject":"global"}),
    )
    .1
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"]["entry"],
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(response["data"]["unavailable_reason"], Value::Null);
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
}
#[test]
fn cross_profile_request_keeps_original_fence_until_unrelated_context_changes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let (target, _) = configure_channel(app.ott.as_mut().unwrap());
    let r = app.ott.as_mut().unwrap();
    let mut other = target.clone();
    other.id = "other-profile".into();
    other.name = "Other".into();
    other.source_alias = "Other".into();
    r.saved.profiles.profiles.push(other.clone());
    r.saved.profiles.active_profile = Some(other.id);
    let page = list(&mut app, 1, Value::Null, 100);
    let (_, response) = invoke(&mut app, 2, items::ACTION, play_params(&page));
    assert!(response.is_none());
    let r = app.ott.as_mut().unwrap();
    assert_eq!(r.resolver_next.as_ref().unwrap().profile, target);
    assert!(
        items::pending_error(r).is_none(),
        "queueing a cross-profile Favorite must not invalidate its own fence"
    );
    r.saved
        .settings
        .insert("profileSources".into(), json!({"unrelated":"edit"}));
    assert_eq!(items::pending_error(r), Some("context_changed"));
    r.cancel_play_request();
}
#[test]
fn elapsed_pending_request_rejects_before_the_native_route() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    configure_channel(app.ott.as_mut().unwrap());
    let page = list(&mut app, 1, Value::Null, 100);
    let (inv, response) = invoke(&mut app, 2, items::ACTION, play_params(&page));
    assert!(response.is_none());
    let r = app.ott.as_mut().unwrap();
    r.pending_play.as_mut().unwrap().deadline_ms = 0;
    let work = r.resolver_next.take().unwrap();
    r.tx.send(Job::Play(
        work.generation,
        work.profile,
        Err("late resolver"),
    ))
    .unwrap();
    runtime_tick(&mut app);
    let response = app
        .ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_id_for_test(&inv.id)
        .unwrap();
    assert_eq!(response["data"]["error"], "expired");
    assert!(!plx_media::route::has_url(&app.player.session));
}


// The following regressions cross the real queued Job::Play handler. Only the
// decoder resource preparation is substituted, as in successor_navigation.
// The provider worker's inspected reply is supplied; no Favorite proof is made
// or assigned by the fixture, and no network or decoder is started.
#[derive(Default)]
struct CaptureResources { starts: usize }
impl playback::PlaybackResources for CaptureResources {
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
        true
    }
}

fn capture_job_tick(app: &mut App, resources: &mut CaptureResources) {
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .expect("host fixture clock");
    super::super::tick_with_start(app, clock, |runtime, app, profile, entry, source| {
        runtime.start_resolved_with_kiosk_using(app, profile, entry, source,
            |runtime, app, profile, entry, source|
                super::super::start_resolved_with(runtime, app, profile, entry, source, resources))
    });
}

fn imported_capture_job() -> (App, ResolveRequest, CatalogEntry, Favorite) {
    let mut app = setup();
    let (profile, entry) = configure_channel(app.ott.as_mut().unwrap());
    let r = app.ott.as_mut().unwrap();
    let imported = r.saved.favorites.entries()[0].clone();
    let mut other = profile;
    other.id = "unrelated-profile".into(); other.name = "Unrelated".into();
    other.source_alias = "Unrelated".into(); other.source = "https://unrelated.invalid/list.m3u".into();
    r.saved.profiles.active_profile = Some(other.id.clone());
    r.saved.profiles.profiles.push(other);
    r.catalog.entries.clear(); r.catalog_complete = true; r.catalog_dirty = false;
    r.saved.favorites_selected = true;
    r.continuity.stop(r.generation);
    r.write_permit.revoke(); r.plex_refresh_requested = false;
    r.play_at(&format!("favorite:{}", imported.identity()), 0).unwrap();
    let request = r.resolver_next.take().expect("real Favorite request queued for worker");
    assert!(request.entry.is_none(), "Favorite must cross its resolver, not a cached browse row");
    assert_eq!(request.favorite.as_ref(), Some(&imported));
    assert!(r.playing.is_none(), "no preinstalled or synthetic Favorite proof");
    (app, request, entry, imported)
}

fn capture_ready_source(entry: &CatalogEntry) -> ReadySource {
    let MediaLocator::Http { url } = &entry.media else { panic!("HTTP fixture") };
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    ReadySource::External(plx_media::ff::ExternalSource::Http(
        plx_data::ott::http_url(url).unwrap()), info, None)
}

#[test]
fn successful_imported_job_retains_its_favorite_without_borrowing_browse_catalogue() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (mut app, request, entry, imported) = imported_capture_job();
    let source = capture_ready_source(&entry);
    let expected_profile = request.profile.clone();
    app.ott.as_ref().unwrap().tx.send(Job::Play(request.generation, request.profile,
        Ok((entry.clone(), source)))).unwrap_or_else(|_| panic!("fixture receiver"));
    let mut resources = CaptureResources::default();
    capture_job_tick(&mut app, &mut resources);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(resources.starts, 1, "the Job must reach native admission exactly once");
    let playing = r.playing.as_ref().expect("successful Job installed its source");
    assert_eq!(playing.profile, expected_profile); assert_eq!(playing.entry, entry);
    assert!(playing_matches(playing, &app));
    assert!(r.catalog.entries.is_empty());
    assert_ne!(r.effective_profile(), Some(&playing.profile));
    assert!(playing.channel_capture.is_some(), "only the successful Job creates retained evidence");
    assert_eq!(playing_favorite::capture(r, playing).unwrap(), imported);
    let (candidate, _, added) = playing_favorite::candidate(r, playing).unwrap();
    assert!(!added, "fullscreen toggle recognizes the imported identity");
    assert!(candidate.favorites.entries().is_empty());
    assert_eq!(r.saved.favorites.entries(), &[imported], "candidate computation never writes");
    assert!(r.play_target.is_none());
}

#[test]
fn rejected_imported_jobs_never_create_capture_proof_or_start_decoder_resources() {
    let _serial = plx_base::testlock::serial();
    for change in ["failure", "generation", "source", "prepared-favorite"] {
        let _reset = Reset;
        let (mut app, request, entry, _) = imported_capture_job();
        let r = app.ott.as_mut().unwrap();
        match change {
            "generation" => r.cancel_play_request(),
            "source" => r.saved.profiles.profiles.iter_mut()
                .find(|profile| profile.id == request.profile.id).unwrap()
                .source = "https://replacement.invalid/list.m3u".into(),
            "prepared-favorite" => {
                let identity = request.favorite.as_ref().unwrap().identity();
                r.saved.favorites.remove(&identity);
            }
            _ => {}
        }
        let result = if change == "failure" { Err("fixture resolver failure") }
            else { Ok((entry.clone(), capture_ready_source(&entry))) };
        r.tx.send(Job::Play(request.generation, request.profile, result))
            .unwrap_or_else(|_| panic!("fixture receiver"));
        let mut resources = CaptureResources::default();
        capture_job_tick(&mut app, &mut resources);
        let r = app.ott.as_ref().unwrap();
        assert_eq!(resources.starts, 0, "{change}");
        assert!(r.playing.is_none(), "{change} cannot install a source or its capture proof");
        if change == "source" { assert_eq!(r.status, "Playback source changed before native admission"); }
        if change == "prepared-favorite" { assert_eq!(r.status, "Favorite no longer exists"); }
        assert!(r.resolver_next.is_none(), "{change} must not create an unrelated successor");
    }
}

#[test]
fn list_accepts_equivalent_integral_json_page_sizes_without_writes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    store(app.ott.as_mut().unwrap(), (1..=101).map(row).collect());
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let mut serial = 1;
    for (literal, count) in [("1", 1), ("1.0", 1), ("1e0", 1), ("100", 100), ("100.0", 100), ("1e2", 100)] {
        let expected = list(&mut app, serial, Value::Null, count);
        serial += 1;
        let params = json!({"runtime":"fixture", "op":"list", "subject":"global",
            "cursor":null, "max_entries":serde_json::from_str::<Value>(literal).unwrap()});
        let actual = invoke(&mut app, serial, items::ACTION, params).1.unwrap();
        serial += 1;
        assert_eq!(actual["status"], "ok", "{literal}: {actual}");
        assert_eq!(actual["data"], expected["data"], "{literal}");
    }
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn list_rejects_fractional_out_of_range_and_non_number_page_sizes_without_writes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    store(app.ott.as_mut().unwrap(), vec![row(1)]);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for (offset, literal) in ["0", "0.0", "-0.0", "-1", "0.5", "1.5", "100.0001", "101", "1e100", "\"1\"", "true", "null", "[]", "{}"].iter().enumerate() {
        let params = json!({"runtime":"fixture", "op":"list", "subject":"global",
            "cursor":null, "max_entries":serde_json::from_str::<Value>(literal).unwrap()});
        let actual = invoke(&mut app, offset as u64 + 1, items::ACTION, params).1.unwrap();
        assert_eq!(actual["data"]["error"], "invalid_request", "{literal}: {actual}");
    }
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

//! Real command ingress and sole-writer settlement. Revoked WritePermit proves
//! pre-I/O failure; modeled Saved/Unknown test publication, not disk durability.
use super::*;

mod status_privacy {
    use super::*;
    use crate::app::ott_kiosk_stored::Stored;
    use plx_data::ott::favorites::{Favorite, Locator, Origin};

    fn local_item() -> Favorite {
        Favorite {
            title: "Public film title".into(),
            origin: Origin {
                provider: "local".into(),
                profile: "Private profile fixture".into(),
                subprofile: None,
                source_alias: "Private source fixture".into(),
            },
            locator: Locator::Local {
                root_alias: "Private root fixture".into(),
                relative_path: "Private folder fixture/clip.mp4".into(),
            },
        }
    }

    fn install_pin(app: &mut App, favorite: Favorite) {
        app.ott.as_mut().unwrap().saved.settings.insert(
            "nativeKiosk".into(), Stored::item(favorite, true).unwrap().encode().unwrap(),
        );
    }

    fn status(app: &mut App, id: u64) -> Value {
        let mut runtime = app.ott.take().unwrap();
        let now = runtime.now_ms();
        let raw = json!({"commands":[],"server_time":1000,"requests":[{
            "id":format!("{id:032x}"),"action":"kiosk",
            "params":{"mode":"status"},"expires_at":1020
        }]}).to_string();
        let mut requests = runtime.remote.raw_poll_for_test(&raw, now);
        assert_eq!(requests.len(), 1);
        let invocation = requests.pop().unwrap();
        let id = invocation.id.clone();
        let response = commands::command(&mut runtime, app, &invocation).unwrap();
        runtime.remote.complete(invocation, response);
        let response = runtime.remote.response_for_id_for_test(&id).unwrap();
        app.ott = Some(runtime);
        response
    }

    fn channel_id(response: &Value) -> &str {
        assert_eq!(response["status"], "ok");
        response["data"]["channel"]["id"].as_str().unwrap()
    }

    #[test]
    fn item_status_does_not_export_private_locator_or_origin() {
        let _serial = plx_base::testlock::serial();
        let mut app = fixture();
        let favorite = local_item();
        let raw_identity = favorite.identity();
        install_pin(&mut app, favorite);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let response = status(&mut app, 1);
        let id = channel_id(&response);
        assert_ne!(id, raw_identity, "status ID must not be the private Favorites identity");
        assert!(serde_json::from_str::<Value>(id).is_err());
        for private in ["Private profile fixture", "Private source fixture", "Private root fixture",
            "Private folder fixture", "clip.mp4", "relativePath", "rootAlias", "sourceAlias"] {
            assert!(!response.to_string().contains(private), "private metadata: {private}");
        }
        assert_eq!(response["data"]["channel"]["name"], "Public film title");
        assert_eq!(response["data"]["provider"], "offline");
        assert_eq!(response["data"]["state"], "locked");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
    }

    #[test]
    fn status_id_is_stable_for_title_but_scoped_to_target_boot_and_runtime() {
        let _serial = plx_base::testlock::serial();
        let mut app = fixture();
        let mut favorite = local_item();
        install_pin(&mut app, favorite.clone());
        let first = channel_id(&status(&mut app, 1)).to_owned();
        assert_eq!(channel_id(&status(&mut app, 2)), first);
        favorite.title = "Renamed public title".into();
        install_pin(&mut app, favorite.clone());
        assert_eq!(channel_id(&status(&mut app, 3)), first);
        app.ott.as_mut().unwrap().entropy = Some([2; 16]);
        assert_ne!(channel_id(&status(&mut app, 4)), first,
            "a different boot must not retain the same private-identity digest");
        app.ott.as_mut().unwrap().entropy = Some([1; 16]);
        app.ott.as_mut().unwrap().remote = Remote::disabled_for_test("dev_fixture", "other-runtime");
        assert_ne!(channel_id(&status(&mut app, 5)), first);
        app.ott.as_mut().unwrap().remote = Remote::disabled_for_test("dev_fixture", "fixture");
        if let Locator::Local { relative_path, .. } = &mut favorite.locator {
            *relative_path = "Private folder fixture/other.mp4".into();
        }
        install_pin(&mut app, favorite);
        assert_ne!(channel_id(&status(&mut app, 6)), first);
    }

    #[test]
    fn item_status_without_boot_entropy_has_no_raw_identity_fallback() {
        let _serial = plx_base::testlock::serial();
        let mut app = fixture();
        install_pin(&mut app, local_item());
        app.ott.as_mut().unwrap().entropy = None;
        let response = status(&mut app, 1);
        assert_eq!(response["status"], "rejected");
        assert!(response["data"].get("channel").is_none());
        assert!(!response.to_string().contains("Private"));
    }

    #[test]
    fn armed_status_remains_available_without_an_item_id_or_entropy() {
        let _serial = plx_base::testlock::serial();
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.settings.insert("nativeKiosk".into(), armed(true));
        runtime.entropy = None;
        let response = status(&mut app, 1);
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["state"], "waiting");
        assert!(response["data"]["channel"].is_null());
    }
}

fn armed(strict: bool) -> Value {
    json!({"version":1,"strict":strict,
        "origin":{"provider":"m3u","profile":"Fixture","subprofile":null,"sourceAlias":"Fixture"},
        "target":{"kind":"armed"}})
}
fn fixture() -> App {
    let mut app = app_fixture();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    app
}
fn request(app: &mut App, params: Value) -> (Invocation, Option<Value>) {
    request_with_lifetime(app, params, 10_000)
}
fn request_with_lifetime(
    app: &mut App,
    params: Value,
    lifetime: u64,
) -> (Invocation, Option<Value>) {
    let mut runtime = app.ott.take().unwrap();
    // Every case uses each parameter object once. Unique content-addressed IDs
    // cross the real ingress without colliding with another pending request.
    let digest = plx_base::sha256::sha256(&serde_json::to_vec(&params).unwrap());
    let id: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":id,"action":"kiosk","params":params,"expires_at":1020
    }]})
    .to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, 100);
    assert_eq!(
        invocations.len(),
        1,
        "unique request admitted by actual ingress"
    );
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(lifetime);
    let response = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    (invocation, response)
}
fn read_status(app: &mut App) -> Value {
    request(app, json!({"mode":"status"})).1.unwrap()
}
fn take_writer(app: &mut App) -> Job {
    let runtime = app.ott.as_ref().unwrap();
    let job = runtime
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("revoked writer result");
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    job
}
fn deliver(app: &mut App, job: Job) -> Value {
    app.ott.as_ref().unwrap().tx.send(job).unwrap();
    runtime_tick(app);
    let remote = &app.ott.as_ref().unwrap().remote;
    let response = remote
        .response_for_test()
        .expect("queued actual wire response");
    assert_eq!(
        remote.response_for_test(),
        Some(response.clone()),
        "read-only observer"
    );
    response
}

#[test]
fn kiosk_status_off_and_malformed_mutation_use_real_dispatcher() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let response = read_status(&mut app);
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["enabled"], false);
    assert_eq!(response["data"]["state"], "off");
    assert_eq!(response["data"]["retry_seconds"], 10);
    assert_eq!(response["data"]["retries"], 0);
    assert_eq!(response["data"]["health"], "idle");
    let (_, response) = request(&mut app, json!({"mode":"off","strict":false}));
    assert_eq!(response.unwrap()["status"], "rejected");
    assert!(!app.ott.as_ref().unwrap().saving);
    let (_, response) = request(&mut app, json!({"mode":"set","query":"1"}));
    assert_eq!(
        response.unwrap()["data"]["error"],
        "Enable kiosk mode first."
    );
}

#[test]
fn kiosk_off_keeps_old_policy_until_real_writer_failure_settles() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), armed(false));
    let before = app.ott.as_ref().unwrap().saved.settings.clone();
    assert!(request(&mut app, json!({"mode":"off"})).1.is_none());
    assert!(app.ott.as_ref().unwrap().saving);
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, before);
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_test()
        .is_none());
    let job = take_writer(&mut app);
    let response = deliver(&mut app, job);
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["error"], "Could not save kiosk policy.");
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, before);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn kiosk_second_mutation_is_busy_and_status_reads_old_policy() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), armed(false));
    assert!(request(&mut app, json!({"mode":"on","strict":true}))
        .1
        .is_none());
    let job = take_writer(&mut app);
    let (_, response) = request(&mut app, json!({"mode":"off"}));
    assert_eq!(response.unwrap()["status"], "rejected");
    let response = read_status(&mut app);
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "waiting");
    assert_eq!(response["data"]["strict"], false);
    assert!(app.ott.as_ref().unwrap().saving);
    let _ = deliver(&mut app, job);
}

#[test]
fn kiosk_strict_only_modeled_save_returns_post_publication_status_without_play() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), armed(false));
    let before = app.ott.as_ref().unwrap().play_generation;
    assert!(request(&mut app, json!({"mode":"on","strict":true}))
        .1
        .is_none());
    let Job::Saved(_, invocation, reload, placeholder) = take_writer(&mut app) else {
        unreachable!()
    };
    let mut candidate = app.ott.as_ref().unwrap().saved.clone();
    candidate.settings.insert("nativeKiosk".into(), armed(true));
    let response = deliver(
        &mut app,
        Job::Saved(Ok(candidate), invocation, reload, placeholder),
    );
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["strict"], true);
    assert_eq!(response["data"]["state"], "waiting");
    assert_eq!(app.ott.as_ref().unwrap().play_generation, before);
    assert_eq!(
        app.ott.as_ref().unwrap().saved.settings["nativeKiosk"]["strict"],
        true
    );
}

#[test]
fn kiosk_unknown_keeps_policy_unavailable_instead_of_reporting_off() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), armed(true));
    assert!(request(&mut app, json!({"mode":"off"})).1.is_none());
    let Job::Saved(_, invocation, _, _) = take_writer(&mut app) else {
        unreachable!()
    };
    let mut candidate = app.ott.as_ref().unwrap().saved.clone();
    candidate.settings.remove("nativeKiosk");
    let response = deliver(&mut app, Job::Uncertain(Some(candidate), invocation));
    assert_eq!(response["status"], "rejected");
    assert!(response["data"]["error"]
        .as_str()
        .unwrap()
        .contains("could not be verified"));
    assert!(!app.ott.as_ref().unwrap().loaded);
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert_eq!(read_status(&mut app)["status"], "rejected");
}

#[test]
fn kiosk_retired_activation_does_not_discard_pending_durable_off() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), armed(true));
    assert!(request(&mut app, json!({"mode":"off"})).1.is_none());
    let Job::Saved(_, invocation, reload, placeholder) = take_writer(&mut app) else {
        unreachable!()
    };
    let mut runtime = app.ott.take().unwrap();
    let before = runtime.playback_epoch;
    // Use the actual playback-retirement boundary, which fences activation
    // without discarding a pending durable kiosk write.
    runtime.retire_catalog_playback(&mut app);
    assert_ne!(runtime.playback_epoch, before);
    app.ott = Some(runtime);
    assert!(commands::kiosk_pending(app.ott.as_ref().unwrap()));
    let mut candidate = app.ott.as_ref().unwrap().saved.clone();
    candidate.settings.remove("nativeKiosk");
    let response = deliver(
        &mut app,
        Job::Saved(Ok(candidate), invocation, reload, placeholder),
    );
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["enabled"], false);
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .saved
        .settings
        .contains_key("nativeKiosk"));
    assert!(!commands::kiosk_pending(app.ott.as_ref().unwrap()));
}

struct ResetKioskMedia;
impl Drop for ResetKioskMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn installed_a() -> App {
    let mut app = fixture();
    let profile = synthetic_profile();
    let entries: Vec<_> = (1..=2)
        .map(|number| {
            let mut entry = synthetic_entry();
            entry.id = format!("video-{number}");
            entry.title = format!("Video {number}");
            entry.provider_id = Some(number.to_string());
            entry.media = MediaLocator::VPortal {
                request: json!({"id":number}),
            };
            entry
        })
        .collect();
    let source = plx_media::ff::ExternalSource::Http("https://media.invalid/a".into());
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        "Video 1",
        "Fixture",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let source = plx_media::route::external_source(&app.player.session)
        .expect("canonical source actually installed by the native route");
    let start = plx_media::route::pending_route_start().expect("prepared native route");
    let attempt = plx_media::route::claim_route_start_attempt(start)
        .expect("addressed native start attempt");
    // Model the native completion through its actual transaction owner, without
    // starting a decoder or claiming that a physical frame was presented.
    assert!(plx_media::route::settle_route_start(
        &mut app.player.session, attempt, plx_media::route::RouteStartResult::Started,
    ));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    assert_eq!(plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Playing);
    // Real route admission only: no start_playback, decoder or network worker.
    let mut runtime = app.ott.take().unwrap();
    runtime.provider = ProviderKind::VPortal;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.catalog.entries = entries.clone();
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.selected = Some(entries[0].id.clone());
    runtime.resolver_busy = true; // retain B in the actual resolver queue, no IO.
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: None,
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry: entries[0].clone(),
        selection_id: entries[0].id.clone(),
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
    });
    let prepared = kiosk_source::prepare(&runtime, &app, Some("1"), false).unwrap();
    runtime.saved = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    assert!(runtime.current_kiosk_pin_owned(&app),
        "fixture's saved pin owns the exact installed native source");
    app.ott = Some(runtime);
    app
}
fn direct_set_candidate(app: &mut App, lifetime: u64) -> (Job, Saved, u64) {
    let runtime = app.ott.take().unwrap();
    let prepared = kiosk_source::prepare(&runtime, app, Some("2"), false).unwrap();
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    app.ott = Some(runtime);
    let (invocation, response) =
        request_with_lifetime(app, json!({"mode":"set","query":"2"}), lifetime);
    assert!(response.is_none(), "direct designation enters sole writer");
    (take_writer(app), candidate, invocation.deadline_ms)
}

#[test]
fn kiosk_direct_set_a_to_b_survives_its_own_enforcement_epoch_change() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let (job, candidate, _) = direct_set_candidate(&mut app, 10_000);
    let Job::Saved(_, invocation, reload, placeholder) = job else {
        unreachable!()
    };
    let response = deliver(
        &mut app,
        Job::Saved(Ok(candidate), invocation, reload, placeholder),
    );
    assert_eq!(response["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime.playback_epoch > epoch,
        "enforcement retired the old owner"
    );
    assert!(runtime.playing.is_none(), "A was retired");
    assert_eq!(
        runtime
            .play_target
            .as_ref()
            .map(|request| request.id.as_str()),
        Some("video-2")
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .and_then(|request| request.entry.as_ref())
            .map(|entry| entry.id.as_str()),
        Some("video-2")
    );
    assert_eq!(response["data"]["state"], "locked");
    let admitted_generation = runtime.play_generation;
    assert!(
        !runtime.saving,
        "enforcement must not launch a second writer"
    );
    assert!(
        app.player.pending_exit.is_none(),
        "teardown must not retire B next tick"
    );
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, admitted_generation);
    assert_eq!(
        runtime
            .play_target
            .as_ref()
            .map(|request| request.id.as_str()),
        Some("video-2")
    );
}

#[test]
fn kiosk_expired_set_still_publishes_b_but_does_not_activate_it() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let (job, candidate, deadline) = direct_set_candidate(&mut app, 20);
    let expected = candidate.settings["nativeKiosk"].clone();
    let Job::Saved(_, invocation, reload, placeholder) = job else {
        unreachable!()
    };
    let now = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .unwrap()
        .unix_ms();
    // Bound the synthetic late completion; no request worker or device runs.
    std::thread::sleep(Duration::from_millis(
        deadline.saturating_sub(now).saturating_add(2),
    ));
    let response = deliver(
        &mut app,
        Job::Saved(Ok(candidate), invocation, reload, placeholder),
    );
    assert_eq!(response["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.settings["nativeKiosk"], expected);
    assert!(
        runtime.playing.is_none(),
        "old A cannot bypass the new durable policy"
    );
    assert!(
        runtime.play_target.is_none(),
        "expired request cannot activate B"
    );
    assert!(runtime.resolver_next.is_none());
    assert_eq!(response["data"]["state"], "locked");
}

#[test]
fn kiosk_direct_set_failure_preserves_the_old_policy_and_native_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let before = app.ott.as_ref().unwrap().saved.settings["nativeKiosk"].clone();
    let identity = plx_media::route::playback_identity(&app.player.session);
    let (job, _, _) = direct_set_candidate(&mut app, 10_000);
    let response = deliver(&mut app, job);
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["error"], "Could not save kiosk policy.");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.settings["nativeKiosk"], before);
    assert_eq!(
        runtime
            .playing
            .as_ref()
            .map(|playing| playing.selection_id.as_str()),
        Some("video-1")
    );
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn strict_only_saved_keeps_the_current_resolving_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let (job, after, _) = direct_set_candidate(&mut app, 10_000);
    let Job::Saved(_, inv, reload, response) = job else {
        unreachable!()
    };
    deliver(&mut app, Job::Saved(Ok(after), inv, reload, response));
    let before = app.ott.as_ref().unwrap().play_generation;
    let target = app.ott.as_ref().unwrap().play_target.clone();
    assert!(target.is_some());
    assert!(request(&mut app, json!({"mode":"on","strict":true}))
        .1
        .is_none());
    let Job::Saved(_, inv, reload, response) = take_writer(&mut app) else {
        unreachable!()
    };
    let mut after = app.ott.as_ref().unwrap().saved.clone();
    after.settings.get_mut("nativeKiosk").unwrap()["strict"] = json!(true);
    let response = deliver(&mut app, Job::Saved(Ok(after), inv, reload, response));
    assert_eq!(response["status"], "ok");
    assert_eq!(app.ott.as_ref().unwrap().play_generation, before);
    assert_eq!(app.ott.as_ref().unwrap().play_target, target);
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, target);
}
#[test]
fn strict_only_saved_keeps_a_retained_resume_without_waking_it() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let (job, after, _) = direct_set_candidate(&mut app, 10_000);
    let Job::Saved(_, inv, reload, response) = job else {
        unreachable!()
    };
    deliver(&mut app, Job::Saved(Ok(after), inv, reload, response));
    assert!(request(&mut app, json!({"mode":"on","strict":true}))
        .1
        .is_none());
    let Job::Saved(_, inv, reload, response) = take_writer(&mut app) else {
        unreachable!()
    };
    app.ott_standby = true;
    let mut runtime = app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut app);
    let retained = runtime.resume_pending.clone();
    assert!(retained.is_some());
    let generation = runtime.play_generation;
    let mut after = runtime.saved.clone();
    after.settings.get_mut("nativeKiosk").unwrap()["strict"] = json!(true);
    app.ott = Some(runtime);
    deliver(&mut app, Job::Saved(Ok(after), inv, reload, response));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.resume_pending, retained);
    assert!(runtime.play_target.is_none());
}

#[test]
fn protected_new_designation_is_denied_before_saved_mutation() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetKioskMedia;
    let mut app = installed_a();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    runtime.catalog.entries[1].protection = Some(true);
    let before = runtime.saved.settings.clone();
    let (_, response) = request(&mut app, json!({"mode":"set","query":"2"}));
    assert_eq!(response.unwrap()["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.settings, before);
    assert!(!runtime.saving);
    assert!(!commands::kiosk_pending(runtime));
    assert!(runtime.rx.try_recv().is_err());
}

//! Ordinary accepted Plex Repeat proposal. Synthetic existing queue/identity
//! fixture only; no PMS, decoder, writer or process is launched. Unrun.
use super::*;

fn fixture() -> (App, plx_plex::plex::ServerId) {
    let mut app = setup();
    app.running = true;
    app.ott.as_mut().unwrap().resume_source_work();
    let sid = register_source(&mut app, "ordinary-repeat-machine");
    seed_queue(&mut app, sid, &["41", "42", "41"]);
    (app, sid)
}

fn work(app: &App) -> Value {
    let runtime = app.ott.as_ref().unwrap();
    json!({
        "generation":runtime.generation, "play_generation":runtime.play_generation,
        "playback_epoch":runtime.playback_epoch, "navigation_epoch":runtime.navigation_epoch,
        "selected":runtime.selected, "saving":runtime.saving,
        "pending_play":runtime.pending_play.is_some(), "play_target":runtime.play_target.is_some(),
        "resolver_next":runtime.resolver_next.is_some(), "resolver_busy":runtime.resolver_busy,
        "candidate":runtime.transient.candidate.is_some(), "active_lease":runtime.active_lease.is_some(),
    })
}

fn params(mode: &str, version: u8) -> Value {
    let mut value = json!({"op":"repeat","repeat":mode});
    if version == 2 { value["state_version"] = json!(2); }
    value
}

#[test]
fn ordinary_repeat_command_changes_only_repeat_and_replies_in_negotiated_version() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for version in [1, 2] {
        for paused in [false, true] {
            let (mut app, _) = fixture();
            if paused {
                plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Paused);
                let Some(transient::TransientMedia::Plex(session)) = app.ott.as_mut().unwrap().transient.accepted.as_mut() else { panic!("Plex fixture"); };
                assert!(session.queue.observe(session.queue.owner(), session.queue.playback().unwrap(), queue::PlaybackPhase::Paused));
            }
            let saved = saved_value(&app);
            let pending = work(&app);
            let identity = plx_media::route::playback_identity(&app.player.session);
            let ticket = plx_media::route::play_request_ticket();
            let state = plx_media::player::state(&app.player.session);
            let before = queue_value(&app);
            for mode in ["all", "all", "none"] {
                let response = command(&mut app, "plex_queue", params(mode, version)).expect("synchronous state reply");
                assert_eq!(response["status"], "ok", "{response}");
                let mut expected = before.clone();
                expected["repeat"] = json!(mode);
                assert_eq!(queue_value(&app), expected);
                expected["version"] = json!(version);
                assert_eq!(response["data"], expected, "existing complete queue snapshot");
                assert_eq!(saved_value(&app), saved);
                assert_eq!(work(&app), pending);
                assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
                assert_eq!(plx_media::route::play_request_ticket(), ticket);
                assert_eq!(plx_media::player::state(&app.player.session), state);
            }
        }
    }
}

#[test]
fn ordinary_repeat_command_controls_last_eos_without_preemptively_advancing() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for mode in ["all", "none"] {
        let (mut app, _) = fixture();
        let response = command(&mut app, "plex_queue", params(mode, 1)).unwrap();
        assert_eq!(response["status"], "ok", "{response}");
        assert_eq!(queue_value(&app)["index"], 0);
        let Some(transient::TransientMedia::Plex(session)) = app.ott.as_mut().unwrap().transient.accepted.as_mut() else { panic!("Plex fixture"); };
        let owner = session.queue.owner();
        let token = queue::PlaybackToken(71);
        session.queue.begin(owner, token, 2).unwrap();
        assert!(session.queue.observe(owner, token, queue::PlaybackPhase::Playing));
        let expected = if mode == "all" { queue::PlexDecision::Start { index: 0 } } else { queue::PlexDecision::Ended };
        assert_eq!(session.queue.ended(owner, token), expected);
        assert_eq!(session.queue.ended(owner, token), queue::PlexDecision::Ignore);
    }
}

#[test]
fn ordinary_repeat_refuses_absent_stopped_busy_foreign_and_kiosk_queues_without_effects() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for case in ["absent", "stopped", "busy", "parked", "foreign-native", "changed-profile", "replaced-token", "stale-invocation", "kiosk", "runtime", "expired"] {
        let (mut app, sid) = fixture();
        let mut invocation = request(&app, "plex_queue", params("all", 1));
        match case {
            "absent" => app.ott.as_mut().unwrap().transient.accepted = None,
            "stopped" => {
                let Some(transient::TransientMedia::Plex(session)) = app.ott.as_mut().unwrap().transient.accepted.as_mut() else { panic!("Plex fixture"); };
                session.queue.stop(session.queue.owner()).unwrap();
            }
            "busy" => { app.ott.as_mut().unwrap().play_target = Some(PlaybackRequest::ordinary("pending", 0)); }
            "parked" => {
                let Some(transient::TransientMedia::Plex(session)) = app.ott.as_mut().unwrap().transient.accepted.as_mut() else { panic!("Plex fixture"); };
                session.queue.park(session.queue.owner()).unwrap();
            }
            "foreign-native" => install_native_identity(&mut app, sid, "different-item"),
            "changed-profile" => {
                app.ott.as_mut().unwrap().saved.profiles.profiles.retain(|p| p.kind != ProviderKind::Plex);
            }
            "replaced-token" => { plx_plex::plex::client_for(sid).unwrap().set_token("replacement-token"); }
            "stale-invocation" => {
                let runtime = app.ott.as_mut().unwrap();
                let device = runtime.remote.device_id().to_owned();
                // In-memory invalidation only; no tick/persist/transport runner.
                runtime.remote.configure(json!({"version":1,"enabled":false,"device_id":device})).unwrap();
            }
            "kiosk" => {
                let runtime = app.ott.as_ref().unwrap();
                let pin = transient::kiosk_queue(runtime, &app).unwrap().unwrap();
                let stored = crate::app::ott_kiosk_stored::Stored::queue(pin.entries, false).unwrap().encode().unwrap();
                app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), stored);
            }
            _ => {},
        }
        let saved = saved_value(&app);
        let pending = work(&app);
        let before = (case != "absent").then(|| queue_value(&app));
        let identity = plx_media::route::playback_identity(&app.player.session);
        if case == "runtime" { invocation.params["runtime"] = json!("retired-runtime"); }
        if case == "expired" { invocation.deadline_ms = app.ott.as_ref().unwrap().now_ms(); }
        let mut runtime = app.ott.take().unwrap();
        let response = commands::command(&mut runtime, &mut app, &invocation).expect("terminal refusal");
        app.ott = Some(runtime);
        match case {
            "kiosk" | "runtime" | "expired" => assert_eq!(response["status"], "rejected", "{case}: {response}"),
            _ => {
                let error = match case {
                    "absent" => queue::QueueError::Empty,
                    "stopped" => queue::QueueError::Unavailable,
                    "busy" => queue::QueueError::Busy,
                    _ => queue::QueueError::ContextChanged,
                };
                assert_eq!(response["status"], "ok", "{case}: {response}");
                assert_eq!(response["data"], queue::empty_error_snapshot(app.ott.as_ref().unwrap().remote.runtime(), error).unwrap(), "{case}");
            }
        }
        assert_eq!(saved_value(&app), saved, "{case}");
        assert_eq!(work(&app), pending, "{case}");
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity, "{case}");
        if let Some(before) = before { assert_eq!(queue_value(&app), before, "{case}"); }
    }
}

#[test]
fn ordinary_repeat_preflights_large_queue_reply_before_mutating() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    let ids = vec!["41"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    let snapshot = |app: &App| {
        let runtime = app.ott.as_ref().unwrap();
        let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref() else { panic!("Plex fixture"); };
        session.queue.snapshot_version(runtime.remote.runtime(), queue::StateVersion::V2).unwrap()
    };
    let before = snapshot(&app);
    let saved = saved_value(&app);
    let pending = work(&app);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let rejected = command(&mut app, "plex_queue", params("all", 1)).unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert!(rejected["data"]["error"].as_str().unwrap().contains("state_version: 2"));
    assert_eq!(snapshot(&app), before, "v1 must not mutate before discovering the response does not fit");
    let accepted = command(&mut app, "plex_queue", params("all", 2)).unwrap();
    let mut expected = before;
    expected["repeat"] = json!("all");
    assert_eq!(accepted["status"], "ok");
    assert_eq!(accepted["data"], expected);
    assert_eq!(snapshot(&app), expected);
    assert_eq!(saved_value(&app), saved);
    assert_eq!(work(&app), pending);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
}

#[test]
fn ordinary_repeat_capability_is_separate_and_native_probe_stays_empty() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, _) = fixture();
    for probe in [json!({}), json!({"plex_queue_repeat":true})] {
        let runtime = app.ott.as_ref().unwrap();
        let mut remote = Remote::disabled_for_test("dev_fixture", runtime.remote.runtime());
        let mut invocation = remote.request_for_test("capabilities", probe.clone());
        invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
        let mut runtime = app.ott.take().unwrap();
        let reply = commands::command(&mut runtime, &mut app, &invocation).unwrap();
        app.ott = Some(runtime);
        if probe.as_object().unwrap().is_empty() {
            assert_eq!(reply["status"], "ok");
            assert_eq!(reply["data"]["plex_queue_repeat"], json!({"version":1,"modes":["none","all"]}));
            assert_eq!(reply["data"]["plex_queue"], queue::queue_capability());
        } else {
            assert_eq!(reply["status"], "rejected", "relay-only flag must be removed by Controller");
        }
    }
}

#[test]
fn kiosk_off_restores_the_requested_repeat_without_changing_the_accepted_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for requested in [queue::RepeatPolicy::None, queue::RepeatPolicy::All] {
        let (mut app, _) = fixture();
        let mut runtime = app.ott.take().unwrap();
        let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut() else { panic!("Plex fixture"); };
        session.queue.set_repeat(session.queue.owner(), requested).unwrap();
        let pin = transient::kiosk_queue(&runtime, &app).unwrap().unwrap();
        // Model already-verified policy publication at the existing effect seam.
        // This test deliberately does not start a writer or claim durable Save.
        let stored = crate::app::ott_kiosk_stored::Stored::queue(pin.entries.clone(), false).unwrap().encode().unwrap();
        runtime.saved.settings.insert("nativeKiosk".into(), stored);
        transient::kiosk_repeat_all(&mut runtime, &app, &pin).unwrap();
        app.ott = Some(runtime);
        assert_eq!(queue_value(&app)["repeat"], "all");
        let before = queue_value(&app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        let pending = work(&app);
        let mut runtime = app.ott.take().unwrap();
        runtime.saved.settings.remove("nativeKiosk");
        let expected_saved = serde_json::to_value(&runtime.saved).unwrap();
        kiosk_source::apply_durable_policy(&mut runtime, &mut app);
        app.ott = Some(runtime);
        let mut expected = before;
        expected["repeat"] = json!(if requested == queue::RepeatPolicy::All { "all" } else { "none" });
        assert_eq!(queue_value(&app), expected);
        assert_eq!(saved_value(&app), expected_saved);
        assert_eq!(work(&app), pending);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    }
}

#[test]
fn ordinary_repeat_cannot_modify_a_superseded_or_borrowed_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for case in ["same-item-new-identity", "foreign-request", "borrowed", "changed-origin", "duplicate-source"] {
        let (mut app, sid) = fixture();
        match case {
            "same-item-new-identity" => install_native_identity(&mut app, sid, "41"),
            "foreign-request" => plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "99"),
            "borrowed" => {
                let runtime = app.ott.as_mut().unwrap();
                let mut playing = runtime.playing.clone().unwrap();
                playing.selection_id = "favorite:outside-queue".into();
                assert!(transient::plex_remote::checkpoint(runtime, &playing, 12_000));
                runtime.playing = Some(playing);
                let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref() else { panic!("Plex fixture"); };
                assert!(session.borrowed.is_some());
            }
            "changed-origin" => {
                app.ott.as_mut().unwrap().saved.settings.get_mut("nativePlexSources").unwrap()[0]["origin"] = json!("https://other-source.invalid");
            }
            "duplicate-source" => {
                let sources = app.ott.as_mut().unwrap().saved.settings.get_mut("nativePlexSources").unwrap().as_array_mut().unwrap();
                sources.push(sources[0].clone());
            }
            _ => unreachable!(),
        }
        let before = queue_value(&app);
        let saved = saved_value(&app);
        let pending = work(&app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        let response = command(&mut app, "plex_queue", params("all", 1)).unwrap();
        assert_eq!(response["status"], "ok", "{case}: {response}");
        assert_eq!(response["data"]["error"], queue::QueueError::ContextChanged.message(), "{case}");
        assert_eq!(queue_value(&app), before, "{case}");
        assert_eq!(saved_value(&app), saved, "{case}");
        assert_eq!(work(&app), pending, "{case}");
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity, "{case}");
    }
}

#[test]
fn durable_queue_policy_forces_repeat_without_deferred_activation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, _) = fixture();
    let pending = work(&app);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let mut runtime = app.ott.take().unwrap();
    let pin = transient::kiosk_queue(&runtime, &app).unwrap().unwrap();
    let stored = crate::app::ott_kiosk_stored::Stored::queue(pin.entries, false).unwrap().encode().unwrap();
    // Exact durable publication seam, including the path where an expired RPC
    // skips later activation. No command is retried and no writer is started.
    runtime.saved.settings.insert("nativeKiosk".into(), stored);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    kiosk_source::apply_durable_policy(&mut runtime, &mut app);
    app.ott = Some(runtime);
    assert_eq!(queue_value(&app)["repeat"], "all");
    assert_eq!(saved_value(&app), saved);
    assert_eq!(work(&app), pending);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);

    let mut runtime = app.ott.take().unwrap();
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut() else { panic!("Plex fixture"); };
    session.queue.park(session.queue.owner()).unwrap();
    runtime.saved.settings.remove("nativeKiosk");
    kiosk_source::apply_durable_policy(&mut runtime, &mut app);
    app.ott = Some(runtime);
    assert_eq!(queue_value(&app)["repeat"], "none", "Off also clears force on a parked queue");
    assert_eq!(queue_value(&app)["state"], "paused");
}

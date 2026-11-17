//! Real temporary publication and ordinary command/admission boundaries; no media IO.
use super::*;

fn install_temporary(app: &mut App, sid: plx_plex::plex::ServerId, ids: &[&str]) {
    let (owner, id) = begin_plex(app, "plex_queue", json!({"op":"play","ids":ids}));
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(&mut runtime, app, owner,
        transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(owner, sid, prepare::Prepared::Queue {
            kind: queue::PrepareKind::Play, selection: selection(ids),
        })));
    assert!(runtime.play_target.is_some());
    let reply = transient::finish_native(&mut runtime, app, Ok(()), None).expect("real staged admission");
    assert_eq!(reply["status"], "ok");
    // The ordinary Job::Play dispatcher settles these after the same finish hook.
    runtime.pending_play = None;
    runtime.play_target = None;
    runtime.play_profile = None;
    runtime.resolver_next = None;
    app.ott = Some(runtime);
}
fn number_command(app: &mut App, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let mut remote = Remote::disabled_for_test("dev_fixture", runtime.remote.runtime());
    // Generic play has only query; runtime is a Plex-specific protocol field.
    let mut invocation = remote.request_for_test("play", params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
    let reply = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    reply
}
fn fixture() -> (App, plx_plex::plex::ServerId) {
    let mut app = setup();
    app.running = true;
    assert!(lifecycle::set_transport_paused(&mut app.adapters.player, false));
    let sid = register_source(&mut app, "numeric-fixture");
    (app, sid)
}

#[test]
fn remote_number_after_actual_plex_admission_reopens_configured_channel_without_saving_provider() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    let saved = saved_value(&app);
    install_temporary(&mut app, sid, &["42", "7"]);
    let queue = app.ott.as_ref().unwrap().catalog.entries[0].id.clone();
    assert_ne!(queue, "live");
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
    let mut runtime = app.ott.take().unwrap();
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "live");
    assert_eq!(runtime.catalog.entries[0].id, queue, "pending resolution cannot replace the queue");
    assert!(matches!(runtime.transient.accepted, Some(transient::TransientMedia::Plex(_))));
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = numeric_remote::staged_binding(&runtime, &request).unwrap().unwrap();
    assert_eq!(profile.id, "permanent");
    assert_eq!(entry.id, "live");
    runtime.rebind_native_admission(&app, &profile, &entry).expect("full fresh admission policy");
    let ticket = plx_media::route::play_request_ticket();
    let native = NativeStartReceipt { generation: runtime.play_generation, before: ticket, after: ticket };
    let reply = numeric_remote::finish(&mut runtime, &app, Ok(()), Some(&native)).unwrap();
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["data"]["channel"]["id"], "live");
    assert!(runtime.transient.accepted.is_none());
    assert_eq!(runtime.catalog.entries[0].id, "live");
    assert_eq!(runtime.selected.as_deref(), Some("live"));
    app.ott = Some(runtime);
    assert_eq!(saved_value(&app), saved);
}

#[test]
fn invalid_or_protected_numeric_targets_never_retire_the_accepted_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for case in ["zero", "range", "overflow", "protected", "standby", "missing", "incomplete", "budget", "capacity", "sparse-object", "vector-capacity"] {
        let (mut app, sid) = fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            match case {
                "protected" => runtime.catalog.entries[0].protection = Some(true),
                "incomplete" => runtime.catalog_complete = false,
                "budget" => runtime.catalog.entries[0].description = "x".repeat(8 * 1024 * 1024),
                "capacity" => runtime.catalog.entries[0].description.reserve(9 * 1024 * 1024),
                "sparse-object" => runtime.catalog.entries[0].media = MediaLocator::VPortal {
                    request: Value::Array(vec![json!({"key":null}); 8_000]),
                },
                "vector-capacity" => {
                    let mut values = Vec::with_capacity(400_000);
                    values.push(Value::Null);
                    runtime.catalog.entries[0].media = MediaLocator::VPortal { request: Value::Array(values) };
                },
                _ => (),
            }
        }
        install_temporary(&mut app, sid, &["42", "7"]);
        if case == "missing" { numeric_remote::clear(app.ott.as_mut().unwrap()); }
        if case == "standby" { app.running = false; }
        let previous = app.ott.as_ref().unwrap().catalog.entries[0].id.clone();
        let owner = app.ott.as_ref().unwrap().transient.accepted.as_ref().unwrap().owner();
        let before = saved_value(&app);
        let query = match case { "zero" => "0", "range" => "2", "overflow" => "999999999999999999999999999999", _ => "1" };
        let reply = number_command(&mut app, json!({"query":query}));
        assert!(reply.is_some(), "{case}: rejection must be synchronous");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog.entries[0].id, previous, "{case}");
        assert!(runtime.transient.accepted.as_ref().unwrap().owner() == owner, "{case}");
        assert!(!numeric_remote::has_pending(runtime), "{case}");
        assert_eq!(saved_value(&app), before, "{case}");
    }
}

#[test]
fn failed_or_stale_native_numeric_ack_keeps_the_temporary_catalogue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for case in ["failure", "missing-ack", "generation", "input", "settings", "deadline", "owner", "native-ticket"] {
        let (mut app, sid) = fixture();
        install_temporary(&mut app, sid, &["42", "7"]);
        let queue = app.ott.as_ref().unwrap().catalog.entries[0].id.clone();
        let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
        let mut runtime = app.ott.take().unwrap();
        let ticket = plx_media::route::play_request_ticket();
        let native = NativeStartReceipt { generation: runtime.play_generation, before: ticket, after: ticket };
        match case {
            "generation" => runtime.generation = runtime.generation.wrapping_add(1),
            "input" => app.last_input = app.last_input.wrapping_add(1),
            "settings" => { runtime.saved.settings.insert("bufferSize".into(), json!(7)); },
            "deadline" => runtime.pending_play.as_mut().unwrap().deadline_ms = 0,
            "owner" => { runtime.transient.accepted = None; },
            "native-ticket" => plx_media::route::cancel_play(&mut app.player.session),
            _ => (),
        }
        let result = if case == "failure" { Err("synthetic resolver failure") } else { Ok(()) };
        let response = numeric_remote::finish(&mut runtime, &app, result,
            if case == "missing-ack" { None } else { Some(&native) }).unwrap();
        assert_ne!(response["status"], "ok", "{case}");
        assert_eq!(runtime.catalog.entries[0].id, queue, "{case}: stale ACK must not publish base rows");
    }
}

#[test]
fn replacement_temporary_queue_keeps_the_original_base_and_ui_numbers_stay_queue_relative() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    install_temporary(&mut app, sid, &["42", "7"]);
    install_temporary(&mut app, sid, &["99", "100"]);
    let runtime = app.ott.as_ref().unwrap();
    assert_ne!(commands::numbered_channels(runtime).resolve("1").unwrap().0, "live",
        "physical/UI numeric projection remains the displayed temporary list");
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "live", "second temporary queue cannot become the base");
}

#[test]
fn changed_base_and_superseded_numeric_request_cannot_rebind_or_publish_rows() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for change in ["source", "active-profile", "category", "newer-intent"] {
        let (mut app, sid) = fixture();
        install_temporary(&mut app, sid, &["42", "7"]);
        let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
        let mut runtime = app.ott.take().unwrap();
        let request = runtime.play_target.as_ref().unwrap().clone();
        match change {
            "source" => runtime.saved.profiles.profiles[0].source = "https://changed.invalid/list".into(),
            "active-profile" => runtime.saved.profiles.active_profile = None,
            "category" => runtime.saved.category.clear(),
            "newer-intent" => runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1),
            _ => unreachable!(),
        }
        assert!(numeric_remote::staged_binding(&runtime, &request).unwrap().is_err(), "{change}");
    }
}

#[test]
fn numeric_base_keeps_its_selected_category_and_canonical_numbers() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    {
        let runtime = app.ott.as_mut().unwrap();
        let mut second = live_entry();
        second.id = "second-news".into();
        runtime.catalog.entries[0].category = "Other".into();
        runtime.catalog.entries.push(second);
    }
    install_temporary(&mut app, sid, &["42", "7"]);
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "All canonical channels remain addressable: {reply:?}");
    let reply = number_command(&mut app, json!({"query":"2"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id, "second-news");
    assert_eq!(app.ott.as_ref().unwrap().saved.category, "News");
}

#[test]
fn late_numeric_native_ack_cannot_replace_a_newer_accepted_plex_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    install_temporary(&mut app, sid, &["42", "7"]);
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric play rejected: {reply:?}");
    let runtime = app.ott.as_ref().unwrap();
    let ticket = plx_media::route::play_request_ticket();
    let old = NativeStartReceipt { generation: runtime.play_generation, before: ticket, after: ticket };
    // Explicit cancellation retires the pending intent before another temporary
    // command is admitted; accepted queue/catalogue ownership is still retained.
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.cancel_play_request();
        runtime.pending_play = None;
    }
    install_temporary(&mut app, sid, &["99", "100"]);
    let mut runtime = app.ott.take().unwrap();
    let newest = runtime.catalog.entries[0].id.clone();
    assert!(numeric_remote::finish(&mut runtime, &app, Ok(()), Some(&old)).is_none());
    assert_eq!(runtime.catalog.entries[0].id, newest);
    assert!(runtime.transient.accepted.is_some());
}

#[test]
fn trimmed_remote_number_cannot_fall_through_to_queue_after_favorites_change() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    install_temporary(&mut app, sid, &["42", "7"]);
    let reply = number_command(&mut app, json!({"query":" 1 "}));
    assert!(reply.is_none(), "trimmed numeric command rejected: {reply:?}");
    let runtime = app.ott.as_mut().unwrap();
    runtime.cancel_play_request();
    runtime.pending_play = None;
    runtime.saved.favorites_selected = true;
    let queue = runtime.catalog.entries[0].id.clone();
    let reply = number_command(&mut app, json!({"query":" 1 "})).unwrap();
    assert_eq!(reply["status"], "rejected");
    assert_eq!(app.ott.as_ref().unwrap().catalog.entries[0].id, queue);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_some());
}

#[test]
fn old_numeric_ack_does_not_consume_a_newer_numeric_selection() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    let mut second = live_entry();
    second.id = "second".into();
    app.ott.as_mut().unwrap().catalog.entries.push(second);
    install_temporary(&mut app, sid, &["42", "7"]);
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "first numeric command rejected: {reply:?}");
    let runtime = app.ott.as_ref().unwrap();
    let ticket = plx_media::route::play_request_ticket();
    let old = NativeStartReceipt { generation: runtime.play_generation, before: ticket, after: ticket };
    let reply = number_command(&mut app, json!({"query":"2"}));
    assert!(reply.is_none(), "newer numeric command rejected: {reply:?}");
    let mut runtime = app.ott.take().unwrap();
    assert!(numeric_remote::finish(&mut runtime, &app, Ok(()), Some(&old)).is_none());
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
    let request = runtime.play_target.as_ref().unwrap().clone();
    assert!(numeric_remote::staged_binding(&runtime, &request).unwrap().is_ok());
}

#[test]
fn borrowed_vportal_favorite_keeps_its_actual_lease_and_native_identity_on_numeric_rejection() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, sid) = fixture();
    let profile = ProviderProfile {
        id: "borrowed-portal".into(), name: "Borrowed portal".into(), kind: ProviderKind::VPortal,
        source: "portal::[key:fixture]https://fixture.invalid/nas/api".into(),
        source_alias: String::new(), enabled: true,
    };
    let mut entry = live_entry();
    entry.id = "borrowed-film".into();
    entry.media = MediaLocator::VPortal { request: json!({"id":"borrowed-film"}) };
    let favorite = selection::capture(&profile, &entry).unwrap();
    let selection = format!("favorite:{}", favorite.identity());
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles.push(profile.clone());
        runtime.saved.favorites.toggle(favorite).unwrap();
    }
    install_temporary(&mut app, sid, &["42", "7"]);
    // A direct global Favorite can borrow the accepted temporary queue without
    // changing its category or Favorites preference (the existing common path).
    explicit_play(&mut app, &selection);
    let owner = app.ott.as_ref().unwrap().play_target.as_ref().unwrap().transient_owner;
    lease_fixture_source(&mut app, "borrowed-server-session", true);
    let (lease, released, done) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    {
        let runtime = app.ott.as_mut().unwrap();
        let playing = runtime.playing.as_mut().unwrap();
        playing.profile = profile;
        playing.entry = entry;
        playing.selection_id = selection;
        playing.transient_owner = owner;
        runtime.active_lease = Some(lease.into());
        transient::plex_remote::admitted(runtime);
        runtime.play_target = None;
        runtime.play_profile = None;
        runtime.pending_play = None;
        runtime.resolver_next = None;
    }
    let identity = plx_media::route::playback_identity(&app.player.session);
    let generation = app.ott.as_ref().unwrap().play_generation;
    let saved = saved_value(&app);
    let reply = number_command(&mut app, json!({"query":"1"})).expect("leased borrowed stream must reject synchronously");
    assert_eq!(reply["status"], "rejected");
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.active_lease.as_ref().is_some_and(HeldLease::is_playback));
    assert!(released.try_recv().is_err(), "numeric rejection must not start lease release");
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.transient.accepted.is_some());
    assert!(plx_media::route::playback_identity(&app.player.session) == identity);
    assert_eq!(saved_value(&app), saved);
    // Complete the synthetic lease barrier without a session worker or network.
    done.send(()).unwrap();
}

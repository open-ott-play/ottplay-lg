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

struct NumericAcceptedResources;
impl playback::PlaybackResources for NumericAcceptedResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external source only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool { true }
}
/// Real ordinary admission/start/publication, replacing only the media-resource seam.
fn complete_numeric_external(app: &mut App, source: plx_media::ff::ExternalSource) {
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = numeric_remote::staged_binding(&runtime, &request).unwrap().unwrap();
    runtime.rebind_native_admission(app, &profile, &entry).unwrap();
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    let before = plx_media::route::play_request_ticket();
    runtime.play_ticket.take();
    runtime.resolver_next.take();
    let result = start_resolved_with(&mut runtime, app, profile, entry,
        ReadySource::External(source, info, None), &mut NumericAcceptedResources);
    assert!(result.is_ok(), "ordinary numeric start refused: {result:?}");
    let native = NativeStartReceipt { generation: runtime.play_generation, before,
        after: plx_media::route::play_request_ticket() };
    let reply = numeric_remote::finish(&mut runtime, app, result, Some(&native)).unwrap();
    assert_eq!(reply["status"], "ok");
    runtime.pending_play.take();
    runtime.take_completed_play_target();
    app.ott = Some(runtime);
}

#[test]
fn progressive_local_scan_cannot_merge_late_rows_or_bindings_after_numeric_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    use crate::app::ott_local_session as local;
    use super::super::local_integration as local_fixture;
    let directory = local_fixture::TempDir::new();
    let (mut app, _) = fixture();
    let root = local::BoundSource {
        id: "progressive-root".into(), name: "Progressive".into(),
        kind: local::SourceKind::Folder, available: true,
        authority_ref: local::AuthorityRef(9), generation: 7,
        canonical_path: Some(directory.folder("Progressive")),
    };
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.local_inventory.install(vec![local_fixture::acquire_root(root.clone())], 7);
    }
    std::fs::write(std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4"),
        b"fixture, never decoded").unwrap();
    let invocation = request(&app, "local_media", json!({
        "op":"play", "source":root.id, "session":true, "progressive":true
    }));
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    let scan_generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    let validated = ott_catalog::LocalBindings {
        operation_error: None, sources: runtime.local_inventory.sources.clone(),
        lookup: None, matched: vec![root.clone()],
    };
    transient::binding_result(&mut runtime, &mut app, scan_generation, Ok(validated));
    let mut file = live_entry();
    file.id = "first-local".into();
    file.media = MediaLocator::Local { relative_path: "first.mp4".into() };
    let mut incoming = Catalog { hierarchy: None, entries: vec![file.clone()] };
    assert!(transient::scan_result(&mut runtime, &mut app, &mut incoming, &mut None));
    assert!(runtime.play_target.is_some(), "first batch proposes playback before the scan completes");
    let response = transient::finish_native(&mut runtime, &mut app, Ok(()), None).unwrap();
    assert_eq!(response["status"], "ok");
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    runtime.pending_play.take(); runtime.play_target.take(); runtime.play_profile.take(); runtime.resolver_next.take();
    app.ott = Some(runtime);
    // Invalid input cannot cancel a still useful progressive scan.
    assert_eq!(number_command(&mut app, json!({"query":"0"})).unwrap()["status"], "rejected");
    assert!(!cancel.load(Ordering::Acquire));
    let reply = number_command(&mut app, json!({"query":"1"}));
    assert!(reply.is_none(), "numeric preparation rejected: {reply:?}");
    assert!(!cancel.load(Ordering::Acquire), "preparation cannot retire the scan before acceptance");
    complete_numeric_external(&mut app, plx_media::ff::ExternalSource::Http("https://media.invalid/live".into()));
    let runtime = app.ott.as_ref().unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert_ne!(runtime.generation, scan_generation);
    assert_eq!(runtime.catalog.entries[0].id, "live");
    let inventory_generation = runtime.local_inventory.generation();
    let catalog = runtime.catalog.clone();
    let mut late = file; late.id = "late-local".into();
    for event in [
        CatalogEvent::Batch(scan_generation, Catalog { hierarchy: None, entries: vec![late] }),
        CatalogEvent::Finished(scan_generation, Ok(())),
        CatalogEvent::LocalBindings(scan_generation, Ok(ott_catalog::LocalBindings {
            operation_error: None, sources: Vec::new(), lookup: None, matched: Vec::new(),
        })),
    ] {
        app.ott.as_ref().unwrap().catalog_tx.send(event).unwrap_or_else(|_| panic!("fixture receiver"));
    }
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog, catalog, "retired folder events cannot contaminate restored channels");
    assert!(runtime.catalog_complete && !runtime.catalog_loading);
    assert_eq!(runtime.local_inventory.generation(), inventory_generation);
    assert_eq!(runtime.local_inventory.sources.len(), 1, "stale bindings cannot withdraw live roots");
}

#[test]
fn server_library_numeric_restore_requires_no_filesystem_binding_and_keeps_source_fence() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for changed in [false, true] {
        let (mut app, sid) = fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.profiles.profiles[0].kind = ProviderKind::Local;
            runtime.saved.profiles.profiles[0].source = "https://library.invalid/catalog.json".into();
            runtime.provider = ProviderKind::Local;
            runtime.catalog.entries[0].media = MediaLocator::ServerLibrary {
                source_id: "library".into(), item_id: "film".into(), relative_path: None, root_alias: None,
            };
            // Observe the genuinely empty device inventory through the ordinary
            // startup binding path; no filesystem grant is fabricated.
            assert!(transient::take_binding_task(runtime).is_none());
            assert!(runtime.local_inventory.sources.is_empty());
        }
        install_temporary(&mut app, sid, &["42", "7"]);
        let reply = number_command(&mut app, json!({"query":"1"}));
        assert!(reply.is_none(), "HTTP Local numeric source rejected: {reply:?}");
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.play_target.as_ref().unwrap().local_selection.is_none());
        let request = runtime.play_target.as_ref().unwrap().clone();
        if changed {
            runtime.saved.profiles.profiles[0].source = "https://changed.invalid/catalog.json".into();
            assert!(numeric_remote::staged_binding(runtime, &request).unwrap().is_err());
        } else {
            complete_numeric_external(&mut app, plx_media::ff::ExternalSource::Http("https://library.invalid/film.mp4".into()));
            assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
            assert!(app.ott.as_ref().unwrap().local_inventory.sources.is_empty());
        }
    }
}

#[test]
fn filesystem_local_restore_keeps_the_exact_live_root_across_catalog_retirement() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    use crate::app::ott_local_session as local;
    use super::super::local_integration as local_fixture;
    for revoked in [false, true] {
        let directory = local_fixture::TempDir::new();
        let root = local::BoundSource {
            id: "configured-root".into(), name: "Configured".into(), kind: local::SourceKind::Folder,
            available: true, authority_ref: local::AuthorityRef(71), generation: 7,
            canonical_path: Some(directory.folder("Configured")),
        };
        let path = std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4");
        std::fs::write(&path, b"fixture, never decoded").unwrap();
        let (mut app, sid) = fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.profiles.profiles[0].kind = ProviderKind::Local;
            runtime.saved.profiles.profiles[0].source = root.canonical_path.clone().unwrap();
            runtime.provider = ProviderKind::Local;
            runtime.catalog.entries[0].media = MediaLocator::Local { relative_path: "first.mp4".into() };
            runtime.local_inventory.install(vec![local_fixture::acquire_root(root.clone())], 7);
        }
        install_temporary(&mut app, sid, &["42", "7"]);
        let reply = number_command(&mut app, json!({"query":"1"}));
        assert!(reply.is_none(), "configured filesystem Local rejected: {reply:?}");
        let original = app.ott.as_ref().unwrap().play_target.as_ref().unwrap().local_selection.clone().unwrap();
        let read = app.ott.as_ref().unwrap().capture_local_source(&original).unwrap();
        if revoked {
            let runtime = app.ott.as_mut().unwrap();
            let request = runtime.play_target.as_ref().unwrap().clone();
            let (profile, entry) = numeric_remote::staged_binding(runtime, &request).unwrap().unwrap();
            runtime.local_inventory.install(Vec::new(), 8);
            assert!(numeric_remote::staged_binding(runtime, &request).unwrap().is_err());
            let mut runtime = app.ott.take().unwrap();
            assert!(runtime.rebind_native_admission(&app, &profile, &entry).is_err());
            assert!(runtime.transient.accepted.is_some(), "revoked base root cannot retire Plex");
        } else {
            let generation = app.ott.as_ref().unwrap().generation;
            complete_numeric_external(&mut app, plx_media::ff::ExternalSource::Local(path));
            let runtime = app.ott.as_ref().unwrap();
            assert_ne!(runtime.generation, generation);
            let playing = runtime.playing.as_ref().unwrap();
            assert!(playing.local_selection.as_ref() == Some(&original));
            assert!(runtime.capture_local_source(&original).unwrap().same_grant(&read));
            let allowed = crate::app::ott_runtime::foreign_native_start_allowed(&mut app);
            assert!(allowed, "catalogue generation is not the root grant's generation; final gate: {}",
                app.ott.as_ref().unwrap().status);
            // Replacing even the same configured path cannot upgrade the old
            // explicit receipt to a newly opened root's different authority.
            let mut replacement = root.clone();
            replacement.authority_ref = local::AuthorityRef(72);
            replacement.generation = 8;
            app.ott.as_mut().unwrap().local_inventory.install(
                vec![local_fixture::acquire_root(replacement)], 8);
            assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app),
                "deferred Load must reject the replaced grant rather than rebind its path");
        }
    }
}

#[test]
fn same_protected_plex_source_keeps_its_grant_through_actual_deferred_load_revalidation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    use plx_media::{player, route};
    for change in ["none", "source", "expiry"] {
        let (mut app, sid) = fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            let profile = runtime.saved.profiles.profiles.iter().find(|profile| profile.source == "numeric-fixture").unwrap().clone();
            runtime.saved.profiles.active_profile = Some(profile.id.clone());
            runtime.provider = ProviderKind::Plex;
            runtime.entropy = Some([7; 16]);
            let entry = &mut runtime.catalog.entries[0];
            entry.id = "configured-42".into();
            entry.protection = Some(true);
            entry.media = MediaLocator::Plex { server_id: profile.source, rating_key: "42".into() };
        }
        parental_runtime::grant_current_profile_for_test(&mut app);
        install_temporary(&mut app, sid, &["42", "7"]);
        let reply = number_command(&mut app, json!({"query":"1"}));
        assert!(reply.is_none(), "authorized same-source numeric playback refused: {reply:?}");
        let mut runtime = app.ott.take().unwrap();
        let request = runtime.play_target.as_ref().unwrap().clone();
        let (profile, entry) = numeric_remote::staged_binding(&runtime, &request).unwrap().unwrap();
        runtime.rebind_native_admission(&app, &profile, &entry).unwrap();
        // Model the accepted ReadySource::Plex request/plan through existing
        // route fixtures. No native Load, resolve or account operation runs.
        let before = route::play_request_ticket();
        let audio = route::CarriedAudio::from_stream(&plx_data::metadata::Stream {
            id: 1, codec: "aac".into(), channels: 2, sampling_rate: 48_000, ..Default::default()
        }, 0);
        route::install_plex_track_fixture_for_test(&mut app.player.session, sid, "42", 77, audio);
        assert!(route::qualify_plex_direct_retry_fixture_for_test(&mut app.player.session, plx_plex::plex::client_for(sid).unwrap()));
        runtime.playing = Some(Playing {
            channel_capture: None, kiosk_selection: None, archive: None, transient_owner: None,
            local_selection: None, identity: route::playback_identity(&app.player.session),
            profile: profile.clone(), entry: entry.clone(), selection_id: entry.id,
            source: None, advance_on_failure: false, requires_server_lease: false,
        });
        let native = NativeStartReceipt { generation: runtime.play_generation, before,
            after: route::play_request_ticket() };
        assert_eq!(numeric_remote::finish(&mut runtime, &app, Ok(()), Some(&native)).unwrap()["status"], "ok");
        runtime.pending_play.take(); runtime.take_completed_play_target(); runtime.resolver_next.take();
        app.ott = Some(runtime);
        assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app),
            "the exact admitted source's grant must survive numeric publication");
        app.adapters.player.begin_release_for_test(std::time::Duration::ZERO);
        assert_eq!(player::start_bufferfeed_tracked(&mut app.player.session, &mut app.adapters.player),
            player::BufferfeedStartOutcome::Deferred);
        assert!(app.adapters.player.release_has_queued_start());
        match change {
            "source" => {
                let runtime = app.ott.as_mut().unwrap();
                runtime.saved.profiles.profiles.iter_mut().find(|saved| saved.id == profile.id).unwrap().source = "replaced-machine".into();
            }
            "expiry" => {
                let runtime = app.ott.as_mut().unwrap();
                runtime.clock_sample = Some(runtime.clock_sample.unwrap()
                    .advance_for_test(std::time::Duration::from_secs(3601)));
            },
            _ => (),
        }
        crate::app::run::publish_playback_frame(&mut app, false);
        assert_eq!(app.adapters.player.release_has_queued_start(), change == "none",
            "deferred Load must retain only unchanged live source authority: {change}");
        assert!(!app.adapters.player.is_live(), "resource seam must not start a real decoder");
        player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    }
}

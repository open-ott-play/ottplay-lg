//! Source-authored reentry regressions. The held host rig executes the actual
//! Session owner and registry publication; no verifier, filesystem writer,
//! catalog worker, network transport, or decoder is started.
use super::*;
use crate::app::{bridge, run};
use plx_plex::plex::{self, session::{Session, SourceRef}};
use plx_session::auth::{SessionCmd, SessionInit};

const SERVER: &str = "managed-reentry-fixture";
struct Reset;
impl Reset {
    fn new() -> Self { plex::reset_servers_for_test(); plex::grant::reset_for_test(); Self }
}
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        plex::reset_servers_for_test(); plex::grant::reset_for_test();
    }
}
fn settle(app: &mut App) {
    for _ in 0..24 {
        app.loop_t = app.loop_t.wrapping_add(16);
        bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: app.loop_t, dt_us: 16_000 }, Vec::new());
    }
}
fn permanent() -> ProviderProfile {
    ProviderProfile { id: "permanent".into(), name: "Permanent".into(), kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(), source_alias: "Permanent".into(), enabled: true }
}
fn entry() -> CatalogEntry {
    CatalogEntry { protection: None, synopsis: None, id: "managed:42".into(), title: "Managed movie".into(), category: String::new(),
        description: String::new(), poster: String::new(),
        media: MediaLocator::Plex { server_id: SERVER.into(), rating_key: "42".into() },
        duration_ms: None, provider_id: None, provider_id_field: None, live: None }
}
fn binding() -> context::Binding {
    context::Binding { candidate_generation: 1, authority_tag: 1, scope_tag: 1 }
}
fn fixture() -> (App, Runtime, ProviderProfile, Favorite) {
    let mut app = run::lifecycle_regression_tests::app();
    app.running = true;
    let disk = Session { client_id: "reentry-install".into(), ..Default::default() };
    app.bridge = bridge::Bridge::for_session_test(SessionInit::captured(disk.clone()));
    let source = SourceRef { machine_id: SERVER.into(), name: "Managed fixture".into(),
        address: "managed.invalid".into(), port: 32400, origin_url: "https://managed.invalid:32400".into(),
        token: "synthetic-pms-token".into(), tier: Some(plex::probe::Location::Local), ..Default::default() };
    let candidate = crate::app::ott_import::managed_candidate_for_test(&disk, source.clone());
    let init = app.bridge.snapshot_session_init();
    let activation = ManagedActivation { operation: 7, expected_epoch: init.epoch,
        expected_runtime: CredentialPatch::of(&init.persisted), expected_disk: init.disk_identity,
        credentials: candidate.credentials.clone() };
    let cancel = Arc::new(AtomicBool::new(false));
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    app.bridge.prepare_managed(activation.clone(), source, candidate.verified.proof.clone(),
        sample.deadline_at_unix_ms(sample.unix_ms() + 60_000).on_main(app.adapters.player.mt()),
        sample.deadline_at_unix_ms(sample.unix_ms() + 120_000).on_main(app.adapters.player.mt()), cancel.clone()).unwrap();
    bridge::execute_session_command(&mut app.pages, SessionCmd::ActivateManaged(activation));
    settle(&mut app);
    let completion = app.bridge.take_managed_completion().expect("actual registry/profile publication");
    assert!(completion.accepted);
    assert!(app.bridge.managed_active_current(7));
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    // Direct dispatch in this held fixture has the same initial clock input as
    // the frame runner, including PIN-policy paths that consult now_ms().
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_anchor = Some(clock);
    runtime.wall_clock_ms = clock.unix_ms();
    runtime.clock_sample = Some(clock);
    runtime.remote = Remote::disabled_for_test("dev_fixture", "reentry-fixture");
    runtime.loaded = true; runtime.entropy = Some([1;16]); runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release); runtime.resolver_busy = true;
    runtime.resume_source_work();
    let profile = ProviderProfile { id: "managed-7".into(), name: "Managed fixture".into(),
        kind: ProviderKind::Plex, source: SERVER.into(), source_alias: SERVER.into(), enabled: true };
    // The successful real publication above is projected without rerunning the
    // external candidate verifier. Context consumption remains the real store.
    runtime.managed.offers.list(0, binding(), "reentry-fixture", SERVER, "original-context").unwrap();
    runtime.managed.offers.admit(1, binding(), "reentry-fixture", SERVER, "original-context", 7, 60_000).unwrap();
    runtime.managed.offers.finish(7).unwrap();
    runtime.managed.active = Some(Active { operation: 7, generation: runtime.generation,
        profile: profile.clone(), category: String::new(), favorites: false,
        source: candidate, refresh: None, cancel });
    assert!(runtime.managed_catalog());
    let favorite = selection::capture_bound(&runtime.selection_bindings().unwrap(),
        selection::SelectionOwner::Profile(&profile), &entry()).unwrap();
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    // Start after a different persistent profile was accepted. Persistence is
    // not mocked: the test never invokes a writer or grants a write permit.
    runtime.saved.profiles.profiles = vec![permanent()];
    runtime.saved.profiles.active_profile = Some("permanent".into());
    runtime.load_catalog();
    (app, runtime, profile, favorite)
}

#[test]
fn managed_favorite_stays_bound_while_m3u_is_active_and_ui_resume_does_not_save() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, profile, favorite) = fixture();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert_eq!(runtime.effective_profile(), Some(&permanent()));
    assert!(matches!(runtime.favorite_bindings(&favorite).unwrap().resolve(&favorite),
        Ok(selection::SelectionOwner::Profile(bound)) if bound == &profile));
    runtime.play(&mut app, &format!("favorite:{}", favorite.identity())).unwrap();
    assert_eq!(runtime.resolver_next.as_ref().unwrap().profile, profile);
    assert_eq!(runtime.effective_profile(), Some(&permanent()), "playing a Favorite does not select its catalog");
    runtime.recover_plex_profiles(&mut app, false).unwrap();
    assert!(!app.bridge.snapshot_session_init().signin_active);
    let row = row(&runtime).unwrap(); assert!(row.enabled);
    assert!(matches!(&row.action, OttAction::ResumeManagedPlex { operation: 7 }));
    let old_generation = runtime.generation;
    let old_catalog = runtime.cancel.clone(); let old_play = runtime.play_cancel.clone();
    handle_action(&mut runtime, &mut app, row.action).unwrap();
    assert!(old_catalog.load(Ordering::Acquire) && old_play.load(Ordering::Acquire));
    assert!(runtime.generation > old_generation);
    assert_eq!(runtime.effective_profile(), Some(&profile));
    assert_eq!(runtime.catalog_next.as_ref(), Some(&profile));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving && runtime.rx.try_recv().is_err());
    assert_eq!(runtime.managed.offers.list(120_001, binding(), "reentry-fixture", SERVER, "replacement-context"),
        Err(context::ContextError::Stale), "UI resume cannot resurrect a consumed context");
    assert!(app.bridge.take_managed_completion().is_none(), "no second activation receipt");
}

#[test]
fn changed_managed_authority_refuses_favorite_queue_and_late_native_admission() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    for change in ["network", "identity", "owner"] {
        let (mut app, mut runtime, profile, favorite) = fixture();
        let id = format!("favorite:{}", favorite.identity());
        runtime.play(&mut app, &id).unwrap();
        assert!(runtime.resolver_next.is_some());
        app.ott = Some(runtime);
        crate::app::ott_runtime::tests::local_integration::install(&mut app, None, "independent-playing");
        runtime = app.ott.take().unwrap();
        observe(&mut runtime, &app);
        assert!(row(&runtime).unwrap().enabled);
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let generation = runtime.generation;
        match change {
            "network" => plex::grant::network_changed(),
            "identity" => plex::grant::identity_changed(),
            "owner" => { bridge::execute_session_command(&mut app.pages, SessionCmd::StartLogin); settle(&mut app); }
            _ => unreachable!(),
        }
        let revision = runtime.revision;
        observe(&mut runtime, &app);
        assert!(!row(&runtime).unwrap().enabled);
        assert!(runtime.revision > revision, "row eligibility changes must publish");
        assert_eq!(runtime.play(&mut app, &id), Err("Plex source is no longer available"), "{change}");
        assert_eq!(runtime.launch_resolver_from_app(&app), Err("Plex source is no longer available"), "{change}");
        assert_eq!(runtime.rebind_native_admission(&app, &profile, &entry()), Err("Plex source is no longer available"), "{change}");
        // Run the final callback path before managed::tick can retire Active.
        let before = plx_media::route::playback_identity(&app.player.session);
        assert_eq!(start_resolved(&mut runtime, &mut app, profile.clone(), entry(),
            ReadySource::Plex { server_id: SERVER.into(), rating_key: "42".into() }),
            Err("Plex source is no longer available"), "{change}");
        assert_eq!(plx_media::route::playback_identity(&app.player.session), before);
        assert!(resume_ui(&mut runtime, &mut app, 7).is_err(), "{change}");
        assert_eq!(runtime.generation, generation);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(guard_profile(&runtime, &app, &permanent()).is_ok(), "unrelated source remains eligible");
        if change == "network" { assert!(runtime.managed.refresh_requested); }
        drop(runtime); drop(app); plex::reset_servers_for_test(); plex::grant::reset_for_test();
    }
}

#[test]
fn only_explicit_resume_repoints_current_server_and_never_changes_the_owner() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, profile, _) = fixture();
    let managed_id = plex::id_of_machine(SERVER).unwrap();
    let origin = plex::Origin::parse("https://other.invalid:32400").unwrap();
    let proof = plex::grant::prove_import(plex::grant::scope(), &origin, "other-machine", "other-machine", false).unwrap();
    let other = plex::register_imported(&proof, "other-token", "reentry-install", Default::default(), None).unwrap();
    assert!(plex::set_current(other.server_id()));
    app.ott = Some(runtime);
    crate::app::ott_runtime::tests::local_integration::install(&mut app, None, "independent-playing");
    runtime = app.ott.take().unwrap();
    let playing = plx_media::route::playback_identity(&app.player.session);
    assert!(playing.is_some());
    let owner = serde_json::to_value(app.bridge.snapshot_session_init()).unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(guard_profile(&runtime, &app, &profile).is_ok());
    assert!(browse_active(&mut runtime, &app));
    assert_eq!(plex::current_server(), other.server_id(), "lookup and browse must not interrupt another Favorite");
    assert_eq!(plx_media::route::playback_identity(&app.player.session), playing);
    resume_ui(&mut runtime, &mut app, 7).unwrap();
    assert_eq!(plex::current_server(), managed_id);
    assert_eq!(serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(), owner);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    // Revoking only the managed intent cannot block an unrelated bound Favorite.
    runtime.managed.active.as_ref().unwrap().cancel.store(true, Ordering::Release);
    let unrelated = CatalogEntry { protection: None, synopsis: None, id: "live".into(), title: "Independent".into(),
        category: String::new(), description: String::new(), poster: String::new(),
        media: MediaLocator::Http { url: "https://media.invalid/live".into() },
        duration_ms: None, provider_id: Some("live".into()), provider_id_field: Some("id".into()), live: None };
    let favorite = selection::capture_bound(&runtime.selection_bindings().unwrap(),
        selection::SelectionOwner::Profile(&permanent()), &unrelated).unwrap();
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.play(&mut app, &format!("favorite:{}", favorite.identity())).unwrap();
    assert_eq!(runtime.resolver_next.as_ref().unwrap().profile, permanent());
    assert!(plex::retire_imported(&other));
}

#[test]
fn managed_resume_obeys_source_policy_and_retired_callback_keeps_its_identity() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, profile, _) = fixture();
    runtime.saved.settings.insert("parentPin".into(), serde_json::json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    runtime.saved.settings.insert("requirePinForProviderSelection".into(), serde_json::json!(1));
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let generation = runtime.generation;
    assert!(resume_ui(&mut runtime, &mut app, 7).is_err());
    assert_eq!(runtime.generation, generation);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    plex::grant::identity_changed();
    super::super::tick(&mut runtime, &mut app);
    assert!(runtime.managed.active.is_none());
    assert!(owns_retained_profile(&runtime, &profile));
    assert_eq!(guard_profile(&runtime, &app, &profile), Err("Plex source is no longer available"));
    assert!(guard_profile(&runtime, &app, &permanent()).is_ok());
}

// Hold the real planner output instead of starting its worker. A busy slot
// cannot capture catalogue authority; capture belongs to task admission.
fn take_held_managed_read(runtime: &mut Runtime, app: &App, expected: &ProviderProfile)
    -> crate::app::adapters::session::ManagedRead
{
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime.take_catalog_task_checked(Some(app)).is_none());
    assert!(runtime.catalog_next.as_ref() == Some(expected));
    assert!(runtime.managed.catalog.is_none(), "no task has captured catalogue authority yet");
    runtime.catalog_done.store(true, Ordering::Release);
    let task = runtime.take_catalog_task_checked(Some(app));
    runtime.catalog_done.store(false, Ordering::Release);
    let Some(ott_catalog::Task::ManagedPlex { profile, read }) = task
        else { panic!("the admitted catalogue task must own its managed read"); };
    assert_eq!(&profile, expected);
    assert!(runtime.catalog_next.is_none(), "the planner consumed exactly this task");
    assert!(guard_catalog_event(runtime, app, runtime.generation).is_ok());
    read
}

#[test]
fn captured_managed_reads_and_late_results_cannot_borrow_replacement_authority() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    for change in ["retoken", "lease"] {
        let (mut app, mut runtime, profile, favorite) = fixture();
        resume_ui(&mut runtime, &mut app, 7).unwrap();
        runtime.launch_catalog_from_app(&app);
        let id = format!("favorite:{}", favorite.identity());
        runtime.request_favorite_metadata(&id).unwrap();
        runtime.launch_metadata_from_app(&app);
        assert!(runtime.catalog_next.is_some() && runtime.metadata_next.is_some(), "workers remain held");
        let catalog_generation = runtime.generation;
        let metadata_generation = runtime.metadata_generation;
        let read = take_held_managed_read(&mut runtime, &app, &profile);
        assert!(read.current());
        if change == "retoken" {
            let client = plex::client_for(plex::id_of_machine(SERVER).unwrap()).unwrap();
            client.set_token("replacement-authority-token");
        } else {
            app.bridge.cancel_managed(7);
        }
        // Every call below refuses before HTTP; no new client is looked up.
        assert!(!read.current());
        assert!(read.metadata("42").is_none());
        assert!(read.sections().is_none());
        assert!(read.section_items(&plex::SectionQuery { section_key: 1, sort: "titleSort",
            filters: &[], start: 0, size: 128, include_meta: false }).is_none());
        assert!(guard_catalog_event(&runtime, &app, catalog_generation).is_err());
        assert!(guard_metadata_event(&runtime, &app, metadata_generation, &profile.id).is_err());
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        // Deliver actual old worker envelopes after revocation. The ordinary
        // tick must reject them before merging catalog/preview state.
        let mut late = entry(); late.title = "MUST NOT PUBLISH".into();
        assert!(runtime.catalog_tx.try_send(CatalogEvent::Batch(catalog_generation,
            Catalog { entries: vec![late.clone()], hierarchy: None })).is_ok());
        assert!(runtime.metadata_tx.send(crate::app::ott_metadata::Event::Preview(crate::app::ott_metadata::Preview {
            generation: metadata_generation, profile_id: profile.id.clone(), entry_id: id,
            entry: Some(late), result: Ok(serde_json::json!({"description":"MUST NOT PUBLISH"})),
        })).is_ok());
        runtime.plex_refresh_requested = false;
        app.ott = Some(runtime);
        let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
        crate::app::ott_runtime::tick(&mut app, sample);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog.entries.is_empty());
        assert!(runtime.favorite_preview.is_none());
        assert!(runtime.metadata_next.is_none() && runtime.catalog_next.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        drop(app); plex::reset_servers_for_test(); plex::grant::reset_for_test();
    }
}


#[test]
fn usb_reply_keeps_its_serial_when_managed_catalog_authority_is_revoked() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let (mut app, mut runtime, profile, _) = fixture();
    resume_ui(&mut runtime, &mut app, 7).unwrap();
    runtime.launch_catalog_from_app(&app);
    let generation = runtime.generation;
    assert!((1..=16).contains(&generation));
    assert!(guard_catalog_event(&runtime, &app, generation).is_ok());
    let read = take_held_managed_read(&mut runtime, &app, &profile);
    assert!(read.current());
    // The held catalogue has finished before the passive Local request starts.
    // Retain its captured read so a delayed old Batch still requires that owner.
    runtime.catalog_next = None;
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.resolver_busy = false;
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_sample = Some(sample);
    runtime.clock_anchor = Some(sample);
    runtime.wall_clock_ms = sample.unix_ms();
    let now = runtime.now_ms();
    let mut target = None;
    // Issue actual independent requests until their serial numerically equals
    // the old catalogue generation. Equality must not conflate the authorities.
    for step in 1..=generation {
        let body = json!({"commands":[],"server_time":1000,"requests":[{
            "id":format!("{step:032x}"),"action":"local_browser",
            "params":{"version":1,"runtime":"reentry-fixture","op":"roots"},
            "expires_at":1020}]}).to_string();
        let inv = runtime.remote.raw_poll_for_test(&body, now).remove(0);
        runtime.usb_browser_prepare_for_test(&inv).unwrap();
        let ott_catalog::Task::UsbRemote { serial, task, cancel } = runtime.take_usb_remote_task().unwrap()
            else { panic!("the admitted request must own a USB task"); };
        drop(task);
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(serial, step);
        if step == generation {
            target = Some(inv);
        } else {
            runtime.usb_remote_result(&mut app, serial, Err(crate::app::ott_usb::Error::Unavailable));
            assert_eq!(runtime.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["error"], "unavailable");
        }
    }
    let inv = target.unwrap();
    app.bridge.cancel_managed(7);
    assert!(!read.current());
    assert!(guard_profile(&runtime, &app, &profile).is_err());
    assert!(guard_catalog_event(&runtime, &app, generation).is_err());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_requested = false;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.catalog_tx.try_send(CatalogEvent::UsbRemote(generation,
        Err(crate::app::ott_usb::Error::Unavailable))).is_ok());
    let mut late = entry();
    late.title = "MUST NOT PUBLISH".into();
    assert!(runtime.catalog_tx.try_send(CatalogEvent::Batch(generation,
        Catalog { entries: vec![late], hierarchy: None })).is_ok());
    app.ott = Some(runtime);
    crate::app::ott_runtime::tick(&mut app, sample);
    let runtime = app.ott.as_ref().unwrap();
    let response = runtime.remote.response_for_id_for_test(&inv.id).expect("USB completion must not borrow Plex authority");
    assert_eq!(response["id"], inv.id);
    assert_eq!(response["data"]["runtime"], inv.params["runtime"]);
    assert_eq!(response["data"]["error"], "unavailable");
    assert!(!runtime.usb_remote_waiting_result());
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.local_inventory.sources.is_empty());
    assert!(runtime.play_target.is_none() && runtime.pending_play.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}


fn bind_portable_local_selection(runtime: &mut Runtime) -> continuity::ResumePoint {
    runtime.saved.settings.insert("offlineProfile:15".into(), json!({
        "version":1,"provider":"offline","name":"Films",
        "binding_id":"01234567-89ab-4def-8123-456789abcdef",
        "sources":[{"key":"s0","kind":"folder","name":"Films"}],
        "selected":{"source_key":"s0","name":"Comedy/example.mkv"}
    }));
    runtime.saved = commands::bind_local(runtime, 15, "/fixture/local-library").unwrap();
    commands::offline_selected_resume(&runtime.saved).unwrap()
}

#[test]
fn managed_catalog_browse_does_not_inherit_permanent_local_restore_or_autoplay() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, profile, _) = fixture();
    let expected = bind_portable_local_selection(&mut runtime);
    runtime.load_catalog();
    assert!(runtime.resume_key().is_some());
    assert_eq!(runtime.continuity.pending_restore(), Some(&expected),
        "the bound permanent Local profile still restores its imported selection");
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    // Use the real retained managed authority and UI reentry path, not a forged
    // active owner. The fixture holds the catalog worker slot; no source is read.
    resume_ui(&mut runtime, &mut app, 7).unwrap();
    assert_eq!(runtime.effective_profile(), Some(&profile));
    assert!(runtime.resume_key().is_none());
    assert!(runtime.continuity.pending_restore().is_none());
    assert_eq!(runtime.continuity.catalog_updated(runtime.generation, &["managed:42"], true),
        continuity::Decision::Idle, "opening the managed catalog carries no play intent");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving && runtime.rx.try_recv().is_err());
}

#[test]
fn managed_binding_validation_rejects_permanent_local_pending_restore() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, _, _) = fixture();
    let expected = bind_portable_local_selection(&mut runtime);
    runtime.load_catalog();
    runtime.validate_bound_resume();
    assert_eq!(runtime.continuity.pending_restore(), Some(&expected),
        "validation keeps the same point while its permanent Local profile owns the catalog");
    resume_ui(&mut runtime, &mut app, 7).unwrap();
    assert!(runtime.resume_key().is_none());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    // Present the previously valid point at the validation boundary. Its ID and
    // private folder metadata still match Saved, but the catalog owner changed.
    runtime.continuity = continuity::Continuity::new(runtime.generation, Some(expected));
    runtime.validate_bound_resume();
    assert!(runtime.continuity.pending_restore().is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!runtime.saving && runtime.rx.try_recv().is_err());
}

#[test]
fn managed_favorite_cannot_escape_nondurable_checkpoint_through_global_selection() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (app, mut runtime, profile, favorite) = fixture();
    runtime.saved.favorites_selected = true;
    runtime.continuity.stop(runtime.generation);
    let selection_id = format!("favorite:{}", favorite.identity());
    let playing = Playing { channel_capture: None, kiosk_selection: None, archive: None,
        transient_owner: None, local_selection: None, identity: None, profile: profile.clone(),
        entry: entry(), selection_id, source: None, advance_on_failure: true, requires_server_lease: false };
    assert!(managed::owns_profile(&runtime, &playing.profile));
    assert!(matches!(runtime.favorite_bindings(&favorite).unwrap().resolve(&favorite),
        Ok(selection::SelectionOwner::Profile(bound)) if bound == &profile));
    assert!(runtime.transient_checkpoint(&playing, 12_000));
    assert!(!runtime.durable_global_favorite(&playing), "global list membership cannot persist the ephemeral managed owner");
    // Exercise the ordinary exit checkpoint merge with the existing receipt
    // seam. The fixture proves managed ownership; it does not start a decoder.
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    runtime.playing = Some(playing);
    runtime.saving = true; // even an unguarded baseline must not launch a writer
    let receipt = plx_media::player::machine::ExitReceipt {
        cause: plx_media::player::machine::ExitCause::RequestedExit,
        playback_identity: None, position_ns: 12_000_000_000,
        sid: plex::id_of_machine(SERVER).unwrap(), rating_key: "42".into(), source: None,
    };
    runtime.checkpoint_at(&app, true, Some(&receipt));
    assert!(runtime.checkpoints.is_empty());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.rx.try_recv().is_err());

    // A durable source remains eligible while the managed catalog is retained.
    let permanent_profile = permanent();
    let mut permanent_entry = entry(); permanent_entry.id = "permanent-video".into();
    permanent_entry.media = MediaLocator::Http { url: "https://media.invalid/video.mp4".into() };
    let permanent_favorite = selection::capture(&permanent_profile, &permanent_entry).unwrap();
    let permanent_id = format!("favorite:{}", permanent_favorite.identity());
    runtime.saved.favorites.toggle(permanent_favorite).unwrap();
    let mut permanent_playing = runtime.playing.take().unwrap();
    permanent_playing.profile = permanent_profile; permanent_playing.entry = permanent_entry;
    permanent_playing.selection_id = permanent_id;
    assert!(runtime.durable_global_favorite(&permanent_playing));
}

// Reuse the real Session/registry publication above. Only the already-playing
// media identity is modeled with the existing route seam; no resolver, metadata
// runner or decoder starts, and the temporary profile never enters Saved.
fn playing_favorite_fixture() -> (App, Runtime, ProviderProfile, Favorite) {
    let (mut app, mut runtime, profile, favorite) = fixture();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(&mut app);
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    settle(&mut app);
    let sid = plex::id_of_machine(SERVER).unwrap();
    plx_media::route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    assert!(runtime.saved.favorites.entries().contains(&favorite));
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    assert!(runtime.saved.favorites.entries().is_empty());
    runtime.continuity.stop(runtime.generation);
    runtime.plex_refresh_pending = true;
    runtime.playing = Some(Playing {
        channel_capture: None, kiosk_selection: None, archive: None,
        transient_owner: None, local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile: profile.clone(), entry: entry(), selection_id: "managed:42".into(),
        source: None, advance_on_failure: false, requires_server_lease: false,
    });
    assert!(runtime.playing.as_ref().unwrap().identity.is_some());
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), &app));
    assert!(guard_profile(&runtime, &app, &profile).is_ok());
    assert!(matches!(app.route(), AppArg::Player));
    assert!(!app.pages.has_pending_navigation() && !app.pages.surface_up());
    assert!(!runtime.saved.profiles.profiles.iter().any(|saved| saved.id == profile.id));
    (app, runtime, profile, favorite)
}

fn playing_favorite_current(runtime: &mut Runtime, app: &mut App, serial: u64) -> serde_json::Value {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":"favorites_global_items",
        "params":{"runtime":runtime.remote.runtime(),"op":"current","subject":"global"},
        "expires_at":1020}]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let inv = requests.remove(0);
    assert!(runtime.remote.invocation_is_current(&inv));
    let response = commands::command(runtime, app, &inv).expect("current is a read-only reply");
    runtime.remote.complete(inv, response.clone());
    response
}

// The ordinary toggle owns Pending. A revoked permit refuses before storage;
// the test then presents the candidate as a completed write to test settlement.
// This is not a disk durability test and never synthesizes a managed authority.
fn stage_playing_favorite(runtime: &mut Runtime, app: &mut App) -> Saved {
    let (candidate, _, added) = playing_favorite::candidate(runtime, runtime.playing.as_ref().unwrap()).unwrap();
    assert!(added);
    let before = runtime.saved.favorites.clone();
    playing_favorite::toggle(runtime, app).unwrap();
    assert!(runtime.saving && runtime.pending_playing_favorite.is_some());
    assert_eq!(runtime.saved.favorites, before, "no optimistic membership publication");
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Job::Saved(Err(_), None, false, _)), "the revoked writer must refuse before storage");
    runtime.saving = false;
    candidate
}

#[test]
fn managed_playing_favorite_capture_and_toggle_preserve_persistent_profiles() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (mut app, mut runtime, _, favorite) = playing_favorite_fixture();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let owner = serde_json::to_value(app.bridge.snapshot_session_init()).unwrap();
    let playing = runtime.playing.as_ref().unwrap().clone();
    assert_eq!(playing_favorite::capture(&runtime, &playing).unwrap(), favorite);
    let response = playing_favorite_current(&mut runtime, &mut app, 1);
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["entry"], serde_json::to_value(&favorite).unwrap());
    assert!(response["data"]["unavailable_reason"].is_null());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);

    let (candidate, id, added) = playing_favorite::candidate(&runtime, &playing).unwrap();
    assert!(added);
    assert_eq!(id, favorite.identity());
    assert_eq!(candidate.favorites.entries(), &[favorite]);
    let mut fields = serde_json::to_value(&candidate).unwrap();
    fields["favorites"] = saved["favorites"].clone();
    assert_eq!(fields, saved, "capture must not migrate profile, credentials, category or resume");
    // Model publication only to exercise the inverse pure candidate operation.
    runtime.saved = candidate;
    let (removed, removed_id, added) = playing_favorite::candidate(&runtime, &playing).unwrap();
    assert!(!added);
    assert_eq!(removed_id, id);
    assert_eq!(serde_json::to_value(&removed).unwrap(), saved);
    assert_eq!(serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(), owner);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn revoked_managed_playing_favorite_refuses_current_and_new_toggle() {
    let _serial = plx_base::testlock::serial();
    for change in ["retoken", "lease"] {
        let _reset = Reset::new();
        let (mut app, mut runtime, profile, favorite) = playing_favorite_fixture();
        let before = playing_favorite_current(&mut runtime, &mut app, 1);
        assert_eq!(before["data"]["entry"], serde_json::to_value(&favorite).unwrap());
        if change == "retoken" {
            plex::client_for(plex::id_of_machine(SERVER).unwrap()).unwrap().set_token("replacement-authority-token");
        } else {
            app.bridge.cancel_managed(7);
        }
        assert!(guard_profile(&runtime, &app, &profile).is_err());
        assert!(playing_matches(runtime.playing.as_ref().unwrap(), &app), "media identity alone stays the same");
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let owner = serde_json::to_value(app.bridge.snapshot_session_init()).unwrap();
        let response = playing_favorite_current(&mut runtime, &mut app, 2);
        assert_eq!(response["status"], "ok");
        assert!(response["data"]["entry"].is_null(), "{change}");
        assert!(!response["data"]["unavailable_reason"].is_null(), "{change}");
        assert!(playing_favorite::toggle(&mut runtime, &mut app).is_err(), "{change}");
        assert!(!runtime.saving && runtime.pending_playing_favorite.is_none());
        assert!(runtime.rx.try_recv().is_err());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(), owner);
    }
}

#[test]
fn managed_playing_favorite_late_completion_cannot_borrow_replacement_authority() {
    let _serial = plx_base::testlock::serial();
    for change in ["lease", "refresh"] {
        let _reset = Reset::new();
        let (mut app, mut runtime, profile, _) = playing_favorite_fixture();
        let old_read = capture_read(&runtime, &app, &profile).unwrap().unwrap();
        assert!(old_read.current());
        let candidate = stage_playing_favorite(&mut runtime, &mut app);
        let owner = serde_json::to_value(app.bridge.snapshot_session_init()).unwrap();
        if change == "lease" {
            app.bridge.cancel_managed(7);
            assert!(guard_profile(&runtime, &app, &profile).is_err());
        } else {
            plex::grant::network_changed();
            assert!(!old_read.current());
            assert!(guard_profile(&runtime, &app, &profile).is_err());
            let origin = plex::Origin::parse("https://managed.invalid:32400").unwrap();
            let proof = plex::grant::prove_import(plex::grant::scope(), &origin, SERVER, SERVER, true).unwrap();
            assert!(app.bridge.refresh_managed(7, proof));
            assert!(guard_profile(&runtime, &app, &profile).is_ok(), "same operation now has fresh authority");
            assert!(capture_read(&runtime, &app, &profile).unwrap().unwrap().current());
            assert_eq!(serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(), owner,
                "refresh does not publish new credentials or select another profile");
        }
        assert!(!old_read.current(), "an admitted old write cannot borrow the new resource");
        assert!(playing_matches(runtime.playing.as_ref().unwrap(), &app));
        let committed = serde_json::to_value(&candidate).unwrap();
        runtime.saved = candidate;
        assert!(!playing_favorite::settled(&mut runtime, &mut app, true), "{change}");
        assert!(runtime.pending_playing_favorite.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), committed,
            "already committed portable metadata is not rolled back when resource authority expires");
        assert!(runtime.rx.try_recv().is_err());
    }
}


// The route preparation below never starts a decoder or reads its synthetic URL.
// It leaves a real pending native start for the normal retirement path to cancel.
fn prepare_navigation_media(app: &mut App, runtime: Runtime) -> Runtime {
    app.ott = Some(runtime);
    crate::app::ott_runtime::tests::local_integration::install(app, None, "managed-navigation-media");
    assert!(plx_media::route::playback_identity(&app.player.session).is_some());
    assert!(plx_media::route::has_url(&app.player.session));
    assert!(plx_media::route::pending_route_start().is_some());
    app.ott.take().unwrap()
}

fn assert_navigation_media_retired(app: &App) {
    // Stop intentionally retains logical target identity for a possible retry.
    // The admitted start and playable URL must nevertheless be retired.
    assert!(plx_media::route::playback_identity(&app.player.session).is_some());
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(plx_media::route::pending_route_start().is_none());
    assert!(app.player.pending_exit.is_none(), "catalogue replacement is not a user Exit receipt");
}

#[test]
fn managed_catalogue_resume_keeps_the_settled_ott_entry_and_retires_native_media() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (app, runtime, profile, _) = fixture();
    let mut owned_app = Box::new(app);
    let app = owned_app.as_mut();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(app);
    let ott_entry = app.pages.nav.top_page().unwrap().id;
    assert!(app.route() == AppArg::Ott);
    assert!(bridge::player(&app.pages).is_none());
    assert!(!app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending());
    let mut runtime = prepare_navigation_media(app, runtime);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    app.refresh_hubs = false;

    resume_ui(&mut runtime, app, 7).unwrap();

    assert_navigation_media_retired(app);
    assert!(!app.refresh_hubs, "no playback page was exited, so no Home refresh is owed");
    assert!(!app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending());
    assert!(runtime.effective_profile() == Some(&profile));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    settle(app);
    assert!(app.route() == AppArg::Ott);
    assert!(app.pages.nav.top_page().unwrap().id == ott_entry, "keep the actual catalogue entry");
}

#[test]
fn managed_resume_exits_a_mounted_player_to_its_original_ott_entry() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (app, runtime, _, _) = fixture();
    let mut owned_app = Box::new(app);
    let app = owned_app.as_mut();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(app);
    let ott_entry = app.pages.nav.top_page().unwrap().id;
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    settle(app);
    assert!(app.route() == AppArg::Player);
    assert!(bridge::player(&app.pages).unwrap().origin.unwrap().entry == ott_entry);
    let mut runtime = prepare_navigation_media(app, runtime);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    app.refresh_hubs = false;

    resume_ui(&mut runtime, app, 7).unwrap();

    assert_navigation_media_retired(app);
    assert!(app.refresh_hubs, "a real Player exit retains the existing refresh policy");
    assert!(app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    settle(app);
    assert!(app.route() == AppArg::Ott);
    assert!(app.pages.nav.top_page().unwrap().id == ott_entry, "PopTo retains the real return target");
    assert!(bridge::player(&app.pages).is_none());
}

#[test]
fn managed_catalogue_retirement_does_not_leave_a_queued_player_mount() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let (app, runtime, _, _) = fixture();
    let mut owned_app = Box::new(app);
    let app = owned_app.as_mut();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(app);
    let _runtime = prepare_navigation_media(app, runtime);
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    assert!(bridge::player(&app.pages).is_none());
    assert!(app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending());
    app.refresh_hubs = false;

    super::super::retire_for_catalog(app);

    assert_navigation_media_retired(app);
    assert!(app.refresh_hubs, "pending navigation retains the existing exit behavior");
    settle(app);
    assert!(app.route() == AppArg::Home, "the existing no-mounted-origin fallback replaces the pending Player");
    assert!(bridge::player(&app.pages).is_none());
    assert!(!app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending());
}

#[test]
fn parental_foreign_owned_managed_favorite_revalidates_actual_session_without_selecting_its_catalogue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let (mut app, mut runtime, profile, favorite) = fixture();
    let sid = plex::id_of_machine(SERVER).unwrap();
    plx_media::route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    runtime.playing = Some(Playing { channel_capture:None, kiosk_selection:None, archive:None,
        transient_owner:None, local_selection:None,
        identity:plx_media::route::playback_identity(&app.player.session), profile,
        entry:entry(), selection_id:format!("favorite:{}", favorite.identity()), source:None,
        advance_on_failure:false, requires_server_lease:false });
    app.ott = Some(runtime);
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    assert!(app.ott.as_ref().unwrap().effective_profile() == Some(&permanent()));
    app.ott.as_mut().unwrap().managed.active.as_ref().unwrap().cancel.store(true, Ordering::Release);
    assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    assert!(!app.adapters.player.is_live());
}

#[test]
fn parental_protected_retained_managed_favorite_uses_its_resource_pin_scope_without_a_saved_duplicate() {
    let _serial = plx_base::testlock::serial();
    for mutation in 0..3 {
        let _reset = Reset::new();
        let (mut app, mut runtime, profile, _) = fixture();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        assert!(!runtime.saved.profiles.profiles.iter().any(|p| p == &profile));
        let mut protected = entry();
        protected.protection = Some(true);
        assert!(runtime.require_parental_entry(&profile, Some(&protected)).is_err());
        parental_runtime::request_foreign_pin(&mut runtime, &mut app, &profile);
        app.ott = Some(runtime);
        settle(&mut app);
        let owner = app.pages.top_page().unwrap();
        let mut runtime = app.ott.take().unwrap();
        runtime.reply = Some(owner);
        parental_runtime::tick(&mut runtime, &mut app);
        assert_eq!(runtime.edit_session.as_ref().unwrap().key, "parental.unlock");
        app.ott = Some(runtime);
        // Complete the existing Gate's verified worker fixture; PIN derivation
        // and the real editor are independently covered by runtime tests.
        parental_runtime::grant_current_profile_for_test(&mut app);
        assert!(app.ott.as_mut().unwrap().require_parental_entry(&profile, Some(&protected)).is_ok());
        match mutation {
            0 => parental_runtime::expire_current_profile_for_test(&mut app),
            1 => { app.ott.as_mut().unwrap().managed.active.as_ref().unwrap().cancel.store(true, Ordering::Release); }
            _ => { app.ott.as_mut().unwrap().managed.active.as_mut().unwrap().cancel = Arc::new(AtomicBool::new(false)); }
        }
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.require_parental_entry(&profile, Some(&protected)).is_err());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(runtime.effective_profile(), Some(&permanent()));
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
}

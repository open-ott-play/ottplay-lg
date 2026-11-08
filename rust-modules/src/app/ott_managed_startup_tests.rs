//! Scheduling through the real managed tick and Remote load completion. Only
//! external verification and the final selection dispatch are held by the test.
use super::*;
use crate::app::ott_import::managed_candidate_for_test;
use plx_plex::plex::{self, session::{Session, SourceRef}};

const SERVER: &str = "managed-startup-fixture";

struct RegistryReset;
impl RegistryReset {
    fn new() -> Self {
        plex::reset_servers_for_test();
        plex::grant::reset_for_test();
        Self
    }
}
impl Drop for RegistryReset {
    fn drop(&mut self) {
        plex::reset_servers_for_test();
        plex::grant::reset_for_test();
    }
}

fn fixture(initial: Saved) -> (App, Runtime, Session, Sender<CandidateResult>) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    app.ott = Some(runtime);
    // Establish ordinary empty OTT readiness through its actual result handler.
    for ready in [false, true] {
        if ready {
            app.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((initial.clone(), [1; 16]))))
                .unwrap_or_else(|_| panic!("fixture state receiver disappeared"));
        }
        let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
        super::super::tick(&mut app, sample);
    }
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.loaded);
    runtime.remote = Remote::from_install("dev_fixture", "initial-runtime");
    let mut held = None;
    assert!(start_with(&mut runtime, &app, |saved, _, sender| {
        held = Some((saved, sender));
        true
    }));
    let (saved, sender) = held.unwrap();
    (app, runtime, saved, sender)
}

fn deliver(saved: &Session, sender: Sender<CandidateResult>) {
    let source = SourceRef {
        machine_id: SERVER.into(), name: "Fixture Plex".into(),
        address: "127.0.0.1".into(), port: 32499,
        origin_url: "http://127.0.0.1:32499".into(), token: "synthetic-pms-token".into(),
        tier: Some(plex::probe::Location::Local), ..Default::default()
    };
    assert!(sender.send(Ok(Some(managed_candidate_for_test(saved, source)))).is_ok());
}

fn pump(runtime: &mut Runtime, app: &mut App) -> Vec<(String, String)> {
    let mut selected = Vec::new();
    tick_with(runtime, app, |_, _, server, context| {
        selected.push((server.to_owned(), context.to_owned()));
        Ok(())
    });
    selected
}

fn complete_load(runtime: &mut Runtime, result: Result<(), &'static str>) {
    runtime.remote.queue_load_for_test(result.map(|()| (
        crate::app::ott_remote::Config::disabled("dev_fixture".into()), true, "loaded-runtime".into())));
    let now = runtime.now_ms();
    assert!(runtime.remote.tick(now, json!({})).is_empty());
    assert!(runtime.remote.configuration_load_settled());
}

#[test]
fn managed_first_waits_for_real_control_load_before_minting_automatic_context() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(Saved::default());
    let before = runtime.remote.authority_generation();
    deliver(&saved, sender);
    assert!(pump(&mut runtime, &mut app).is_empty(), "must not dispatch before the initial load");
    assert!(runtime.managed.automatic);
    assert!(runtime.managed.row.is_none());
    complete_load(&mut runtime, Ok(()));
    assert_ne!(runtime.remote.authority_generation(), before);
    let selected = pump(&mut runtime, &mut app);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].0, SERVER);
    let binding = runtime.managed.binding(&runtime);
    assert_eq!(runtime.managed.offers.check(runtime.now_ms(), binding, runtime.remote.runtime(),
        &selected[0].0, &selected[0].1), Ok(()));
    assert!(pump(&mut runtime, &mut app).is_empty(), "automatic selection dispatches once");
    assert!(plex::id_of_machine(SERVER).is_none(), "selection dispatch is not a grant");
}

#[test]
fn control_first_dispatches_when_the_verified_candidate_arrives() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(Saved::default());
    complete_load(&mut runtime, Ok(()));
    assert!(pump(&mut runtime, &mut app).is_empty());
    deliver(&saved, sender);
    assert_eq!(pump(&mut runtime, &mut app).len(), 1);
}

#[test]
fn failed_control_load_settles_without_granting_or_blocking_managed_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(Saved::default());
    deliver(&saved, sender);
    assert!(pump(&mut runtime, &mut app).is_empty());
    let before = runtime.remote.authority_generation();
    complete_load(&mut runtime, Err("fixture control load failed"));
    assert_eq!(runtime.remote.authority_generation(), before);
    assert_eq!(runtime.remote.snapshot()["state"], "error");
    assert_eq!(runtime.remote.snapshot()["persisted"], false);
    assert_eq!(pump(&mut runtime, &mut app).len(), 1);
    assert!(runtime.managed.pending.is_none());
    assert!(app.bridge.snapshot_session_init().managed.is_none());
    assert!(plex::id_of_machine(SERVER).is_none());
}

#[test]
fn later_control_authority_change_still_rejects_the_original_context() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(Saved::default());
    complete_load(&mut runtime, Ok(()));
    deliver(&saved, sender);
    let selected = pump(&mut runtime, &mut app);
    assert_eq!(selected.len(), 1);
    runtime.remote.configure(json!({"device_id":"dev_fixture","enabled":false})).unwrap();
    assert_eq!(select_ui(&mut runtime, &mut app, &selected[0].0, &selected[0].1), Err("source_changed"));
    assert!(runtime.managed.pending.is_none());
    assert!(app.bridge.snapshot_session_init().managed.is_none());
    assert!(plex::id_of_machine(SERVER).is_none());
}

#[test]
fn explicit_intent_while_control_load_waits_cannot_be_rearmed_by_its_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(Saved::default());
    deliver(&saved, sender);
    assert!(pump(&mut runtime, &mut app).is_empty());
    navigation(&mut runtime.managed);
    complete_load(&mut runtime, Ok(()));
    assert!(pump(&mut runtime, &mut app).is_empty());
    assert!(!runtime.managed.automatic);
    assert!(runtime.managed.pending.is_none());
}

#[test]
fn accepted_managed_completion_preserves_the_unmounted_initial_ott_root() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut owned_app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    let app = owned_app.as_mut();
    app.running = true;
    let disk = Session { client_id: "managed-boot-install".into(), ..Default::default() };
    app.bridge = bridge::Bridge::for_session_test(plx_session::auth::SessionInit::captured(disk.clone()));
    assert!(app.boot_initial.is_none(), "ordinary owner, not controlled record/replay boot");
    assert!(app.pages.top_arg().is_none());
    // Ordinary boot's root stays pending across the owner/adapter drain.
    bridge::nav_root(&mut app.pages, AppArg::Ott);
    assert!(app.pages.has_pending_navigation());
    assert!(app.pages.top_or_mounting_arg() == Some(&AppArg::Ott));

    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_anchor = Some(sample);
    runtime.clock_sample = Some(sample);
    runtime.wall_clock_ms = sample.unix_ms();
    runtime.remote = Remote::disabled_for_test("dev_fixture", "managed-boot-runtime");
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    let source = SourceRef { machine_id: SERVER.into(), name: "Fixture Plex".into(),
        address: "127.0.0.1".into(), port: 32499, origin_url: "http://127.0.0.1:32499".into(),
        token: "synthetic-pms-token".into(), tier: Some(plex::probe::Location::Local), ..Default::default() };
    let candidate = managed_candidate_for_test(&disk, source.clone());
    let authority = Authority::capture(app);
    let activation = ManagedActivation { operation: 17, expected_epoch: authority.epoch,
        expected_runtime: authority.credentials.clone(), expected_disk: authority.disk.clone(),
        credentials: candidate.credentials.clone() };
    let now = runtime.now_ms();
    let remote_runtime = runtime.remote.runtime().to_owned();
    let deadline = now + 60_000;
    let command_deadline = sample.deadline_at_unix_ms(deadline);
    let context_deadline = sample.deadline_at_unix_ms(now + 120_000);
    let cancel = Arc::new(AtomicBool::new(false));
    let binding = runtime.managed.binding(&runtime);
    runtime.managed.offers.list(now, binding, &remote_runtime, SERVER, "boot-context").unwrap();
    runtime.managed.offers.admit(now, binding, &remote_runtime, SERVER,
        "boot-context", 17, deadline).unwrap();
    app.bridge.prepare_managed(activation.clone(), source, candidate.verified.proof.clone(),
        command_deadline.on_main(app.adapters.player.mt()), context_deadline.on_main(app.adapters.player.mt()),
        cancel.clone()).unwrap();
    runtime.managed.authority = Some(authority);
    runtime.managed.pending = Some(Pending { operation: 17, deadline, command_deadline, context_deadline,
        navigation: runtime.navigation_epoch, expected_epoch: activation.expected_epoch,
        generation: runtime.generation, source: candidate, binding, server_id: SERVER.into(),
        context: "boot-context".into(), invocation: None, request: None, cancel, worker: None, staged: true });
    bridge::execute_session_command(&mut app.pages, plx_session::auth::SessionCmd::ActivateManaged(activation));
    let parts = app.pages.parts(plx_machine::machine::Tick::default());
    app.pages.drain(&mut app.bridge, &parts, 128, &mut plx_ui::dispatch::FrameReport::default(),
        &mut plx_ui::dispatch::NoTap);
    assert!(app.bridge.managed_active_current(17), "the real owner and adapter accepted publication");
    assert!(plex::id_of_machine(SERVER).is_some());
    assert!(app.pages.top_arg().is_none(), "the initial root is still pending");
    assert!(app.pages.top_or_mounting_arg() == Some(&AppArg::Ott));
    let owner = serde_json::to_value(app.bridge.snapshot_session_init()).unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let epochs = (runtime.generation, runtime.navigation_epoch, runtime.playback_epoch);
    assert_eq!(epochs, (0, 0, 0));

    tick(&mut runtime, app);

    assert!(runtime.managed.pending.is_none());
    assert!(runtime.managed.active.as_ref().is_some_and(|active| active.operation == 17));
    assert!(runtime.catalog_next.as_ref().is_some_and(|profile| profile.source == SERVER));
    assert!(runtime.generation > epochs.0, "accepted source owns a new catalogue generation");
    assert_eq!(runtime.navigation_epoch, epochs.1, "no viewer navigation was synthesized");
    assert_eq!(runtime.playback_epoch, epochs.2 + 1, "source replacement retires the prior playback epoch once");
    assert!(!app.refresh_hubs, "no Player was exited and Home must not acquire a refresh");
    assert!(app.player.pending_exit.is_none());
    assert!(app.pages.top_or_mounting_arg() == Some(&AppArg::Ott), "do not replace the boot request with Home");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(), owner);
    assert!(app.bridge.take_managed_completion().is_none(), "one completion was consumed");
    app.ott = Some(runtime);
    for step in 1..=24 {
        bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: step * 16, dt_us: 16_000 }, Vec::new());
    }
    assert!(app.route() == AppArg::Ott);
    assert!(app.pages.top_page().is_some(), "the real OTT screen mounted");
    assert!(bridge::player(&app.pages).is_none());
    assert!(!app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending());
}


/// Exercise the same serialized key written by the provider command. This does
/// not substitute Runtime.provider for persisted intent or manufacture a profile.
fn persisted_choice(choice: Option<&str>) -> Saved {
    let mut saved = Saved::default();
    if let Some(choice) = choice {
        saved.settings.insert("selectedProvider".into(), json!(choice));
    }
    let restored: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    restored.validate().unwrap();
    restored
}

#[test]
fn persisted_non_plex_provider_without_profile_prevents_automatic_plex() {
    let _serial = plx_base::testlock::serial();
    for choice in ["m3u", "vportal", "local", "offline", "xtream", "stalker"] {
        let _reset = RegistryReset::new();
        let initial = persisted_choice(Some(choice));
        let before = serde_json::to_value(&initial).unwrap();
        let (mut app, mut runtime, saved, sender) = fixture(initial);
        assert!(runtime.saved.profiles.active_profile.is_none());
        assert!(runtime.saved.profiles.profiles.is_empty());
        assert_eq!((runtime.generation, runtime.navigation_epoch, runtime.playback_epoch), (0, 0, 0));
        complete_load(&mut runtime, Ok(()));
        deliver(&saved, sender);
        assert!(pump(&mut runtime, &mut app).is_empty(), "persisted {choice} must prevent Plex dispatch");
        assert!(!runtime.managed.automatic);
        assert!(runtime.managed.pending.is_none());
        assert!(runtime.managed.active.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(plex::id_of_machine(SERVER).is_none());
    }
}

#[test]
fn absent_provider_and_explicit_plex_keep_initial_automatic_selection() {
    let _serial = plx_base::testlock::serial();
    for choice in [None, Some("plex")] {
        let _reset = RegistryReset::new();
        let initial = persisted_choice(choice);
        assert_eq!(initial.settings.get("selectedProvider").and_then(Value::as_str), choice);
        let before = serde_json::to_value(&initial).unwrap();
        let (mut app, mut runtime, saved, sender) = fixture(initial);
        complete_load(&mut runtime, Ok(()));
        deliver(&saved, sender);
        let selected = pump(&mut runtime, &mut app);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, SERVER);
        assert!(pump(&mut runtime, &mut app).is_empty(), "dispatch remains one-shot");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn persisted_m3u_without_candidate_does_not_fall_back_to_plex_login() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let (mut app, mut runtime, saved, sender) = fixture(persisted_choice(Some("m3u")));
    assert!(!saved.can_go_local(), "fixture must exercise the login-fallback condition");
    complete_load(&mut runtime, Ok(()));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let epoch = app.bridge.auth_read().0.flow_epoch;
    assert!(sender.send(Ok(None)).is_ok());
    assert!(pump(&mut runtime, &mut app).is_empty());
    assert!(runtime.managed.startup_settled);
    // Drain any real queued Session command through the existing held test rig.
    // Before the fix StartLogin is emitted here and advances its flow epoch.
    app.pages.frame_with(&mut app.bridge, plx_machine::machine::Tick::default(),
        Vec::new(), Vec::new(), &mut plx_ui::dispatch::NoTap, false);
    assert_eq!(app.bridge.auth_read().0.flow_epoch, epoch);
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn legacy_saved_default_does_not_synthesize_provider_intent() {
    let restored: Saved = serde_json::from_value(json!({"version":1})).unwrap();
    restored.validate().unwrap();
    assert!(!restored.settings.contains_key("selectedProvider"));
    assert!(restored.profiles.active_profile.is_none());
    let restored: Saved = serde_json::from_slice(&serde_json::to_vec(&restored).unwrap()).unwrap();
    assert!(!restored.settings.contains_key("selectedProvider"));
}

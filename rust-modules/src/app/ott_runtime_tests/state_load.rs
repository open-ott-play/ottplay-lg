//! Protected-state failure and retry admission, without protected-store IO.
//! Mounted tests cross the real dispatcher/Bridge/Runtime boundary. The injected
//! spawn callback retains only the result sender; no worker body is executed.
//! Synthetic Loaded messages prove completion handling, not durable storage.
use super::*;
use plx_machine::machine::Key;

const LOAD_ERROR: &str = "Could not read protected OTT state";

fn mount_ott(app: &mut App) {
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    runtime_tick(app);
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(!app.pages.has_pending_navigation());
}

fn action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(
        OttOut::Action { reply, action },
    )));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn deliver(app: &mut App, result: Result<(Saved, [u8; 16]), &'static str>) {
    app.ott.as_ref().unwrap().tx.send(Job::Loaded(result))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

fn failed_app() -> App {
    let mut app = app_fixture();
    mount_ott(&mut app);
    assert!(app.ott.as_mut().unwrap().start_state_load_with(|_, _| true));
    deliver(&mut app, Err(LOAD_ERROR));
    assert!(!app.ott.as_ref().unwrap().loaded);
    app
}

#[test]
fn failed_state_load_denies_navigation_and_preserves_error_after_real_back() {
    let _serial = plx_base::testlock::serial();
    let mut app = failed_app();
    let instance = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    action(&mut app, OttAction::SelectProvider("m3u".into()));
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert_eq!(app.ott.as_ref().unwrap().provider, ProviderKind::M3u);
    assert_eq!(app.ott.as_ref().unwrap().view().status, LOAD_ERROR);
    action(&mut app, OttAction::ShowProfiles);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, epoch);

    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Back, tick));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.state_load, StateLoad::Failed(LOAD_ERROR));
    assert_eq!(runtime.view().status, LOAD_ERROR, "Back cannot erase a real load failure");
    assert_eq!(app.pages.top_page(), instance);
    assert!(!runtime.loaded);
    assert_eq!(runtime.navigation_epoch, epoch);
    deliver(&mut app, Ok((Saved::default(), [7; 16])));
    action(&mut app, OttAction::SelectProvider("m3u".into()));
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    assert!(app.ott.as_ref().unwrap().navigation_epoch > epoch);
}

#[test]
fn failed_state_load_denies_profile_edit_and_submit_without_a_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = failed_app();
    action(&mut app, OttAction::ShowSettings);
    action(&mut app, OttAction::EditSetting("profile.add".into()));
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    // Even a stale or fabricated submit cannot bypass unavailable policy.
    action(&mut app, OttAction::SubmitSetting {
        session: 1,
        key: "profile.source".into(),
        value: "https://fixture.invalid/channels.m3u".into(),
    });
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.status, "This action is restricted by kiosk policy.");
    assert_eq!(runtime.view().status, LOAD_ERROR, "protected load error remains visible");
    assert_eq!(runtime.navigation_epoch, epoch);
    assert!(runtime.edit_session.is_none());
    assert!(!runtime.saving);
    assert!(runtime.saved.profiles.profiles.is_empty());
    assert!(matches!(runtime.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    deliver(&mut app, Ok((Saved::default(), [7; 16])));
    action(&mut app, OttAction::EditSetting("profile.add".into()));
    assert!(app.ott.as_ref().unwrap().edit_session.is_some());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn state_load_retry_is_single_flight_and_success_restores_the_mounted_page() {
    let _serial = plx_base::testlock::serial();
    let mut app = failed_app();
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    action(&mut app, OttAction::ShowProfiles);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, epoch);
    let mut result_sender = None;
    assert!(app.ott.as_mut().unwrap().start_state_load_with(|sender, permit| {
        assert!(permit.is_valid());
        result_sender = Some(sender);
        true
    }));
    assert!(!app.ott.as_mut().unwrap().start_state_load_with(|_, _| {
        panic!("a second loader must not be started")
    }));
    assert_eq!(app.ott.as_ref().unwrap().state_load, StateLoad::Loading);
    assert_eq!(app.ott.as_ref().unwrap().view().status, "Loading…");
    let catalog_generation = app.ott.as_ref().unwrap().generation;
    action(&mut app, OttAction::Refresh);
    assert_eq!(app.ott.as_ref().unwrap().generation, catalog_generation,
        "Refresh during loading must neither launch a second load nor refresh the catalogue");
    assert_eq!(app.ott.as_ref().unwrap().state_load, StateLoad::Loading);

    // A default/empty protected record is valid and needs no catalogue worker.
    result_sender.unwrap().send(Job::Loaded(Ok((Saved::default(), [7; 16]))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded);
    assert_eq!(runtime.state_load, StateLoad::Idle);
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.navigation_epoch, epoch);
    assert!(runtime.view().status.is_empty());
    assert!(runtime.saved.profiles.profiles.is_empty());
    assert!(!runtime.catalog_loading);
    assert!(!runtime.plex_refresh_pending);
    action(&mut app, OttAction::ShowProfiles);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    assert!(app.ott.as_ref().unwrap().navigation_epoch > epoch);
}

#[test]
fn refused_state_loader_is_a_visible_failure_and_can_be_retried() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    assert!(!runtime.start_state_load_with(|_, _| false));
    assert_eq!(runtime.state_load, StateLoad::Failed("Could not start state loader"));
    assert_eq!(runtime.view().status, "Could not start state loader");
    assert!(!runtime.loaded);
    assert!(runtime.start_state_load_with(|_, _| true));
    assert_eq!(runtime.state_load, StateLoad::Loading);
    assert_eq!(runtime.view().status, "Loading…");
}

#[test]
fn protected_load_admission_respects_loaded_saving_and_uncertain_owners() {
    let _serial = plx_base::testlock::serial();
    for (loaded, saving, uncertain) in [(true, false, false), (false, true, false), (false, false, true)] {
        let mut runtime = runtime_fixture();
        runtime.loaded = loaded;
        runtime.saving = saving;
        runtime.persistence_uncertain = uncertain;
        runtime.state_load = StateLoad::Failed(LOAD_ERROR);
        assert!(!runtime.start_state_load_with(|_, _| panic!("load guard must refuse before spawning")));
        assert_eq!(runtime.state_load, StateLoad::Failed(LOAD_ERROR));
        assert_eq!((runtime.loaded, runtime.saving, runtime.persistence_uncertain),
            (loaded, saving, uncertain));
    }
}

#[test]
fn settings_save_distinguishes_a_running_loader_from_a_failed_or_uncertain_one() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    assert!(runtime.start_state_load_with(|_, _| true));
    assert_eq!(runtime.save(Saved::default(), None, false),
        Err("Protected OTT settings are still loading"));
    assert!(!runtime.saving);
    runtime.state_load = StateLoad::Failed(LOAD_ERROR);
    assert_eq!(runtime.save(Saved::default(), None, false), Err(LOAD_ERROR));
    runtime.persistence_uncertain = true;
    assert_eq!(runtime.save(Saved::default(), None, false), Err(
        "Settings write needs reconciliation; restart the player before another change"));
    assert!(!runtime.saving);
    assert!(matches!(runtime.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn protected_state_status_reaches_companion_views_and_uncertainty_has_priority() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.status = "unrelated transient status".into();
    runtime.remote_snapshot = json!({"pairing_code": "TESTCODE"});
    for page in ["providers", "profiles", "settings", "torrent-settings"] {
        runtime.page = page;
        runtime.state_load = StateLoad::Failed(LOAD_ERROR);
        runtime.persistence_uncertain = false;
        assert_eq!(runtime.view().status, LOAD_ERROR, "{page}");
        runtime.state_load = StateLoad::Loading;
        assert_eq!(runtime.view().status, "Loading…", "{page}");
        runtime.state_load = StateLoad::Failed(LOAD_ERROR);
        runtime.persistence_uncertain = true;
        assert_eq!(runtime.protected_state_status(),
            Some("Settings write needs reconciliation; restart the player"));
        assert_eq!(runtime.view().status,
            plx_platform::i18n::msg::ott_profile_restart_in(plx_platform::i18n::current()), "{page}");
    }
}

#[test]
fn settings_write_refusal_prioritizes_state_failure_over_an_operation_in_progress() {
    let _serial = plx_base::testlock::serial();
    for (loaded, saving, uncertain, load, expected) in [
        (false, false, false, StateLoad::Idle, "Protected OTT settings are not loaded"),
        (false, true, false, StateLoad::Idle, "Protected OTT settings are not loaded"),
        (false, true, false, StateLoad::Loading, "Protected OTT settings are still loading"),
        (false, true, false, StateLoad::Failed(LOAD_ERROR), LOAD_ERROR),
        (false, true, true, StateLoad::Failed(LOAD_ERROR),
            "Settings write needs reconciliation; restart the player before another change"),
        (true, true, false, StateLoad::Idle, "Another settings operation is in progress"),
    ] {
        let mut runtime = runtime_fixture();
        runtime.loaded = loaded;
        runtime.saving = saving;
        runtime.persistence_uncertain = uncertain;
        runtime.state_load = load;
        // Both import_plex and save_with_response use this admission reason.
        assert_eq!(runtime.settings_write_refusal(), Some(expected));
        assert_eq!(runtime.save(Saved::default(), None, false), Err(expected));
        assert_eq!((runtime.loaded, runtime.saving, runtime.persistence_uncertain, runtime.state_load),
            (loaded, saving, uncertain, load), "refusal must not mutate an existing operation");
        assert!(matches!(runtime.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
    let mut ready = runtime_fixture();
    ready.loaded = true;
    assert_eq!(ready.settings_write_refusal(), None);
    // Do not execute an admitted write or Plex verifier in this no-IO fixture.
}

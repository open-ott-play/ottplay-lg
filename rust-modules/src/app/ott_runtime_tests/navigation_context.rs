//! Browse intent versus late protected-state results. The UI requests cross the
//! mounted OTT instance, dispatcher and Bridge, not handle_action directly.
//!
//! Saved(Ok) below is an injected completion-model boundary, NOT a claim that
//! storage succeeded: each writer is admitted normally with a revoked permit,
//! and its real NotApplied result is consumed before the modeled reply. The
//! revoked permit rejects before private storage IO. Catalogue/resolver workers
//! are held at their existing queue boundaries; there is no device/network IO.
use super::*;

#[path = "plex_settings_fields.rs"]
mod plex_settings_fields;

#[path = "source_editor_activation.rs"]
mod source_editor_activation;

#[path = "source_clear.rs"]
mod source_clear;

fn mounted() -> App {
    mount_app(app_fixture())
}

fn mounted_ready() -> App {
    // Initialize the empty Local inventory and verified policy before holding
    // worker slots. These tests deliver catalogue events, not worker bodies.
    mount_app(ready_app_fixture())
}

fn mount_app(mut app: App) -> App {
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.write_permit.revoke();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(!app.pages.has_pending_navigation());
    app
}

fn fixture_saved() -> Saved {
    let mut saved = Saved::default();
    saved.profiles.profiles = vec![
        ProviderProfile {
            id: "local".into(),
            name: "Local fixture".into(),
            kind: ProviderKind::Local,
            // HTTP Server library keeps the Local provider/navigation identity.
            // These epoch tests do not exercise or fabricate a disk RootFD grant.
            source: "https://media.invalid/library".into(),
            source_alias: String::new(),
            enabled: true,
        },
        ProviderProfile {
            id: "m3u".into(),
            name: "M3U fixture".into(),
            kind: ProviderKind::M3u,
            source: "https://media.invalid/list.m3u".into(),
            source_alias: String::new(),
            enabled: true,
        },
    ];
    saved.profiles.active_profile = Some("local".into());
    saved
        .settings
        .insert("selectedProvider".into(), json!("local"));
    saved.validate().expect("valid synthetic profiles");
    saved
}

fn seed_saved(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved = fixture_saved();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.sync_saved_context();
}

fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(
        MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn deliver(app: &mut App, job: Job) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(job)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

fn browse(app: &App) -> (&'static str, ProviderKind) {
    let runtime = app.ott.as_ref().unwrap();
    (runtime.page, runtime.provider)
}

/// Preserve the real admitted operation's reply/reload metadata. Consuming its
/// refusal here keeps it out of the next UI tick, so that tick really races a
/// still-pending save, rather than accidentally completing it first.
fn capture_refused_write(app: &mut App) -> (Option<Invocation>, bool, Value) {
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.saving, "premise: a real writer was admitted");
    match runtime
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("revoked writer reply")
    {
        Job::Saved(Err("Could not prepare protected OTT settings for saving"), invocation, reload, response) => {
            (invocation, reload, response)
        }
        _ => panic!("expected the actual pre-IO revoked-permit refusal"),
    }
}

fn modeled_saved(app: &mut App, saved: Saved, pending: (Option<Invocation>, bool, Value)) {
    let (invocation, reload, response) = pending;
    deliver(app, Job::Saved(Ok(saved), invocation, reload, response));
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn initial_loaded_without_navigation_restores_saved_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(browse(&app), ("channels", ProviderKind::Local));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded);
    assert_eq!(
        runtime.catalog_next.as_ref().map(|p| p.id.as_str()),
        Some("local")
    );
}

#[test]
fn late_loaded_preserves_newer_provider_profiles_page() {
    let _serial = plx_base::testlock::serial();
    // This is a late completion race after verified empty state, not authority
    // to navigate while the initial protected-state result is unavailable.
    let mut app = mounted_ready();
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
    assert!(app.ott.as_ref().unwrap().loaded, "data still lands");
}

#[test]
fn late_loaded_cannot_reclaim_browse_after_navigation_aba() {
    let _serial = plx_base::testlock::serial();
    // This is a late completion race after verified empty state, not authority
    // to navigate while the initial protected-state result is unavailable.
    let mut app = mounted_ready();
    let initial = browse(&app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    ui(&mut app, OttAction::ShowProviders);
    assert_eq!(
        browse(&app),
        initial,
        "premise: same visible pair, newer intent"
    );
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(
        browse(&app),
        initial,
        "pair equality must not revive old authority"
    );
}

#[test]
fn preview_does_not_retire_initial_profile_restoration() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    ui(&mut app, OttAction::Preview("not-loaded-yet".into()));
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(browse(&app), ("channels", ProviderKind::Local));
}

#[test]
fn rejected_action_without_navigation_keeps_initial_restoration() {
    let _serial = plx_base::testlock::serial();
    // This is a late completion race after verified empty state, not authority
    // to navigate while the initial protected-state result is unavailable.
    let mut app = mounted_ready();
    ui(
        &mut app,
        OttAction::SelectProvider("invalid-provider".into()),
    );
    assert_eq!(app.ott.as_ref().unwrap().status, "Unknown provider");
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(browse(&app), ("channels", ProviderKind::Local));
}

#[test]
fn unrelated_saved_setting_preserves_browsed_provider_without_newer_navigation() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.settings.insert("fixtureSetting".into(), json!(1));
    app.ott
        .as_mut()
        .unwrap()
        .save(saved.clone(), None, false)
        .unwrap();
    let pending = capture_refused_write(&mut app);
    modeled_saved(&mut app, saved, pending);
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
    assert_eq!(
        app.ott.as_ref().unwrap().saved.settings["fixtureSetting"],
        json!(1)
    );
}

#[test]
fn accepted_select_profile_still_activates_current_intent() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    assert_eq!(browse(&app), ("channels", ProviderKind::M3u));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .catalog_next
            .as_ref()
            .map(|p| p.id.as_str()),
        Some("m3u")
    );
}

#[test]
fn newer_navigation_blocks_old_select_profile_browse_effect() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::SelectProvider("local".into()));
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    assert_eq!(browse(&app), ("profiles", ProviderKind::Local));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .saved
            .profiles
            .active_profile
            .as_deref(),
        Some("m3u")
    );
}

#[test]
fn remote_provider_reselection_without_profiles_keeps_explicit_authority() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime
            .saved
            .settings
            .insert("selectedProvider".into(), json!("m3u"));
    }
    ui(&mut app, OttAction::SelectProvider("local".into()));
    let mut runtime = app.ott.take().unwrap();
    let mut request = runtime
        .remote
        .request_for_test("provider", json!({"query":"m3u"}));
    request.deadline_ms = runtime.now_ms() + 10_000;
    assert!(commands::command(&mut runtime, &mut app, &request).is_none());
    let saved = runtime.saved.clone();
    app.ott = Some(runtime);
    let pending = capture_refused_write(&mut app);
    assert!(!pending.1, "no profile means no catalogue reload");
    modeled_saved(&mut app, saved, pending);
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
}

#[test]
fn uncertain_visible_image_cannot_replace_browsed_provider() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    let mut saved = fixture_saved();
    saved.settings.insert("fixtureSetting".into(), json!(1));
    app.ott
        .as_mut()
        .unwrap()
        .save(saved.clone(), None, false)
        .unwrap();
    let _pending = capture_refused_write(&mut app);
    deliver(&mut app, Job::Uncertain(Some(saved), None));
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.loaded && !runtime.saving && runtime.persistence_uncertain);
}

#[test]
fn background_catalog_refresh_preserves_provider_and_page() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    app.ott
        .as_mut()
        .unwrap()
        .refresh_catalog_preserving_playback();
    assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .catalog_next
            .as_ref()
            .map(|p| p.id.as_str()),
        Some("local")
    );
}

#[test]
fn partially_failed_plex_navigation_still_fences_late_loaded() {
    let _serial = plx_base::testlock::serial();
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            plx_plex::plex::reset_servers_for_test();
        }
    }
    let _cleanup = Cleanup;
    plx_plex::plex::reset_servers_for_test();
    // This is a late completion race after verified empty state, not authority
    // to navigate while the initial protected-state result is unavailable.
    let mut app = mounted_ready();
    let sid = plx_plex::plex::register_for_test(
        "nav-fixture-machine",
        "127.0.0.1",
        1,
        "fixture-token",
        "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    // A real writer is admitted, then its pre-IO revoked-permit refusal is
    // held so recovery cannot admit a competing settings write.
    let saved = app.ott.as_ref().unwrap().saved.clone();
    app.ott.as_mut().unwrap().save(saved, None, false).unwrap();
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::SelectProvider("plex".into()));
    assert_eq!(browse(&app), ("profiles", ProviderKind::Plex));
    assert_eq!(
        app.ott.as_ref().unwrap().status,
        "Another settings operation is in progress"
    );
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    assert_eq!(browse(&app), ("profiles", ProviderKind::Plex));
    let (invocation, reload, response) = pending;
    deliver(&mut app, Job::Saved(Err("Could not prepare protected OTT settings for saving"),
        invocation, reload, response));
    assert!(!app.ott.as_ref().unwrap().saving);
}

fn entry(id: &str) -> CatalogEntry {
    CatalogEntry { protection: None,
        synopsis: None,
        id: id.into(),
        title: format!("{id}.mp4"),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::ServerLibrary {
            source_id: "fixture-source".into(),
            item_id: id.into(),
            relative_path: Some(format!("{id}.mp4")),
            root_alias: Some("Navigation fixture".into()),
        },
        duration_ms: Some(60_000),
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}

fn first_batch(app: &mut App, id: &str) {
    let runtime = app.ott.as_ref().unwrap();
    let mut row = entry(id);
    if runtime
        .active()
        .is_some_and(|profile| profile.kind == ProviderKind::M3u)
    {
        row.media = MediaLocator::Http {
            url: format!("https://media.invalid/{id}.mp4"),
        };
    }
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog { hierarchy: None, entries: vec![row] },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn late_loaded_batch_cannot_start_behind_newer_settings() {
    let _serial = plx_base::testlock::serial();
    // This is a late completion race after verified empty state, not authority
    // to navigate while the initial protected-state result is unavailable.
    let mut app = mounted_ready();
    ui(&mut app, OttAction::ShowSettings);
    deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
    first_batch(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.catalog.entries.len(),
        1,
        "catalogue data still arrives"
    );
    assert!(
        runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "late startup state must not activate a source"
    );
    assert_eq!(browse(&app), ("settings", ProviderKind::M3u));
}

#[test]
fn late_saved_batch_cannot_start_behind_newer_settings() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::ShowSettings);
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    first_batch(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries.len(), 1);
    assert!(
        runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "durable stale selection must not create fresh playback intent"
    );
    assert_eq!(browse(&app), ("settings", ProviderKind::Local));
}

#[test]
fn current_profile_completion_still_starts_first_batch() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().save_ui_owner, app.pages.top_page());
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    assert!(app.ott.as_ref().unwrap().save_ui_owner.is_none());
    first_batch(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_target.as_ref().map(|request| request.id.as_str()),
        Some("first")
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .map(|request| request.profile.id.as_str()),
        Some("m3u")
    );
    assert_eq!(browse(&app), ("channels", ProviderKind::M3u));
}

#[test]
fn ui_saved_after_outer_navigation_cannot_start_its_first_batch() {
    let _serial = plx_base::testlock::serial();
    for departure in ["left", "replaced", "pending"] {
        let mut app = mounted_ready();
        seed_saved(&mut app);
        ui(&mut app, OttAction::SelectProvider("m3u".into()));
        ui(&mut app, OttAction::SelectProfile("m3u".into()));
        let pending = capture_refused_write(&mut app);
        let old_owner = app.pages.top_page().unwrap();
        assert_eq!(app.ott.as_ref().unwrap().save_ui_owner, Some(old_owner));
        let navigation_epoch = app.ott.as_ref().unwrap().navigation_epoch;
        let playback_epoch = app.ott.as_ref().unwrap().playback_epoch;

        // Queue or complete an outer navigation without ticking the runtime:
        // the Saved reply must be guarded before its later owner reconciliation.
        bridge::nav_root(&mut app.pages, AppArg::Home);
        if departure == "pending" {
            assert!(app.pages.has_pending_navigation());
            assert_eq!(app.pages.top_page(), Some(old_owner));
        } else {
            settle_pages(&mut app);
            assert!(matches!(app.route(), AppArg::Home));
            if departure == "replaced" {
                bridge::nav_root(&mut app.pages, AppArg::Ott);
                settle_pages(&mut app);
                assert!(matches!(app.route(), AppArg::Ott));
                assert_ne!(app.pages.top_page(), Some(old_owner));
            }
        }
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation_epoch);
        assert_eq!(app.ott.as_ref().unwrap().playback_epoch, playback_epoch);
        assert_eq!(app.ott.as_ref().unwrap().reply, Some(old_owner));

        let mut saved = fixture_saved();
        saved.profiles.active_profile = Some("m3u".into());
        modeled_saved(&mut app, saved, pending);
        first_batch(&mut app, "first");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("m3u"));
        assert_eq!(runtime.catalog.entries.len(), 1, "{departure}: data still lands");
        assert!(runtime.save_ui_owner.is_none());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
            "{departure}: the retired/departing UI cannot activate a saved source");
    }
}

#[test]
fn remote_profile_saved_outside_ott_still_starts_its_first_batch() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    let previous_owner = app.pages.top_page();
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    assert!(matches!(app.route(), AppArg::Home));
    // Deliberately leave reply reconciliation to the ordinary tick: remote
    // admission must not borrow the previous UI address even while it is held.
    assert_eq!(app.ott.as_ref().unwrap().reply, previous_owner);
    let mut runtime = app.ott.take().unwrap();
    let mut request = runtime.remote.request_for_test("profile", json!({"number":1}));
    request.deadline_ms = runtime.now_ms() + 10_000;
    assert!(commands::command(&mut runtime, &mut app, &request).is_none());
    assert!(runtime.save_ui_owner.is_none(), "remote admission has no UI owner");
    app.ott = Some(runtime);
    let pending = capture_refused_write(&mut app);
    assert!(pending.0.is_some() && pending.1, "real remote selection requested reload");
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    first_batch(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.save_ui_owner.is_none());
    assert_eq!(runtime.play_target.as_ref().map(|request| request.id.as_str()), Some("first"));
    assert_eq!(runtime.resolver_next.as_ref().map(|request| request.profile.id.as_str()),
        Some("m3u"));
    assert!(matches!(app.route(), AppArg::Home), "no decoder was started by this fixture");
}

#[test]
fn passive_saved_refresh_retains_newer_queued_selection() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    app.ott.as_mut().unwrap().catalog.entries = vec![entry("chosen")];
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::Play("chosen".into()));
    let generation = app.ott.as_ref().unwrap().play_generation;
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    first_batch(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(
        runtime.play_target.as_ref().map(|request| request.id.as_str()),
        Some("chosen")
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .map(|request| request.profile.id.as_str()),
        Some("local")
    );
    assert!(
        !runtime.play_cancel.load(Ordering::Acquire),
        "the newer explicit selection must retain its original source"
    );
}

#[test]
fn stale_changed_profile_cannot_rebind_old_restore_to_matching_new_id() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.continuity = continuity::Continuity::new(
            runtime.generation,
            Some(continuity::ResumePoint {
                entry_id: "same-id".into(),
                position_ms: 42_000,
            }),
        );
    }
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::ShowSettings);
    assert!(
        app.ott
            .as_ref()
            .unwrap()
            .continuity
            .pending_restore()
            .is_some(),
        "old profile's restore is pending before completion"
    );
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    first_batch(&mut app, "same-id");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.continuity.pending_restore().is_none());
    assert!(
        runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "an old-profile ID must not activate the stale replacement profile"
    );
    assert_eq!(browse(&app), ("settings", ProviderKind::Local));
}

#[test]
fn stale_same_profile_reload_preserves_bound_restore() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.continuity = continuity::Continuity::new(
            runtime.generation,
            Some(continuity::ResumePoint {
                entry_id: "same-profile".into(),
                position_ms: 42_000,
            }),
        );
    }
    ui(&mut app, OttAction::SelectProfile("local".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::ShowSettings);
    modeled_saved(&mut app, fixture_saved(), pending);
    first_batch(&mut app, "same-profile");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime
            .play_target
            .as_ref()
            .map(|request| (request.id.as_str(), request.position_ms)),
        Some(("same-profile", 42_000))
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .map(|request| request.profile.id.as_str()),
        Some("local")
    );
    assert_eq!(browse(&app), ("settings", ProviderKind::Local));
}

#[test]
fn stale_changed_profile_retires_unbound_parked_resume_before_wake() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProfile("m3u".into()));
    let pending = capture_refused_write(&mut app);
    ui(&mut app, OttAction::ShowSettings);
    // Model the parked id-only queue, not a decoder or lifecycle transition.
    // Suspension prevents tick from consuming it before we inspect retirement.
    app.ott_standby = true;
    app.ott.as_mut().unwrap().resume_pending = Some(PlaybackRequest::ordinary("same-id", 42_000));
    let mut saved = fixture_saved();
    saved.profiles.active_profile = Some("m3u".into());
    modeled_saved(&mut app, saved, pending);
    assert!(
        app.ott.as_ref().unwrap().resume_pending.is_none(),
        "an id-only resume cannot retain its meaning after changing catalogue source"
    );
    app.ott_standby = false;
    first_batch(&mut app, "same-id");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

// Append to navigation_context.rs; reuses its existing mounted/seed_saved/entry,
// capture_refused_write and deliver helpers. No module registration is needed.
#[test]
fn plex_verified_save_hop_retains_pre_remote_play_epoch() {
    let _serial = plx_base::testlock::serial();
    use crate::app::ott_import;
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = ott_import::set_authorized_sources(&[]);
            plx_plex::plex::reset_servers_for_test();
            plx_plex::plex::grant::reset_for_test();
        }
    }
    let _cleanup = Cleanup;
    plx_plex::plex::reset_servers_for_test();
    plx_plex::plex::grant::reset_for_test();
    ott_import::set_authorized_sources(&[]).unwrap();
    let mut app = mounted_ready();
    seed_saved(&mut app);
    let (mut import_request, admitted_epoch, navigation_epoch) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries = vec![entry("chosen")];
        // Model the pending verifier's already-captured admission metadata with
        // the existing real activating writer, without launching an HTTP probe.
        runtime.save(runtime.saved.clone(), None, true).unwrap();
        (
            runtime
                .remote
                .request_for_test("provider_settings", json!({"provider":"plex",
                    "settings":{"server":"http://127.0.0.1:1","token":"fixture-token"}})),
            runtime.save_playback_epoch,
            runtime.navigation_epoch,
        )
    };
    let _first_stage_refusal = capture_refused_write(&mut app);
    import_request.deadline_ms = app.ott.as_ref().unwrap().now_ms() + 10_000;
    let mut play_request = import_request.clone();
    // Distinct valid request identity; this test invokes the command adapter
    // directly, not a second transport poll or response acknowledgement.
    play_request.id = "1123456789abcdef0123456789abcdef".into();
    play_request.action = "play".into();
    play_request.params = json!({"query":1});
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, &mut app, &play_request).is_none());
    assert_ne!(runtime.playback_epoch, admitted_epoch);
    assert_eq!(
        runtime.navigation_epoch, navigation_epoch,
        "remote play changes playback intent without UI navigation"
    );
    let accepted_generation = runtime.play_generation;
    app.ott = Some(runtime);

    let source = plx_plex::plex::session::SourceRef {
        machine_id: "navigation-verifier-fixture".into(),
        name: "Plex fixture".into(),
        address: "127.0.0.1".into(),
        port: 1,
        origin_url: "http://127.0.0.1:1".into(),
        token: "fixture-token".into(),
        ..Default::default()
    };
    let verified = ott_import::verified_source_for_test(source);
    // Exercise the real success handler and its save_reply admission. The
    // resulting write is refused before IO by the existing revoked permit.
    deliver(&mut app, Job::PlexVerified(import_request, Ok(verified)));
    // The second-stage worker can finish in the same tick or the next. Both
    // schedules are valid: drain the real refusal only if it remains pending.
    if app.ott.as_ref().unwrap().saving {
        let (invocation, reload, response) = capture_refused_write(&mut app);
        deliver(
            &mut app,
            Job::Saved(
                Err("Could not prepare protected OTT settings for saving"),
                invocation,
                reload,
                response,
            ),
        );
    }
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert_eq!(
        runtime.status, "Could not prepare protected OTT settings for saving",
        "second-stage save was admitted and settled at the real pre-IO boundary"
    );
    assert_eq!(
        runtime.save_playback_epoch, admitted_epoch,
        "the verifier must not borrow the newer remote play epoch at save_reply"
    );
    assert_ne!(runtime.save_playback_epoch, runtime.playback_epoch);
    assert_eq!(runtime.play_generation, accepted_generation);
    assert_eq!(
        runtime.play_target.as_ref().map(|request| request.id.as_str()),
        Some("chosen")
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .map(|request| request.profile.id.as_str()),
        Some("local")
    );
}

#[test]
fn unavailable_policy_denies_navigation_without_minting_browse_authority() {
    let _serial = plx_base::testlock::serial();
    for actions in [
        vec![OttAction::SelectProvider("m3u".into())],
        vec![OttAction::SelectProvider("m3u".into()), OttAction::ShowProviders],
        vec![OttAction::SelectProvider("invalid-provider".into())],
        vec![OttAction::SelectProvider("plex".into())],
        vec![OttAction::ShowSettings],
    ] {
        let mut app = mounted();
        let original = browse(&app);
        let epoch = app.ott.as_ref().unwrap().navigation_epoch;
        for action in actions {
            ui(&mut app, action);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(browse(&app), original);
            assert_eq!(runtime.navigation_epoch, epoch);
            assert_eq!(runtime.status, "This action is restricted by kiosk policy.");
            assert!(!runtime.loaded && !runtime.saving);
            assert!(runtime.edit_session.is_none());
            assert!(runtime.catalog_next.is_none() && runtime.resolver_next.is_none());
        }
        deliver(&mut app, Job::Loaded(Ok((fixture_saved(), [1; 16]))));
        assert_eq!(browse(&app), ("channels", ProviderKind::Local));
        ui(&mut app, OttAction::SelectProvider("m3u".into()));
        assert_eq!(browse(&app), ("profiles", ProviderKind::M3u));
        assert!(app.ott.as_ref().unwrap().navigation_epoch > epoch);
    }
}

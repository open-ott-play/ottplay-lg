//! Source-only regression inputs. Held existing worker slots prevent network IO.
use super::*;

struct Registry;
impl Registry {
    fn new() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}
fn server() -> plx_plex::plex::ServerId {
    let origin = plx_plex::plex::Origin::parse("https://classic-fixture.invalid:443").unwrap();
    let id = plx_plex::plex::register_pinned_with_client_id(
        "classic-fixture-machine", &origin, "synthetic-token", None,
        "synthetic-client", Default::default());
    assert!(plx_plex::plex::set_current(id));
    id
}
fn session(app: &mut App, epoch: u64) {
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = epoch;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = bridge::Bridge::for_session_test(init);
    assert!(app.bridge.session_ready_current());
}
fn fixture() -> (App, Box<Runtime>) {
    let mut app = run::lifecycle_regression_tests::app();
    session(&mut app, 10);
    let mut runtime = Box::new(runtime_fixture());
    // Match the frame's clock boundary without ticking workers before their holds are set.
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .expect("fixture clock");
    runtime.clock_anchor = Some(sample);
    runtime.wall_clock_ms = sample.unix_ms();
    runtime.clock_sample = Some(sample.steady_from(sample));
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.resume_source_work();
    (app, runtime)
}
#[test]
fn classic_source_is_temporary_and_never_arms_autoplay() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let ticket = plx_media::route::play_request_ticket();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let profile = runtime.effective_profile().unwrap();
    assert_eq!(profile.kind, ProviderKind::Plex);
    assert_eq!(profile.source, "classic-fixture-machine");
    let id = profile.id.clone();
    assert!(classic_source::temporary_id(&runtime, &id));
    assert!(runtime.resume_key().is_none());
    let generation = runtime.generation;
    assert!(runtime.catalog_next.is_some(), "held slot keeps the real catalog request staged");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.resume_pending.is_none());
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    assert_eq!(runtime.effective_profile().unwrap().id, id);
    assert_eq!(runtime.generation, generation, "repaint must not restart the catalog");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(plx_media::route::play_request_ticket(), ticket);
    assert!(!runtime.saving);
    runtime.catalog_done.store(true, Ordering::Release);
    assert!(matches!(runtime.take_catalog_task_checked(Some(&app)),
        Some(ott_catalog::Task::ClassicPlex { .. })));
}

#[test]
fn classic_source_epoch_change_rejects_old_profile_and_catalog_reply() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let old = runtime.effective_profile().unwrap().clone();
    let generation = runtime.generation;
    session(&mut app, 11);
    assert!(classic_source::guard_profile(&runtime, &app, &old).is_err());
    assert!(classic_source::guard_catalog_event(&runtime, &app, generation).is_err());
    classic_source::reconcile(&mut runtime, &app);
    assert!(runtime.effective_profile().is_none());
    assert!(runtime.catalog_next.is_none());
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    assert_ne!(runtime.effective_profile().unwrap().id, old.id);
    assert!(classic_source::guard_profile(&runtime, &app, &old).is_err());
}
#[test]
fn classic_source_reuses_saved_plex_identity_but_reloads_unbound_rows() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    let profile = ProviderProfile { id: "kept".into(), name: "Kept".into(),
        kind: ProviderKind::Plex, source: "classic-fixture-machine".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.profiles.push(profile.clone());
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    let generation = runtime.generation;
    runtime.saved.category = "Library A".into();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    runtime.selected = Some("kept:42".into());
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    assert_eq!(runtime.effective_profile(), Some(&profile));
    assert!(classic_source::capture_read(&runtime, &app, &profile).unwrap().is_some());
    assert_eq!(runtime.resume_key().as_deref(), Some("kept"));
    assert!(!classic_source::temporary_id(&runtime, &profile.id));
    assert!(classic_source::binding_profile(&runtime).is_none(),
        "a durable source must not be duplicated in Favorite bindings");
    let mut row = synthetic_entry();
    row.id = format!("{}:42", profile.id);
    row.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: "42".into() };
    let bindings = runtime.selection_bindings().unwrap();
    let favorite = selection::capture_bound(&bindings,
        selection::SelectionOwner::Profile(&profile), &row).unwrap();
    assert_eq!(favorite.origin.profile, "Plex");
    assert_eq!(favorite.origin.source_alias, "classic-fixture-machine");
    assert_eq!(runtime.catalog_next.as_ref(), Some(&profile));
    assert_ne!(runtime.generation, generation);
    assert_eq!(runtime.selected.as_deref(), Some("kept:42"));
    assert_eq!(runtime.view_selection().0, "Library A");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    runtime.loaded = false;
    assert!(!classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    runtime.loaded = true;
    runtime.persistence_uncertain = true;
    assert!(!classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
}
#[test]
fn classic_source_category_is_local_but_favorites_remains_global() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    runtime.saved.category = "saved-other-provider".into();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    classic_source::set_view(&mut runtime, "Library A");
    assert_eq!(runtime.view_selection(), ("Library A", false));
    let candidate = runtime.category_choice_candidate(Some("Library B")).unwrap();
    assert_eq!(candidate.category, "saved-other-provider");
    assert_eq!(candidate.profiles, runtime.saved.profiles);
    let favorites = runtime.category_choice_candidate(None).unwrap();
    assert!(favorites.favorites_selected);
    runtime.saved.favorites_selected = true;
    assert_eq!(runtime.view_selection(), ("Library A", true));
}

#[test]
fn classic_source_never_substitutes_an_unrelated_saved_provider() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    let profile = ProviderProfile { id: "saved-m3u".into(), name: "Saved M3U".into(),
        kind: ProviderKind::M3u, source: "https://fixture.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.profiles.push(profile.clone());
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    assert_eq!(runtime.effective_profile().unwrap().kind, ProviderKind::Plex);
    assert_eq!(runtime.effective_profile().unwrap().source, "classic-fixture-machine");
    assert_eq!(runtime.active(), Some(&profile));
    runtime.sync_saved_context();
    assert_eq!(runtime.provider, ProviderKind::Plex);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.resume_key().is_none());
}

#[test]
fn classic_source_queued_batch_and_finished_cannot_cross_epoch_or_network_scope() {
    let _serial = plx_base::testlock::serial();
    for (network_change, durable) in [(false, false), (true, false), (false, true), (true, true)] {
        let _registry = Registry::new();
        server();
        let (mut app, mut runtime) = fixture();
        if durable {
            runtime.saved.profiles.profiles.push(ProviderProfile {
                id: "durable-plex".into(), name: "Saved Plex".into(),
                kind: ProviderKind::Plex, source: "classic-fixture-machine".into(),
                source_alias: String::new(), enabled: true,
            });
            runtime.saved.profiles.active_profile = Some("durable-plex".into());
        }
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        let profile = runtime.effective_profile().unwrap().clone();
        let generation = runtime.generation;
        let mut entry = synthetic_entry();
        entry.id = format!("{}:42", profile.id);
        entry.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: "42".into() };
        runtime.catalog_tx.send(CatalogEvent::Batch(generation,
            Catalog { entries: vec![entry], hierarchy: None })).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(generation, Ok(()))).unwrap();
        // Keep the existing writer/worker slots held: this tests addressed
        // consumption through the real tick, not network or checkpoint IO.
        runtime.saving = true;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        app.ott = Some(runtime);
        if network_change { plx_plex::plex::grant::network_changed(); }
        else { session(&mut app, 11); }
        assert!(classic_source::guard_profile(app.ott.as_ref().unwrap(), &app, &profile).is_err());
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog.entries.is_empty());
        assert!(!runtime.catalog_complete);
        assert!(runtime.generation != generation);
        assert!(runtime.catalog_rx.try_recv().is_err());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        if !durable { assert!(classic_source::guard_profile(runtime, &app, &profile).is_err()); }
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    }
}

fn retained_catalog(durable: bool) -> (App, Box<Runtime>, ProviderProfile) {
    let (mut app, mut runtime) = fixture();
    let saved = ProviderProfile { id: "unrelated-m3u".into(), name: "Other".into(),
        kind: ProviderKind::M3u, source: "https://fixture.invalid/other.m3u".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.active_profile = Some(saved.id.clone());
    runtime.saved.profiles.profiles.push(saved);
    if durable {
        runtime.saved.profiles.profiles.push(ProviderProfile { id: "saved-plex".into(),
            name: "Plex".into(), kind: ProviderKind::Plex,
            source: "classic-fixture-machine".into(), source_alias: String::new(), enabled: true });
    }
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let profile = runtime.effective_profile().unwrap().clone();
    let mut row = synthetic_entry();
    row.id = format!("{}:42", profile.id);
    row.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: "42".into() };
    runtime.catalog.entries = vec![row];
    runtime.catalog_next = None;
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.selected = Some(format!("{}:42", profile.id));
    classic_source::set_view(&mut runtime, "Library A");
    (app, runtime, profile)
}

#[test]
fn classic_source_suspend_resume_keeps_temporary_and_durable_catalog_owner() {
    let _serial = plx_base::testlock::serial();
    for durable in [false, true] {
        let _registry = Registry::new();
        server();
        let (mut app, mut runtime, profile) = retained_catalog(durable);
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let selected = runtime.selected.clone();
        let generation = runtime.generation;
        let cancel = runtime.cancel.clone();
        runtime.suspend_source_work(&mut app);
        assert!(!runtime.source_work_allowed());
        assert!(cancel.load(Ordering::Acquire));
        assert_ne!(runtime.generation, generation);
        assert_eq!(runtime.effective_profile(), Some(&profile));
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.selected, selected);
        assert_eq!(runtime.view_selection().0, "Library A");
        classic_source::reconcile(&mut runtime, &app);
        runtime.resume_source_work();
        runtime.rearm_source_work(&mut app);
        assert!(!runtime.catalog_work_allowed(), "old worker still owns its slot");
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        assert!(runtime.catalog_work_allowed());
        assert_eq!(runtime.catalog_next.as_ref(), Some(&profile));
        assert!(matches!(runtime.take_catalog_task_checked(Some(&app)),
            Some(ott_catalog::Task::ClassicPlex { .. })));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.play_target.is_none());
    }
}

#[test]
fn classic_source_changed_while_suspended_cannot_rearm_plex_as_saved_m3u() {
    let _serial = plx_base::testlock::serial();
    for (durable, network) in [(false, false), (false, true), (true, false), (true, true)] {
        let _registry = Registry::new();
        server();
        let (mut app, mut runtime, profile) = retained_catalog(durable);
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        runtime.suspend_source_work(&mut app);
        if network { plx_plex::plex::grant::network_changed(); }
        else { session(&mut app, 11); }
        // The wake entry point must validate itself, even before the next tick.
        runtime.resume_source_work();
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        assert!(runtime.catalog.entries.is_empty());
        assert!(runtime.catalog_next.is_none());
        assert!(!runtime.catalog_complete);
        assert!(runtime.effective_profile() != Some(&profile));
        assert_eq!(runtime.effective_profile().unwrap().kind, ProviderKind::M3u);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.play_target.is_none());
    }
}

#[test]
fn classic_source_interrupted_scan_restoration_keeps_exact_owner() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (app, mut runtime, profile) = retained_catalog(false);
    let generation = runtime.generation;
    let catalog = runtime.catalog.clone();
    let interrupted = transient::InterruptedScan { profile: Some(profile.clone()), owner: None, restart: true };
    transient::restore_interrupted_scan(&mut runtime, &interrupted);
    assert_ne!(runtime.generation, generation);
    assert_eq!(runtime.effective_profile(), Some(&profile));
    assert_eq!(runtime.catalog_next.as_ref(), Some(&profile));
    assert_eq!(runtime.catalog, catalog);
    assert!(classic_source::guard_profile(&runtime, &app, &profile).is_ok());
    runtime.catalog_done.store(true, Ordering::Release);
    assert!(matches!(runtime.take_catalog_task_checked(Some(&app)),
        Some(ott_catalog::Task::ClassicPlex { .. })));
}


#[test]
fn classic_source_back_after_pending_local_cancel_keeps_exact_owner() {
    let _serial = plx_base::testlock::serial();
    for (durable, op) in [(false, "play"), (false, "shuffle"), (true, "play"), (true, "shuffle")] {
        let _registry = Registry::new();
        server();
        let (mut app, mut runtime, profile) = retained_catalog(durable);
        runtime.page = "channels";
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let selected = runtime.selected.clone();
        let now = runtime.now_ms();
        let body = json!({"commands":[], "server_time":1000, "requests":[{
            "id":"00000000000000000000000000000001", "action":"local_media",
            "params":{"op":op, "runtime":runtime.remote.runtime(), "source":"Fixture folder"},
            "expires_at":1020
        }]}).to_string();
        let mut requests = runtime.remote.raw_poll_for_test(&body, now);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert!(runtime.remote.invocation_is_current(&request));
        // Use real command admission but hold its shared catalogue worker slot.
        // No folder is opened and no filesystem or media worker is launched.
        assert!(commands::command(&mut runtime, &mut app, &request).is_none());
        assert!(!runtime.catalog_done.load(Ordering::Acquire));
        let candidate = runtime.transient.local_candidate().expect("pending Local playback");
        assert_eq!(candidate.request.op.name(), op);
        assert_eq!(candidate.interrupted.profile.as_ref(), Some(&profile));
        assert!(candidate.interrupted.restart);
        assert_eq!(runtime.effective_profile(), Some(&profile));
        let pending_generation = runtime.generation;

        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();

        assert!(runtime.transient.candidate.is_none());
        // Local cancellation restores the interrupted scan, then Back retires
        // that scan. The binding must follow both distinct generation changes.
        assert_eq!(runtime.generation, pending_generation.wrapping_add(2));
        assert_eq!(runtime.effective_profile(), Some(&profile));
        assert!(classic_source::guard_profile(&runtime, &app, &profile).is_ok());
        classic_source::reconcile(&mut runtime, &app);
        assert_eq!(runtime.effective_profile(), Some(&profile));
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.selected, selected);
        assert_eq!(runtime.view_selection().0, "Library A");
        assert_eq!(runtime.page, "profiles");
        assert!(runtime.catalog_next.is_none());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving);
        let response = runtime.remote.response_for_id_for_test(&request.id).unwrap();
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"]["error"], "runtime_changed");
    }
}

// These roundtrips exercise the normal candidate/final-snapshot merge without
// invoking a protected writer. Assigning the candidate models its acknowledged
// publication, not disk durability. All source worker slots remain held.
fn saved_category_profile() -> ProviderProfile {
    ProviderProfile { id: "category-plex".into(), name: "Saved Plex".into(),
        kind: ProviderKind::Plex, source: "classic-fixture-machine".into(),
        source_alias: String::new(), enabled: true }
}
fn category_rows(runtime: &mut Runtime, profile: &ProviderProfile) -> (String, String, String) {
    let rows: Vec<_> = ["42", "43"].into_iter().map(|key| {
        let mut row = synthetic_entry();
        row.id = format!("{}:{key}", profile.id);
        row.title = format!("Film {key}");
        row.category = "Library B".into();
        row.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: key.into() };
        row
    }).collect();
    let first = rows[0].id.clone(); let second = rows[1].id.clone();
    runtime.catalog = Catalog { entries: rows, hierarchy: None };
    runtime.catalog_complete = true; runtime.catalog_loading = false;
    runtime.catalog_revision += 1; runtime.page = "channels";
    let category = runtime.organization_categories().into_iter()
        .find(|row| row.title == "Library B").unwrap().id;
    (category, first, second)
}

#[test]
fn classic_saved_category_and_focus_roundtrip_preserve_active_provider() {
    let _serial = plx_base::testlock::serial();
    for active in [true, false] {
        let _registry = Registry::new(); server();
        let (mut app, mut runtime) = fixture();
        let profile = saved_category_profile();
        let other = ProviderProfile { id: "unrelated-m3u".into(), name: "Other".into(),
            kind: ProviderKind::M3u, source: "https://fixture.invalid/other.m3u".into(),
            source_alias: String::new(), enabled: true };
        runtime.saved.profiles.profiles = vec![profile.clone(), other.clone()];
        runtime.saved.profiles.active_profile = Some(if active { profile.id.clone() } else { other.id.clone() });
        runtime.saved.category = "Legacy category".into();
        runtime.saved.resume.insert(profile.id.clone(), SavedResume { entry_id: "unchanged".into(),
            position_ms: 12_500, source_fingerprint: profile_fingerprint(&profile) });
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        assert_eq!(runtime.view_selection().0, if active { "Legacy category" } else { "" });
        let (category, first, second) = category_rows(&mut runtime, &profile);
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let candidate = runtime.category_choice_candidate(Some(&category)).unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before, "no premature publication");
        assert_eq!(candidate.profiles, runtime.saved.profiles);
        assert_eq!(candidate.category, if active { category.as_str() } else { "Legacy category" });
        let fingerprint = profile_fingerprint(&profile);
        assert_eq!(category_preferences::Preferences::read(&candidate.settings).unwrap()
            .remembered(category_preferences::Source { profile_id: &profile.id, fingerprint: &fingerprint }).unwrap(),
            Some(category.as_str()));
        runtime.saved = candidate;
        classic_source::set_view(&mut runtime, &category);
        runtime.selected = Some(first.clone());
        let context = runtime.view().preview_context.unwrap();
        handle_action(&mut runtime, &mut app, OttAction::FocusAt { context, id: second.clone() }).unwrap();
        assert_eq!(runtime.view().selected_row.as_deref(), Some(second.as_str()));
        assert_eq!(runtime.selected.as_deref(), Some(first.as_str()));
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.playing.is_none());
        let mut snapshot = runtime.saved.clone(); browse_focus::prepare(&runtime, &mut snapshot).unwrap();
        let profiles = snapshot.profiles.clone(); let resumes = snapshot.resume.clone();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        assert!(!String::from_utf8_lossy(&encoded).contains("synthetic-token"));
        runtime = Box::new(runtime_fixture());
        // Recreated Runtime needs the same clock boundary as the original fixture.
        let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("fixture clock");
        runtime.clock_anchor = Some(sample);
        runtime.wall_clock_ms = sample.unix_ms();
        runtime.clock_sample = Some(sample.steady_from(sample));
        runtime.loaded = true; runtime.entropy = Some([1; 16]);
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true; runtime.plex_refresh_pending = true; runtime.resume_source_work();
        runtime.saved = serde_json::from_slice(&encoded).unwrap(); runtime.sync_saved_context();
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        category_rows(&mut runtime, &profile); runtime.selected = Some(first.clone());
        assert_eq!(runtime.view_selection().0, category);
        assert_eq!(runtime.effective_category(), category);
        assert_eq!(runtime.view().selected_row.as_deref(), Some(second.as_str()));
        assert_eq!(runtime.saved.profiles, profiles);
        assert!(runtime.saved.resume == resumes);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.playing.is_none());
        // An explicit All entry outranks the legacy top-level field, including
        // when that field belongs to an unrelated active Saved provider.
        runtime.saved = runtime.category_choice_candidate(Some("")).unwrap();
        if active { runtime.saved.category = "Legacy category".into(); }
        classic_source::release(&mut runtime);
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        assert_eq!(runtime.view_selection().0, "");
        assert_eq!(runtime.saved.profiles, profiles);
    }
}

#[test]
fn classic_saved_category_replaced_binding_cannot_write_or_inherit_focus() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new(); server();
    let (mut app, mut runtime) = fixture();
    let mut profile = saved_category_profile();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let (category, _, second) = category_rows(&mut runtime, &profile);
    runtime.saved = runtime.category_choice_candidate(Some(&category)).unwrap();
    classic_source::set_view(&mut runtime, &category);
    let context = runtime.view().preview_context.unwrap();
    handle_action(&mut runtime, &mut app, OttAction::FocusAt { context, id: second }).unwrap();
    let mut snapshot = runtime.saved.clone(); browse_focus::prepare(&runtime, &mut snapshot).unwrap();
    runtime.saved = snapshot;
    profile.source = "replacement-machine".into();
    runtime.saved.profiles.profiles[0] = profile.clone();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.category_choice_candidate(Some("Must not be stored")).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    let origin = plx_plex::plex::Origin::parse("https://replacement.invalid:443").unwrap();
    let id = plx_plex::plex::register_pinned_with_client_id(&profile.source, &origin,
        "synthetic-replacement-token", None, "synthetic-replacement-client", Default::default());
    assert!(plx_plex::plex::set_current(id));
    runtime.sync_saved_context();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let (_, first, _) = category_rows(&mut runtime, &profile); runtime.selected = Some(first.clone());
    assert_eq!(runtime.view_selection().0, "", "old fingerprint must not seed the replacement from legacy state");
    assert_eq!(runtime.view().selected_row.as_deref(), Some(first.as_str()));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    runtime.generation += 1;
    assert!(runtime.category_choice_candidate(Some("Retired generation")).is_err());
}

#[test]
fn classic_temporary_category_and_focus_never_enter_saved_profile_preferences() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new(); server();
    let (mut app, mut runtime) = fixture();
    runtime.saved.category = "Unrelated".into();
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    let profile = runtime.effective_profile().unwrap().clone();
    let (category, _, second) = category_rows(&mut runtime, &profile);
    let before = runtime.saved.clone();
    runtime.saved = runtime.category_choice_candidate(Some(&category)).unwrap();
    classic_source::set_view(&mut runtime, &category);
    let context = runtime.view().preview_context.unwrap();
    handle_action(&mut runtime, &mut app, OttAction::FocusAt { context, id: second }).unwrap();
    let mut snapshot = runtime.saved.clone(); browse_focus::prepare(&runtime, &mut snapshot).unwrap();
    assert_eq!(snapshot.profiles, before.profiles);
    assert_eq!(snapshot.category, before.category);
    assert_eq!(snapshot.settings, before.settings);
    assert!(classic_source::binding_profile(&runtime).is_some());
}

#[test]
fn provider_context_reports_temporary_classic_source_without_changing_saved() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    server();
    let (mut app, mut runtime) = fixture();
    // Admit the established explicit Classic fixture, then model a different
    // Saved selection. This tests diagnostics, not automatic startup routing.
    assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
    runtime.saved.settings.insert("selectedProvider".into(), json!("m3u"));
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "saved-m3u-canary".into(), name: "Private profile canary".into(),
        kind: ProviderKind::M3u, source: "https://private.invalid/list?token=private-canary".into(),
        source_alias: "Private alias canary".into(), enabled: true,
    });
    runtime.saved.profiles.active_profile = Some("saved-m3u-canary".into());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    runtime.sync_saved_context();
    assert!(classic_source::profile(&runtime).is_some());
    assert!(runtime.catalog_next.is_some(), "existing held worker keeps the request staged");
    app.ott = Some(runtime);
    let response = super::provider_context::read_status(&mut app, "status", 1);
    assert_eq!(response["data"]["provider_context"], json!({
        "loaded":true, "write_permit_valid":true, "state_load":"idle",
        "selected_provider":"m3u", "saved_active_profile_present":true,
        "saved_active_profile_kind":"m3u", "runtime_provider":"plex",
        "effective_provider":"plex", "managed_active":false, "classic_active":true
    }));
    for private in ["saved-m3u-canary", "Private profile canary", "Private alias canary",
        "private.invalid", "private-canary", "classic-fixture-machine", "synthetic-token"] {
        assert!(!response.to_string().contains(private));
    }
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    classic_source::release(runtime);
    let response = super::provider_context::read_status(&mut app, "state", 2);
    assert_eq!(response["data"]["provider_context"]["classic_active"], false);
    assert_eq!(response["data"]["provider_context"]["effective_provider"], "m3u");
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
}

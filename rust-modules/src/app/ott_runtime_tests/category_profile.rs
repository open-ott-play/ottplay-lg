//! Real category selection projection and writer/settlement boundaries.
//! Synthetic Saved completions do not claim persistence durability.
use super::*;

fn profile(id: &str) -> ProviderProfile {
    ProviderProfile { id: id.into(), name: id.into(), kind: ProviderKind::M3u,
        source: format!("https://media.invalid/{id}.m3u"), source_alias: String::new(), enabled: true }
}
fn entry(id: &str, category: &str) -> CatalogEntry {
    let mut e = synthetic_entry();
    e.id = id.into(); e.title = id.into(); e.category = category.into();
    e.media = MediaLocator::Http { url: format!("https://media.invalid/{id}.mp4") };
    e
}
fn ready() -> App {
    let mut app = app_fixture();
    let r = app.ott.as_mut().unwrap();
    r.loaded = true; r.entropy = Some([1;16]); r.write_permit.revoke();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true; r.plex_refresh_pending = true;
    r.saved.profiles.profiles = vec![profile("A"), profile("B")];
    r.saved.profiles.active_profile = Some("A".into());
    r.catalog.entries = vec![entry("news", "News"), entry("movie", "Movies")];
    r.continuity.stop(r.generation);
    r.page = "categories";
    app
}
fn channel_query(app: &mut App) -> Value {
    let mut r = app.ott.take().unwrap();
    let mut remote = Remote::disabled_for_test("dev_fixture", r.remote.runtime());
    let mut invocation = remote.request_for_test("channels", json!({}));
    invocation.deadline_ms = r.now_ms().saturating_add(10_000);
    let response = commands::command(&mut r, app, &invocation).expect("channel query is synchronous");
    app.ott = Some(r);
    response
}
fn switch(r: &mut Runtime, id: &str) {
    let mut candidate = r.saved.clone();
    candidate.profiles.active_profile = Some(id.into());
    category_state::prepare_profile_change(&r.saved, &mut candidate).unwrap();
    r.saved = candidate;
}
fn stage(app: &mut App, action: OttAction) -> Saved {
    let mut r = app.ott.take().unwrap();
    let candidate = match &action {
        OttAction::ShowFavorites => r.category_choice_candidate(None).unwrap(),
        OttAction::SelectCategory(category) | OttAction::SelectCategoryAt { category, .. } =>
            r.category_choice_candidate(Some(category)).unwrap(),
        _ => panic!("category action"),
    };
    handle_action(&mut r, app, action).unwrap();
    assert!(r.saving && r.pending_category_choice.is_some());
    assert!(matches!(r.save_navigation, SaveNavigation::Preserve));
    // Actual revoked writer cannot touch protected storage. Its failure is
    // consumed here; subsequent injected outcomes test the settlement boundary.
    assert!(matches!(r.rx.recv_timeout(Duration::from_secs(1)).unwrap(), Job::Saved(Err(_), None, false, _)));
    app.ott = Some(r);
    candidate
}
fn deliver(app: &mut App, result: Result<Saved, &'static str>) {
    app.ott.as_ref().unwrap().tx.send(Job::Saved(result, None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
}

#[test]
fn profile_category_a_b_a_roundtrip_retains_global_favorites_document_and_selection() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    let favorite = selection::capture(&profile("A"), &entry("news", "News")).unwrap();
    r.saved.favorites.toggle(favorite).unwrap();
    r.saved = r.category_choice_candidate(Some("News")).unwrap();
    r.saved.favorites_selected = true;
    let document = r.saved.favorites.clone();
    switch(r, "B");
    assert!(r.saved.category.is_empty());
    assert!(r.saved.favorites_selected);
    r.saved = r.category_choice_candidate(Some("Movies")).unwrap();
    r.saved.favorites_selected = true;
    let encoded = serde_json::to_vec(&r.saved).unwrap();
    r.saved = serde_json::from_slice(&encoded).unwrap();
    switch(r, "A");
    assert_eq!(r.saved.category, "News");
    assert!(r.saved.favorites_selected);
    assert_eq!(r.saved.favorites, document);
    r.page = "categories";
    assert_eq!(r.view().selected_row.as_deref(), Some("favorites"));
}

#[test]
fn changed_profile_source_cannot_inherit_previous_category() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    r.saved = r.category_choice_candidate(Some("News")).unwrap();
    let mut candidate = r.saved.clone();
    candidate.profiles.profiles[0].source = "https://other.invalid/source.m3u".into();
    category_state::prepare_profile_change(&r.saved, &mut candidate).unwrap();
    assert!(candidate.category.is_empty());
    assert!(!candidate.favorites_selected);
}

#[test]
fn partial_and_completed_missing_category_fallback_never_erases_saved_choice() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    r.saved = r.category_choice_candidate(Some("Later")).unwrap();
    let saved = serde_json::to_value(&r.saved).unwrap();
    for complete in [false, true] {
        r.catalog_complete = complete;
        assert_eq!(r.effective_category(), "");
        assert_eq!(r.ordered_ids(), vec!["news", "movie"]);
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    }
    r.catalog.entries.push(entry("later", "Later"));
    assert_eq!(r.effective_category(), "Later");
    assert_eq!(r.ordered_ids(), vec!["later"]);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
}

#[test]
fn category_selection_publishes_only_after_saved_and_failure_preserves_old_selection() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let candidate = stage(&mut app, OttAction::SelectCategory("News".into()));
    assert!(app.ott.as_ref().unwrap().saved.category.is_empty());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    deliver(&mut app, Ok(candidate));
    assert_eq!(app.ott.as_ref().unwrap().saved.category, "News");
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    let _candidate = stage(&mut app, OttAction::ShowFavorites);
    deliver(&mut app, Err("Fixture write refused"));
    let r = app.ott.as_ref().unwrap();
    assert!(!r.saved.favorites_selected);
    assert_eq!(r.saved.category, "News");
    assert_eq!(r.page, "channels");
    assert!(r.pending_category_choice.is_none());
}

#[test]
fn newer_navigation_blocks_only_old_browse_effect_not_durable_favorites_selection() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let candidate = stage(&mut app, OttAction::ShowFavorites);
    let r = app.ott.as_mut().unwrap();
    r.navigation_epoch += 1;
    r.page = "settings";
    deliver(&mut app, Ok(candidate));
    let r = app.ott.as_ref().unwrap();
    assert!(r.saved.favorites_selected);
    assert_eq!(r.page, "settings");
    assert!(r.pending_category_choice.is_none());
}

#[test]
fn transient_local_selection_uses_global_favorites_and_preserves_persistent_provider() {
    let _serial = plx_base::testlock::serial();
    let (_root_dir, mut app, _root) = super::local_integration::setup();
    let r = app.ott.as_mut().unwrap();
    let profiles = r.saved.profiles.clone();
    r.saved.category = "Permanent category".into();
    r.saved.favorites_selected = true;
    assert!(r.view_selection().1, "fresh temporary source cannot hide global Favorites");
    let candidate = r.category_choice_candidate(Some("Movies")).unwrap();
    assert!(!candidate.favorites_selected);
    assert_eq!(candidate.category, "Permanent category");
    assert_eq!(candidate.profiles, profiles);
    assert_eq!(candidate.favorites, r.saved.favorites);
}

#[test]
fn uncertain_category_write_discards_the_pending_browse_effect() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let _candidate = stage(&mut app, OttAction::ShowFavorites);
    app.ott.as_ref().unwrap().tx.send(Job::Uncertain(None, None))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert!(r.persistence_uncertain);
    assert!(r.pending_category_choice.is_none());
    assert!(!r.saved.favorites_selected);
}


#[test]
fn channel_command_uses_same_missing_category_projection_as_the_list() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    app.ott.as_mut().unwrap().saved.category = "Later".into();
    for complete in [false, true] {
        app.ott.as_mut().unwrap().catalog_complete = complete;
        let response = channel_query(&mut app);
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["channels"].as_array().unwrap().len(), 2);
        assert_eq!(response["data"]["channels"][0]["id"], "news");
        assert_eq!(app.ott.as_ref().unwrap().saved.category, "Later");
    }
    app.ott.as_mut().unwrap().catalog.entries.push(entry("later", "Later"));
    let response = channel_query(&mut app);
    assert_eq!(response["data"]["channels"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"]["channels"][0]["id"], "later");
    assert_eq!(response["data"]["channels"][0]["number"], 3);
}

fn mixed_favorites(r: &mut Runtime) {
    let first = selection::capture(&profile("A"), &entry("news", "News")).unwrap();
    let second = Favorite {
        title: "Library film".into(),
        origin: plx_data::ott::favorites::Origin { provider: "plex".into(),
            profile: "Library".into(), subprofile: None, source_alias: "Server".into() },
        locator: plx_data::ott::favorites::Locator::Plex {
            server_id: "synthetic-server".into(), rating_key: "42".into() },
    };
    r.saved.favorites.toggle(first).unwrap();
    r.saved.favorites.toggle(second).unwrap();
}

#[test]
fn global_favorites_projection_remains_mixed_when_the_active_profile_changes() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    mixed_favorites(r);
    r.saved.favorites_selected = true;
    r.page = "favorites";
    let document = r.saved.favorites.clone();
    let expected: Vec<_> = document.entries().iter()
        .map(|favorite| format!("favorite:{}", favorite.identity())).collect();
    for active in ["A", "B", "A"] {
        switch(r, active);
        let view = r.view();
        assert!(view.page_id.starts_with("favorites:"), "the real view has a scoped identity");
        assert_eq!(view.rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(), expected);
        for (row, favorite) in view.rows.iter().zip(document.entries()) {
            assert_eq!(row.content_title.as_deref(), Some(favorite.title.as_str()));
            assert_eq!(row.detail, format!("{} · {}", favorite.origin.provider, favorite.origin.profile));
            assert!(matches!(&row.action, OttAction::Play(id) if id == &row.id));
        }
        assert_eq!(r.saved.favorites, document);
        assert!(r.saved.favorites_selected);
        assert!(!r.saving);
    }
}

#[test]
fn favorites_categories_all_exits_only_after_saved_without_deleting_favorites() {
    let _serial = plx_base::testlock::serial();
    for empty in [true, false] {
        let mut app = ready();
        let mut r = app.ott.take().unwrap();
        if !empty { mixed_favorites(&mut r); }
        r.saved.favorites_selected = true;
        r.page = "favorites";
        let before = r.saved.clone();
        handle_action(&mut r, &mut app, OttAction::ShowCategories).unwrap();
        assert_eq!(r.page, "categories");
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), serde_json::to_value(&before).unwrap());
        let all = r.view().rows.iter().find(|row| row.id == "all").unwrap().action.clone();
        assert!(matches!(&all, OttAction::SelectCategoryAt { category, .. } if category.is_empty()));
        app.ott = Some(r);
        let _candidate = stage(&mut app, all);
        assert!(app.ott.as_ref().unwrap().saved.favorites_selected);
        deliver(&mut app, Err("Fixture write refused"));
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.page, "categories");
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), serde_json::to_value(&before).unwrap());
        let all = r.view().rows.iter().find(|row| row.id == "all").unwrap().action.clone();
        let candidate = stage(&mut app, all);
        deliver(&mut app, Ok(candidate));
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.page, "channels");
        assert!(!r.saved.favorites_selected);
        assert!(r.saved.category.is_empty());
        assert_eq!(r.saved.favorites, before.favorites);
        assert_eq!(r.saved.profiles, before.profiles);
        assert_eq!(r.ordered_ids(), vec!["news", "movie"]);
        assert!(!r.saving);
        assert!(r.pending_category_choice.is_none());
    }
}

#[test]
fn category_saved_after_transient_owner_retired_has_no_old_browse_effect() {
    let _serial = plx_base::testlock::serial();
    let (_root_dir, mut app, _root) = super::local_integration::setup();
    let r = app.ott.as_mut().unwrap();
    r.saving = false;
    r.entropy = Some([1; 16]);
    r.write_permit.revoke();
    r.page = "categories";
    let candidate = stage(&mut app, OttAction::ShowFavorites);
    let r = app.ott.as_mut().unwrap();
    r.transient.accepted = None;
    r.page = "settings";
    deliver(&mut app, Ok(candidate));
    let r = app.ott.as_ref().unwrap();
    assert!(r.saved.favorites_selected);
    assert_eq!(r.page, "settings");
    assert!(r.pending_category_choice.is_none());
}


// A real old-view action is queued on the same mounted screen before the
// replacement profile's Saved completion. The runtime drains Saved first.
// Revoked storage is intercepted: this test proves admission/ownership only.
fn old_category_action_cannot_write_replacement_profile(kind: ProviderKind) {
    let mut app = ready();
    {
        let runtime = app.ott.as_mut().unwrap();
        if kind == ProviderKind::VPortal {
            runtime.saved.profiles.profiles[1].kind = kind;
            runtime.saved.profiles.profiles[1].source =
                "portal::[key:fixture]https://media.invalid/api".into();
        }
        runtime.saved = runtime.category_choice_candidate(Some("News")).unwrap();
        switch(runtime, "B");
        runtime.saved = runtime.category_choice_candidate(Some("Movies")).unwrap();
        switch(runtime, "A");
        runtime.sync_saved_context();
        runtime.catalog_complete = true;
    }
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let reply = app.pages.top_page().expect("mounted OTT screen");
    let old_action = app.ott.as_ref().unwrap().view().rows.iter()
        .find(|row| row.title == "News").expect("A category row").action.clone();
    let (replacement, pending) = {
        let runtime = app.ott.as_mut().unwrap();
        let mut replacement = runtime.saved.clone();
        replacement.profiles.active_profile = Some("B".into());
        category_state::prepare_profile_change(&runtime.saved, &mut replacement).unwrap();
        assert_eq!(replacement.category, "Movies");
        runtime.save(replacement.clone(), None, true).unwrap();
        let pending = match runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            Job::Saved(Err(_), invocation, reload, response) => (invocation, reload, response),
            _ => panic!("expected pre-IO revoked writer refusal"),
        };
        (replacement, pending)
    };
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
        reply, action: old_action,
    })));
    let frame = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, frame, Vec::new());
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(replacement.clone()),
        pending.0, pending.1, pending.2)).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let wrong_writer_admitted = runtime.saving;
    if wrong_writer_admitted {
        // Drain the unintended writer's pre-IO refusal even on a failing test.
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Job::Saved(Err(_), None, false, _)));
    }
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("B"));
    assert_eq!(runtime.saved.category, "Movies");
    assert_eq!(runtime.saved.settings, replacement.settings);
    assert_eq!(runtime.page, "channels");
    assert!(!wrong_writer_admitted,
        "an A-view category click must not admit a category write for replacement profile B");
    assert!(runtime.pending_category_choice.is_none());
}

#[test]
fn old_m3u_category_action_cannot_write_new_m3u_profile() {
    let _serial = plx_base::testlock::serial();
    old_category_action_cannot_write_replacement_profile(ProviderKind::M3u);
}

#[test]
fn old_m3u_category_action_cannot_write_new_vportal_profile() {
    let _serial = plx_base::testlock::serial();
    old_category_action_cannot_write_replacement_profile(ProviderKind::VPortal);
}


fn load_without_active_profile(favorites: bool, navigation: u64) -> (App, Saved) {
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    let mut incoming = r.saved.clone();
    incoming.profiles.active_profile = None;
    incoming.favorites_selected = favorites;
    incoming.favorites.toggle(selection::capture(&profile("A"), &entry("news", "News")).unwrap()).unwrap();
    incoming.validate().unwrap();
    r.saved = Saved::default();
    r.loaded = false;
    r.catalog = Catalog::default();
    r.page = if navigation == 0 { "providers" } else { "settings" };
    r.navigation_epoch = navigation;
    r.tx.send(Job::Loaded(Ok((incoming.clone(), [2; 16]))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    (app, incoming)
}

#[test]
fn initial_global_favorites_without_active_profile_reopens_list_without_playing() {
    let _serial = plx_base::testlock::serial();
    let (app, incoming) = load_without_active_profile(true, 0);
    let r = app.ott.as_ref().unwrap();
    assert!(r.loaded && r.saved.profiles.active_profile.is_none());
    assert_eq!(r.page, "favorites");
    let rows = r.raw_view().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, format!("favorite:{}", incoming.favorites.entries()[0].identity()));
    assert_eq!(r.saved.favorites, incoming.favorites);
    assert!(r.play_target.is_none() && r.resume_pending.is_none() && r.playing.is_none());
    assert!(!r.catalog_loading && r.catalog_next.is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn initial_no_active_profile_without_favorites_keeps_provider_selection() {
    let _serial = plx_base::testlock::serial();
    let (app, _) = load_without_active_profile(false, 0);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.page, "providers");
    assert!(!r.saved.favorites_selected);
    assert!(r.play_target.is_none() && r.playing.is_none());
}

#[test]
fn late_global_favorites_load_without_active_profile_cannot_replace_newer_navigation() {
    let _serial = plx_base::testlock::serial();
    let (app, _) = load_without_active_profile(true, 1);
    let r = app.ott.as_ref().unwrap();
    assert!(r.saved.favorites_selected);
    assert_eq!(r.page, "settings");
    assert!(r.play_target.is_none() && r.playing.is_none());
}


#[test]
fn category_row_context_survives_new_batches_but_not_same_label_source_aba() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let runtime = app.ott.as_mut().unwrap();
    runtime.sync_saved_context();
    let action = runtime.view().rows.iter().find(|row| row.title == "News")
        .expect("real category row").action.clone();
    assert!(matches!(&action, OttAction::SelectCategoryAt { .. }));
    let admitted = category_state::resolve_action(runtime, action.clone()).unwrap();
    assert!(matches!(admitted, OttAction::SelectCategory(_)));
    runtime.catalog.entries.push(entry("later", "Later"));
    runtime.catalog_revision += 1;
    assert_eq!(category_state::resolve_action(runtime, action.clone()).unwrap(), admitted,
        "a sibling batch must not invalidate a still-present category");

    switch(runtime, "B");
    runtime.sync_saved_context();
    runtime.generation += 1;
    assert!(runtime.organization_categories().iter().any(|row| row.title == "News"),
        "B deliberately has the same category label and provider-derived ID");
    assert!(category_state::resolve_action(runtime, action.clone()).is_err());
    switch(runtime, "A");
    runtime.sync_saved_context();
    runtime.generation += 1;
    assert!(category_state::resolve_action(runtime, action).is_err(),
        "A to B to A must not revive the original click");
}

#[test]
fn category_all_is_scoped_but_global_favorites_and_internal_selection_are_not() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let runtime = app.ott.as_mut().unwrap();
    let view = runtime.view();
    let all = view.rows.iter().find(|row| row.id == "all").unwrap().action.clone();
    let favorites = view.rows.iter().find(|row| row.id == "favorites").unwrap().action.clone();
    assert!(matches!(&all, OttAction::SelectCategoryAt { category, .. } if category.is_empty()));
    assert_eq!(favorites, OttAction::ShowFavorites);
    runtime.navigation_epoch += 1;
    assert!(category_state::resolve_action(runtime, all).is_err());
    assert_eq!(category_state::resolve_action(runtime, favorites).unwrap(), OttAction::ShowFavorites);
    let internal = OttAction::SelectCategory("News".into());
    assert_eq!(category_state::resolve_action(runtime, internal.clone()).unwrap(), internal);
}


#[test]
fn stale_screen_preview_cannot_target_replacement_profile_with_same_entry_id() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    app.running = true;
    {
        let r = app.ott.as_mut().unwrap();
        r.sync_saved_context();
        r.page = "channels";
        r.selected = Some("shared".into());
        r.catalog.entries = vec![entry("shared", "News")];
        r.catalog_complete = true;
    }
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let reply = app.pages.top_page().expect("mounted OTT screen");
    let mut actions = app.bridge.take_ott_reqs().into_iter().filter_map(|out| match out {
        OttOut::Action { action, .. } => Some(action), _ => None,
    });
    let old_preview = actions.next().expect("actual screen dwell must produce a Preview action");
    assert!(format!("{old_preview:?}").starts_with("Preview"));
    assert!(actions.next().is_none(), "one source-bound preview per settled focus");
    let (replacement, pending) = {
        let r = app.ott.as_mut().unwrap();
        assert!(r.metadata_target.is_none(), "old action has not reached the runtime");
        let mut replacement = r.saved.clone();
        replacement.profiles.active_profile = Some("B".into());
        category_state::prepare_profile_change(&r.saved, &mut replacement).unwrap();
        // The actual writer is unable to access protected storage. Preserve its
        // owned completion shape while modeling durable publication below.
        r.save(replacement.clone(), None, false).unwrap();
        let pending = match r.rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            Job::Saved(Err(_), invocation, reload, response) => (invocation, reload, response),
            _ => panic!("revoked Saved writer"),
        };
        (replacement, pending)
    };
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(replacement), pending.0, pending.1, pending.2))
        .unwrap_or_else(|_| panic!("fixture Saved receiver"));
    runtime_tick(&mut app);
    let next_generation = {
        let r = app.ott.as_mut().unwrap();
        assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("B"));
        assert!(r.catalog.entries.is_empty(), "replacement loader remains held");
        assert!(r.play_target.is_none() && r.resolver_next.is_none() && r.playing.is_none());
        // Profile activation owns a fresh autoplay policy. Stop that unrelated
        // startup before testing the retained A preview against B's first batch.
        r.continuity.stop(r.generation);
        r.generation
    };
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
        reply, action: old_preview,
    })));
    let frame = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, frame, Vec::new());
    let r = app.ott.as_ref().unwrap();
    // Use the generation published by the real Saved handler, not a guessed
    // number of internal source invalidations. This tick installs B's batch
    // before dispatching the retained A screen action for the same entry ID.
    r.catalog_tx.send(CatalogEvent::Batch(next_generation,
        Catalog { hierarchy: None, entries: vec![entry("shared", "News")] }))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver"));
    runtime_tick(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("B"));
    assert_eq!(r.catalog.entries[0].id, "shared");
    assert!(r.metadata_target.is_none(),
        "an A screen Preview must not queue B metadata merely because B reused the entry ID");
    assert!(r.metadata_next.is_none());
    assert!(r.play_target.is_none() && r.resolver_next.is_none() && r.playing.is_none());
    assert!(!r.metadata_done.load(Ordering::Acquire), "fixture worker slot stays held");
}


fn preview_ready() -> App {
    let mut app = ready();
    app.running = true;
    { let r = app.ott.as_mut().unwrap(); r.page = "channels"; r.sync_saved_context(); }
    runtime_tick(&mut app);
    app
}

#[test]
fn preview_context_survives_batches_but_not_source_aba_and_keeps_internal_action() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    let r = app.ott.as_mut().unwrap();
    let context = r.view().preview_context.expect("active catalogue preview context");
    let action = OttAction::PreviewAt { context, id: "news".into() };
    assert_eq!(preview::resolve(r, action.clone()).unwrap(), OttAction::Preview("news".into()));
    r.catalog.entries.push(entry("later", "Later")); r.catalog_revision += 1;
    assert!(preview::resolve(r, action.clone()).is_ok(), "append does not replace source");
    switch(r, "B"); r.sync_saved_context(); r.generation += 1;
    assert!(preview::resolve(r, action.clone()).is_err());
    switch(r, "A"); r.sync_saved_context(); r.generation += 1;
    assert!(preview::resolve(r, action).is_err(), "old source A receipt cannot revive");
    let internal = OttAction::Preview("news".into());
    assert_eq!(preview::resolve(r, internal.clone()).unwrap(), internal);
}

#[test]
fn preview_favorites_context_covers_inactive_actual_owner_not_only_active_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    let r = app.ott.as_mut().unwrap();
    let favorite = selection::capture(&profile("B"), &entry("news", "News")).unwrap();
    let id = format!("favorite:{}", favorite.identity());
    r.saved.favorites.toggle(favorite).unwrap();
    r.saved.favorites_selected = true; r.page = "favorites";
    let context = r.view().preview_context.unwrap();
    let action = OttAction::PreviewAt { context, id };
    assert!(preview::resolve(r, action.clone()).is_ok());
    let active = r.active().cloned();
    r.saved.profiles.profiles.iter_mut().find(|p| p.id == "B").unwrap().source =
        "https://replacement.invalid/b.m3u".into();
    assert_eq!(r.active(), active.as_ref());
    assert!(preview::resolve(r, action).is_err(), "Favorite B uses its actual owner");
}

#[test]
fn preview_context_tracks_transient_local_owner_and_does_not_touch_saved() {
    let _serial = plx_base::testlock::serial();
    let (_root_dir, mut app, _root) = super::local_integration::setup();
    app.running = true;
    runtime_tick(&mut app);
    let r = app.ott.as_mut().unwrap();
    r.page = "channels";
    let before = serde_json::to_value(&r.saved).unwrap();
    let context = r.view().preview_context.unwrap();
    let id = r.ordered_ids().into_iter().next().expect("local fixture row");
    let action = OttAction::PreviewAt { context, id };
    assert!(preview::resolve(r, action.clone()).is_ok());
    let persistent = r.active().cloned();
    r.transient.accepted = None;
    assert_eq!(r.active(), persistent.as_ref());
    assert!(preview::resolve(r, action).is_err(), "same persistent profile is not the Local owner");
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
}

#[test]
fn preview_fenced_action_is_non_navigating_and_never_starts_playback() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    bridge::show_page(&mut app.pages, AppArg::Ott); settle_pages(&mut app);
    runtime_tick(&mut app); settle_pages(&mut app);
    app.bridge.take_ott_reqs(); // Discard any already-produced dwell message.
    let reply = app.pages.top_page().unwrap();
    let r = app.ott.as_ref().unwrap();
    let navigation = r.navigation_epoch;
    let saved = serde_json::to_value(&r.saved).unwrap();
    let action = OttAction::PreviewAt { context: r.view().preview_context.unwrap(), id: "news".into() };
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let frame = next_tick(&mut app); bridge::frame(&mut app.pages, &mut app.bridge, frame, Vec::new());
    runtime_tick(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.navigation_epoch, navigation);
    assert_eq!(r.metadata_target, Some(("A".into(), "news".into())));
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(r.play_target.is_none() && r.resolver_next.is_none() && r.playing.is_none());
}

#[test]
fn caption_projects_effective_category_all_and_favorites_without_relabeling_profile() {
    let _serial = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut app = ready(); let r = app.ott.as_mut().unwrap();
    r.page = "channels"; r.saved.favorites_selected = false;
    assert_eq!(r.view().category_label.as_deref(), Some("All"));
    r.saved = r.category_choice_candidate(Some("News")).unwrap();
    assert_eq!(r.view().category_label.as_deref(), Some("News"));
    assert_eq!(r.view().description, "A");
    r.saved.favorites_selected = true;
    assert_eq!(r.view().category_label.as_deref(), Some("Favorites"));
    r.page = "profiles"; assert!(r.view().category_label.is_none());
}

fn focus_row(app: &mut App, id: &str) {
    let mut runtime = app.ott.take().unwrap();
    let action = OttAction::FocusAt { context: runtime.view().preview_context.unwrap(), id: id.into() };
    handle_action(&mut runtime, app, action).unwrap();
    app.ott = Some(runtime);
}

#[test]
fn browse_focus_roundtrip_is_independent_of_playing_checkpoint_and_autoplay() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    {
        let r = app.ott.as_mut().unwrap();
        r.selected = Some("news".into());
        r.playing = Some(Playing { channel_capture: None, kiosk_selection: None, archive: None,
            transient_owner: None, local_selection: None, identity: None, profile: profile("A"),
            entry: entry("news", "News"), selection_id: "news".into(), source: None,
            advance_on_failure: true, requires_server_lease: false });
        r.saved.resume.insert("A".into(), SavedResume { entry_id: "news".into(), position_ms: 12_500,
            source_fingerprint: profile_fingerprint(&profile("A")) });
    }
    focus_row(&mut app, "movie");
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.view().selected_row.as_deref(), Some("movie"));
    assert_eq!(r.selected.as_deref(), Some("news"));
    assert_eq!(r.playing.as_ref().unwrap().selection_id, "news");
    assert_eq!(r.saved.resume["A"].entry_id, "news");
    assert_eq!(r.saved.resume["A"].position_ms, 12_500);
    assert!(r.play_target.is_none() && r.resolver_next.is_none() && !r.saving);
    assert!(!r.saved.settings.contains_key("nativeBrowseFocusV1"));
    // This is the same candidate merge used by the normal writer and final q
    // shutdown write, without running a protected writer in this fixture.
    let mut final_snapshot = r.saved.clone(); browse_focus::prepare(r, &mut final_snapshot).unwrap();
    let saved: Saved = serde_json::from_slice(&serde_json::to_vec(&final_snapshot).unwrap()).unwrap();
    let mut restarted = preview_ready();
    let r = restarted.ott.as_mut().unwrap(); r.saved = saved; r.sync_saved_context();
    r.selected = Some("news".into());
    assert_eq!(r.view().selected_row.as_deref(), Some("movie"));
    assert_eq!(r.saved.resume["A"].entry_id, "news");
    assert_eq!(r.saved.resume["A"].position_ms, 12_500);
}

#[test]
fn metadata_dwell_does_not_become_browse_intent_and_stale_focus_is_rejected() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    let mut r = app.ott.take().unwrap(); r.selected = Some("news".into());
    let context = r.view().preview_context.unwrap();
    handle_action(&mut r, &mut app, OttAction::PreviewAt { context: context.clone(), id: "movie".into() }).unwrap();
    assert_eq!(r.view().selected_row.as_deref(), Some("news"));
    let mut saved = r.saved.clone(); browse_focus::prepare(&r, &mut saved).unwrap();
    assert!(!saved.settings.contains_key("nativeBrowseFocusV1"));
    let stale = OttAction::FocusAt { context, id: "movie".into() };
    switch(&mut r, "B"); r.sync_saved_context(); r.generation += 1;
    assert!(handle_action(&mut r, &mut app, stale).is_err());
    assert_eq!(r.view().selected_row.as_deref(), Some("news"));
    assert!(r.play_target.is_none() && r.resolver_next.is_none());
}

#[test]
fn completed_absence_uses_normal_fallback_without_retargeting_saved_focus() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready(); focus_row(&mut app, "movie");
    {
        let r = app.ott.as_mut().unwrap(); r.selected = Some("news".into());
        r.catalog_complete = true; r.catalog.entries.retain(|row| row.id != "movie"); r.catalog_revision += 1;
        assert_eq!(r.view().selected_row.as_deref(), Some("news"));
        assert!(r.play_target.is_none() && r.resolver_next.is_none());
        r.catalog.entries.push(entry("movie", "Movies")); r.catalog_revision += 1;
        assert_eq!(r.view().selected_row.as_deref(), Some("movie"));
    }
    focus_row(&mut app, "news");
    let r = app.ott.as_mut().unwrap();
    r.catalog.entries.retain(|row| row.id != "movie"); r.catalog_revision += 1;
    r.catalog.entries.push(entry("movie", "Movies")); r.catalog_revision += 1;
    assert_eq!(r.view().selected_row.as_deref(), Some("news"), "newer explicit focus wins");
}

#[test]
fn failed_or_busy_writer_does_not_clear_pending_browse_focus() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready(); focus_row(&mut app, "movie");
    let r = app.ott.as_mut().unwrap(); r.saving = true;
    browse_focus::request_flush(r); browse_focus::flush(r);
    assert!(r.saving);
    let mut candidate = r.saved.clone(); browse_focus::prepare(r, &mut candidate).unwrap();
    assert!(candidate.settings.contains_key("nativeBrowseFocusV1"));
    // A failed/uncertain writer does not replace Saved; reconciliation against
    // that old committed image must keep the newer RAM intent available.
    r.saving = false; r.persistence_uncertain = true; browse_focus::reconcile(r); browse_focus::flush(r);
    assert!(!r.saving);
    let mut exit_snapshot = r.saved.clone(); browse_focus::prepare(r, &mut exit_snapshot).unwrap();
    assert_eq!(exit_snapshot.settings["nativeBrowseFocusV1"], candidate.settings["nativeBrowseFocusV1"]);
    assert_eq!(r.view().selected_row.as_deref(), Some("movie"));
}

#[test]
fn explicit_ui_play_replaces_missing_focus_but_remote_play_does_not() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready(); focus_row(&mut app, "movie");
    let mut r = app.ott.take().unwrap();
    r.catalog.entries.retain(|row| row.id != "movie"); r.catalog_complete = true; r.catalog_revision += 1;
    // The fixture holds the resolver; admission queues no media I/O.
    r.play(&mut app, "news").unwrap();
    r.catalog.entries.push(entry("movie", "Movies")); r.catalog_revision += 1;
    assert_eq!(r.view().selected_row.as_deref(), Some("movie"));
    r.catalog.entries.retain(|row| row.id != "movie"); r.catalog_revision += 1;
    handle_action(&mut r, &mut app, OttAction::Play("news".into())).unwrap();
    r.catalog.entries.push(entry("movie", "Movies")); r.catalog_revision += 1;
    assert_eq!(r.view().selected_row.as_deref(), Some("news"));
    assert_eq!(r.play_target.as_ref().unwrap().id, "news");
}


#[test]
fn saved_vportal_structural_category_and_focus_restore_together() {
    let _serial = plx_base::testlock::serial();
    let mut app = preview_ready();
    let profile = synthetic_profile();
    let catalog = plx_data::ott::parse_vportal_response(&json!({
        "type":"videoportal", "items":[{"type":"category", "title":"Series", "items":[
            {"type":"category", "title":"Season", "items":[
                {"type":"stream", "id":"first", "title":"First", "request":{"id":"first"}},
                {"type":"stream", "id":"second", "title":"Second", "request":{"id":"second"}}
            ]}
        ]}]
    }).to_string(), &profile, "").unwrap().catalog;
    let r = app.ott.as_mut().unwrap();
    r.saved.profiles.profiles = vec![profile.clone()];
    r.saved.profiles.active_profile = Some(profile.id.clone());
    r.sync_saved_context(); r.catalog = catalog.clone(); r.catalog_revision += 1;
    r.catalog_complete = true; r.catalog_loading = false; r.page = "channels";
    let category = r.organization_categories().into_iter()
        .find(|row| row.title == "Series / Season").unwrap().id;
    let first = r.catalog.entries[0].id.clone(); let second = r.catalog.entries[1].id.clone();
    r.saved = r.category_choice_candidate(Some(&category)).unwrap();
    r.selected = Some(first.clone());
    focus_row(&mut app, &second);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.selected.as_deref(), Some(first.as_str()));
    assert!(r.play_target.is_none() && r.resolver_next.is_none() && r.playing.is_none());
    let mut snapshot = r.saved.clone(); browse_focus::prepare(r, &mut snapshot).unwrap();
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    app.ott = Some(runtime_fixture());
    let r = app.ott.as_mut().unwrap();
    r.loaded = true; r.resume_source_work();
    r.saved = serde_json::from_slice(&encoded).unwrap(); r.sync_saved_context();
    r.catalog = catalog.clone(); r.catalog_revision += 1;
    r.catalog_complete = true; r.catalog_loading = false; r.page = "channels";
    r.selected = Some(first);
    assert_eq!(r.effective_category(), category);
    assert_eq!(r.view().selected_row.as_deref(), Some(second.as_str()));
    // Missing/partial source categories remain a projection fallback, never a
    // rewrite of either saved category or its independent browse selection.
    r.catalog = Catalog::default(); r.catalog_revision += 1; r.catalog_complete = false;
    assert_eq!(r.effective_category(), ""); assert_eq!(r.saved.category, category);
    r.catalog = catalog; r.catalog_revision += 1; r.catalog_complete = true;
    assert_eq!(r.effective_category(), category);
    assert_eq!(r.view().selected_row.as_deref(), Some(second.as_str()));
    assert!(r.saved.resume.is_empty());
    assert!(r.play_target.is_none() && r.resolver_next.is_none() && r.playing.is_none());
}

#[test]
fn home_display_name_import_preserves_category_and_global_favorites() {
    for remembered in [false, true] {
        for favorites in [false, true] {
            let mut before = Saved::default();
            before.profiles.profiles = vec![profile("A")];
            before.profiles.active_profile = Some("A".into());
            before.category = "News".into();
            before.favorites_selected = favorites;
            before.favorites.toggle(selection::capture(&profile("A"), &entry("news", "News")).unwrap()).unwrap();
            if remembered {
                let fingerprint = profile_fingerprint(&before.profiles.profiles[0]);
                let mut preferences = category_preferences::Preferences::default();
                preferences.choose_category(category_preferences::Source {
                    profile_id: "A", fingerprint: &fingerprint,
                }, "News").unwrap();
                preferences.write_into(&mut before.settings).unwrap();
            }
            let original = serde_json::to_value(&before).unwrap();
            let mut imported = before.clone();
            imported.profiles.profiles[0].name = "Home".into();
            category_state::prepare_profile_change(&before, &mut imported).unwrap();
            assert_eq!(imported.category, "News", "remembered={remembered}");
            let prepared = serde_json::to_value(&imported).unwrap();
            category_state::prepare_profile_change(&before, &mut imported).unwrap();
            assert_eq!(serde_json::to_value(&imported).unwrap(), prepared,
                "preparing an existing after-image must be idempotent");
            assert_eq!(imported.favorites_selected, favorites);
            assert_eq!(imported.favorites, before.favorites);
            assert_eq!(serde_json::to_value(&before).unwrap(), original);
            let restored: Saved = serde_json::from_slice(&serde_json::to_vec(&imported).unwrap()).unwrap();
            assert_eq!(category_state::remembered_category(&restored, &restored.profiles.profiles[0]).unwrap(), "News");
            assert_eq!(restored.favorites, before.favorites);
        }
    }
}

#[test]
fn inactive_display_name_import_keeps_its_category_without_changing_active_selection() {
    let mut before = Saved::default();
    before.profiles.profiles = vec![profile("A"), profile("B")];
    before.profiles.active_profile = Some("B".into());
    before.category = "Movies".into();
    before.favorites_selected = true;
    let mut preferences = category_preferences::Preferences::default();
    let fingerprint = profile_fingerprint(&before.profiles.profiles[0]);
    preferences.choose_category(category_preferences::Source {
        profile_id: "A", fingerprint: &fingerprint,
    }, "News").unwrap();
    preferences.write_into(&mut before.settings).unwrap();
    let mut imported = before.clone();
    imported.profiles.profiles[0].name = "Home".into();
    category_state::prepare_profile_change(&before, &mut imported).unwrap();
    assert_eq!(imported.category, "Movies");
    assert!(imported.favorites_selected);
    assert_eq!(imported.profiles.active_profile.as_deref(), Some("B"));
    let mut switched = imported.clone();
    switched.profiles.active_profile = Some("A".into());
    category_state::prepare_profile_change(&imported, &mut switched).unwrap();
    assert_eq!(switched.category, "News");
    assert!(switched.favorites_selected);
}

#[test]
fn profile_name_change_never_rebinds_a_changed_source_or_stale_category_fingerprint() {
    for change in ["source", "alias", "enabled", "stale"] {
        let mut before = Saved::default();
        before.profiles.profiles = vec![profile("A")];
        before.profiles.active_profile = Some("A".into());
        before.category = "News".into();
        before.favorites_selected = true;
        let mut preferences = category_preferences::Preferences::default();
        let fingerprint = if change == "stale" { "a".repeat(64) }
            else { profile_fingerprint(&before.profiles.profiles[0]) };
        preferences.choose_category(category_preferences::Source {
            profile_id: "A", fingerprint: &fingerprint,
        }, "News").unwrap();
        preferences.write_into(&mut before.settings).unwrap();
        let mut imported = before.clone();
        let next = &mut imported.profiles.profiles[0];
        next.name = "Home".into();
        match change {
            "source" => next.source = "https://other.invalid/changed.m3u".into(),
            "alias" => next.source_alias = "Different source".into(),
            "enabled" => next.enabled = false,
            "stale" => {},
            _ => unreachable!(),
        }
        let proposed = serde_json::to_value(&imported).unwrap();
        let result = category_state::prepare_profile_change(&before, &mut imported);
        if change == "enabled" {
            assert!(result.is_err(), "a disabled active profile is invalid");
            assert_eq!(serde_json::to_value(&imported).unwrap(), proposed);
            continue;
        }
        result.unwrap();
        assert!(imported.category.is_empty(), "{change}");
        assert!(imported.favorites_selected, "{change}");
        assert_eq!(imported.favorites, before.favorites);
    }
}


// Keep construction and restored Runtime in separate frames: no two App values
// coexist. The Local root below is a synthetic owner input, never an opened path.
fn mixed_favorites_restart_image() -> (Vec<u8>, Vec<String>) {
    use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    let mut plex = profile("plex");
    plex.kind = ProviderKind::Plex;
    plex.source = "fixture-server".into();
    let mut portal = profile("portal");
    portal.kind = ProviderKind::VPortal;
    portal.source = "portal::[key:fixture]https://media.invalid/api".into();
    r.saved.profiles.profiles.extend([plex.clone(), portal.clone()]);
    let mut row = entry("shared-row", "News");
    row.title = "Same title".into();
    row.provider_id = Some("42".into());
    for id in ["A", "B"] {
        r.saved.favorites.toggle(selection::capture(&profile(id), &row).unwrap()).unwrap();
    }
    let entries = r.saved.favorites.entries();
    assert_eq!(entries[0].locator, entries[1].locator, "same tvg-id in two profiles");
    assert_ne!(entries[0].identity(), entries[1].identity());
    row.media = MediaLocator::Plex { server_id: plex.source.clone(), rating_key: "42".into() };
    r.saved.favorites.toggle(selection::capture(&plex, &row).unwrap()).unwrap();
    row.media = MediaLocator::VPortal { request: json!({"fid":"42"}) };
    row.provider_id_field = Some("fid".into());
    r.saved.favorites.toggle(selection::capture(&portal, &row).unwrap()).unwrap();
    row.media = MediaLocator::Local { relative_path: "Series/film.mp4".into() };
    let root = BoundSource { id: "temporary-root".into(), name: "Movies".into(),
        kind: SourceKind::Folder, available: true, authority_ref: AuthorityRef(11),
        generation: 7, canonical_path: Some("/fixture/temporary-movies".into()) };
    let bindings = selection::SelectionBindings::new(&r.saved.profiles, [&root], 7).unwrap();
    let local = selection::capture_bound(&bindings, selection::SelectionOwner::LocalRoot(&root), &row).unwrap();
    r.saved.favorites.toggle(local).unwrap();
    r.saved = r.category_choice_candidate(Some("News")).unwrap();
    switch(r, "B");
    r.saved = r.category_choice_candidate(Some("Movies")).unwrap();
    switch(r, "A");
    r.saved.favorites_selected = true;
    r.saved.validate().unwrap();
    let ids = r.saved.favorites.entries().iter()
        .map(|favorite| format!("favorite:{}", favorite.identity())).collect();
    (serde_json::to_vec(&r.saved).unwrap(), ids)
}

#[test]
fn mixed_favorites_restart_preserves_profile_categories_and_requires_new_local_binding() {
    let _serial = plx_base::testlock::serial();
    let (image, ids) = mixed_favorites_restart_image();
    let mut app = ready();
    let r = app.ott.as_mut().unwrap();
    r.saved = serde_json::from_slice(&image).unwrap();
    r.saved.validate().unwrap();
    r.catalog = Catalog::default();
    r.catalog_revision = r.catalog_revision.wrapping_add(1);
    r.page = "favorites";
    assert!(r.transient.accepted.is_none() && r.local_inventory.sources.is_empty());
    let document = r.saved.favorites.clone();
    assert_eq!(document.entries().len(), 5);
    assert_eq!(document.entries().iter().map(|f| f.origin.provider.as_str()).collect::<Vec<_>>(),
        ["m3u", "m3u", "plex", "vportal", "local"]);
    let generation = r.generation;
    for (active, category) in [("A", "News"), ("B", "Movies"), ("portal", ""), ("plex", ""), ("A", "News")] {
        switch(r, active);
        r.sync_saved_context();
        r.changed();
        assert_eq!(r.saved.category, category, "category belongs to the selected profile");
        assert!(r.saved.favorites_selected);
        let view = r.view();
        assert!(view.page_id.starts_with("favorites:"));
        assert_eq!(view.rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(), ids);
        for row in &view.rows {
            assert_eq!(row.content_title.as_deref(), Some("Same title"));
            assert!(matches!(&row.action, OttAction::Play(id) if id == &row.id));
        }
        let bindings = r.selection_bindings().unwrap();
        for (favorite, expected_owner) in r.saved.favorites.entries().iter()
            .zip([Some("A"), Some("B"), Some("plex"), Some("portal"), None]) {
            match expected_owner {
                Some(expected) => assert!(matches!(bindings.resolve(favorite),
                    Ok(selection::SelectionOwner::Profile(profile)) if profile.id == expected),
                    "active profile must not retarget a mixed Favorite"),
                None => assert!(matches!(bindings.resolve(favorite), Err(selection::BindingError::MissingProfile)),
                    "a saved temporary Local favorite stays visible but carries no live root authority"),
            }
        }
        assert_eq!(r.saved.favorites, document);
        assert_eq!(r.generation, generation);
        assert!(!r.saving && r.play_target.is_none() && r.resolver_next.is_none() && r.catalog_next.is_none());
    }
}

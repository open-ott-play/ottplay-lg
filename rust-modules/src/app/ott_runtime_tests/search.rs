//! Runtime search requests pass through the mounted Dispatcher/Bridge.
//! Worker slots are held. Local context uses a real temporary root descriptor;
//! no filesystem worker, protected-state write or media admission is needed.
use super::*;
use crate::app::ott_local_session as local;

fn item(id: &str, title: &str, category: &str) -> CatalogEntry {
    CatalogEntry { id: id.into(), title: title.into(), category: category.into(),
        description: String::new(), poster: String::new(),
        media: MediaLocator::Http { url: format!("https://media.invalid/{id}.mp4") },
        duration_ms: Some(10_000), provider_id: None, provider_id_field: None, live: None }
}
fn mounted() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saved.profiles.profiles = vec![ProviderProfile {
        id: "permanent".into(), name: "Permanent".into(), kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(), source_alias: String::new(), enabled: true,
    }];
    runtime.saved.profiles.active_profile = Some("permanent".into());
    runtime.local_inventory.phase = transient::InventoryPhase::Ready(runtime.generation);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.write_permit.revoke();
    runtime.continuity.stop(runtime.generation);
    runtime.catalog.entries = vec![item("one", "Straße", "Drama"),
        item("two", "Σίσυφος", "Drama"), item("three", "Café", "Comedy")];
    runtime.page = "channels";
    runtime.changed();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(matches!(app.route(), AppArg::Ott));
    app
}
fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}
fn begin(app: &mut App) -> u64 {
    ui(app, OttAction::BeginSearch);
    let editor = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap();
    assert!(editor.key.is_empty());
    editor.id
}
fn query(app: &mut App, value: &str) {
    let session = begin(app);
    ui(app, OttAction::SubmitSearch { session, value: value.into() });
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
}
fn ids(app: &App) -> Vec<String> {
    app.ott.as_ref().unwrap().view().rows.into_iter().map(|r| r.id).collect()
}
fn saved(app: &App) -> Value {
    serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap()
}
fn install_local_context(
    app: &mut App,
    generation: u64,
    root_dir: &super::local_integration::TempDir,
) {
    let runtime = app.ott.as_mut().unwrap();
    let root = local::BoundSource {
        id: "root".into(),
        name: "Local Movies".into(),
        kind: local::SourceKind::Folder,
        available: true,
        authority_ref: local::AuthorityRef(generation),
        generation,
        canonical_path: Some(root_dir.folder("LocalMovies")),
    };
    let request =
        local::LocalRequest::parse(&json!({"op":"play", "runtime":runtime.remote.runtime(),
        "source":"root", "session":true, "progressive":true}))
        .unwrap();
    let owner = local::OwnerToken(generation);
    let mut pending = local::PendingLocal::new(request, owner, u64::MAX);
    // This fixture exposes all three existing catalogue rows under this owner.
    let rows = ["one", "two", "three"].map(|id| local::LocalItem {
        id: id.into(),
        playable: true,
    });
    let scan = local::CatalogSnapshot {
        source_id: "root",
        authority_ref: root.authority_ref,
        source_generation: generation,
        state: local::ScanState::Scanning,
        items: &rows,
    };
    let context = local::AdmissionContext {
        runtime: runtime.remote.runtime(),
        owner,
        invocation_current: true,
        now_ms: runtime.now_ms(),
        mutations_allowed: true,
    };
    let local::Admission::Propose(context) = pending.evaluate(&context, &root, &scan, 1) else {
        panic!("local context should be proposed");
    };
    runtime.local_inventory.install(
        vec![super::local_integration::acquire_root(root.clone())],
        generation,
    );
    let binding = runtime.bind_local_selection(&root).unwrap();
    let read = runtime
        .capture_local_source(&binding)
        .expect("installed live source");
    runtime.transient.accepted = Some(transient::TransientMedia::Local(transient::LocalSession {
        read,
        context,
        profile: transient::source_profile(&root).unwrap(),
        borrowed: None, kiosk_resume: None,
    }));
    runtime.changed();
}

#[test]
fn show_catalog_preserves_query_category_and_row_without_play_reload_or_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    { let runtime = app.ott.as_mut().unwrap(); runtime.saved.category = "Drama".into();
      runtime.selected = Some("one".into()); runtime.changed(); }
    query(&mut app, "STRASSE");
    let before = saved(&app);
    let generation = app.ott.as_ref().unwrap().generation;
    ui(&mut app, OttAction::ShowSettings);
    ui(&mut app, OttAction::ShowCatalog);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert_eq!(runtime.view().search.as_ref().unwrap().query, "STRASSE");
    assert_eq!(runtime.view().selected_row.as_deref(), Some("one"));
    assert_eq!(ids(&app), ["one"]);
    assert_eq!(runtime.generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.catalog_next.is_none());
    assert!(!runtime.saving && !plx_media::route::has_url(&app.player.session));
    assert_eq!(saved(&app), before);
}

#[test]
fn search_is_unicode_caseless_keeps_original_ids_and_accepts_new_scan_rows() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    for (value, expected) in [("STRASSE", "one"), ("ΣΊΣΥΦΟΣ", "two"), ("Cafe\u{301}", "three")] {
        query(&mut app, value); assert_eq!(ids(&app), [expected]);
    }
    query(&mut app, "cafe"); // Casefold does not erase accents.
    assert!(ids(&app).is_empty());
    query(&mut app, "STRASSE");
    let runtime = app.ott.as_ref().unwrap();
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation, Catalog { hierarchy: None, entries: vec![item("four", "Neue Straße", "Drama")] })).unwrap();
    runtime_tick(&mut app);
    assert_eq!(ids(&app), ["one", "four"]);
    assert_eq!(app.ott.as_ref().unwrap().view().rows[1].title, "4  Neue Straße");
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn cancel_keeps_committed_query_empty_clears_and_failed_submit_can_retry() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    query(&mut app, "Straße");
    let first = begin(&mut app);
    ui(&mut app, OttAction::CancelSearch { session: first });
    assert_eq!(ids(&app), ["one"]);
    let session = begin(&mut app);
    assert!(session > first);
    ui(&mut app, OttAction::SubmitSearch { session, value: "x".repeat(257) });
    assert_eq!(app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id, session);
    assert_eq!(ids(&app), ["one"]);
    ui(&mut app, OttAction::SubmitSearch { session, value: String::new() });
    assert_eq!(ids(&app), ["one", "two", "three"]);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn search_keeps_catalog_error_for_partial_results() {
    let _serial = plx_base::testlock::serial();
    for value in ["Straße", ""] {
        let mut app = mounted();
        let runtime = app.ott.as_ref().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(
            runtime.generation, Err("Some catalogue pages are unavailable"),
        )).unwrap();
        runtime_tick(&mut app);
        let status = app.ott.as_ref().unwrap().status.clone();
        assert_eq!(status, "Some catalogue pages are unavailable");
        assert!(!app.ott.as_ref().unwrap().catalog_complete);
        query(&mut app, value);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.status, status, "search must preserve the partial-load error");
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
        assert_eq!(runtime.view().search.as_ref().unwrap().query, value);
    }
}

#[test]
fn search_stale_cancel_does_not_consume_navigation_authority() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let old = begin(&mut app);
    ui(&mut app, OttAction::CancelSearch { session: old });
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    ui(&mut app, OttAction::CancelSearch { session: old });
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, epoch,
        "a duplicate cancellation must not revoke pending save navigation");
    let current = begin(&mut app);
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    ui(&mut app, OttAction::CancelSearch { session: old });
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.navigation_epoch, epoch);
    assert_eq!(runtime.edit_session.as_ref().unwrap().id, current);
}

#[test]
fn effective_local_owner_and_category_revoke_old_search_drafts() {
    let _serial = plx_base::testlock::serial();
    let _root_dir = super::local_integration::TempDir::new();
    let mut app = mounted();
    let permanent_page = app.ott.as_ref().unwrap().view().page_id;
    install_local_context(&mut app, 7, &_root_dir);
    let local_page = app.ott.as_ref().unwrap().view().page_id;
    assert_ne!(permanent_page, local_page);
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.active_profile.as_deref(), Some("permanent"));
    assert_eq!(app.ott.as_ref().unwrap().view().search.unwrap().scope, "Local Movies");
    let session = begin(&mut app);
    install_local_context(&mut app, 8, &_root_dir); // Same source path/profile, different accepted local owner.
    ui(&mut app, OttAction::SubmitSearch { session, value: "STRASSE".into() });
    assert_eq!(ids(&app), ["one", "two", "three"]);
    let session = begin(&mut app);
    { let runtime = app.ott.as_mut().unwrap(); assert!(transient::set_view(runtime, "Comedy", false)); runtime.changed(); }
    ui(&mut app, OttAction::SubmitSearch { session, value: "STRASSE".into() });
    assert_eq!(ids(&app), ["three"]);
    assert!(app.ott.as_ref().unwrap().view().search.unwrap().query.is_empty());
}

#[test]
fn profile_source_change_rejects_stale_query_and_settings_share_the_editor_namespace() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let stale = begin(&mut app);
    { let runtime = app.ott.as_mut().unwrap(); runtime.saved.profiles.profiles[0].source = "https://media.invalid/new.m3u".into(); runtime.changed(); }
    ui(&mut app, OttAction::SubmitSearch { session: stale, value: "STRASSE".into() });
    assert_eq!(ids(&app), ["one", "two", "three"]);
    let searching = begin(&mut app);
    ui(&mut app, OttAction::EditSetting("profile.name".into()));
    let setting = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().clone();
    assert!(setting.id > searching);
    assert_eq!(setting.key, "profile.name");
    ui(&mut app, OttAction::CancelSearch { session: setting.id });
    assert_eq!(app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id, setting.id);
    ui(&mut app, OttAction::SubmitSearch { session: setting.id, value: "changed".into() });
    assert_eq!(app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().key, "profile.name");
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.profiles[0].name, "Permanent");
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn favorites_searches_mixed_projected_titles_and_show_catalog_retains_favorites() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    { let runtime = app.ott.as_mut().unwrap();
      for (provider, title, locator) in [
        ("m3u", "News Straße", plx_data::ott::favorites::Locator::Channel { channel_id:"live".into() }),
        ("local", "Movie Straße", plx_data::ott::favorites::Locator::Local { root_alias:"Movies".into(), relative_path:"A/film.mp4".into() }),
        ("plex", "Other film", plx_data::ott::favorites::Locator::Plex { server_id:"fixture".into(), rating_key:"3".into() })] {
          runtime.saved.favorites.toggle(Favorite { title:title.into(), origin:plx_data::ott::favorites::Origin {
              provider:provider.into(), profile:"Fixture".into(), subprofile:None,
              source_alias:if provider == "local" { "Movies".into() } else { "Fixture".into() } }, locator }).unwrap();
      }
      runtime.saved.favorites_selected = true; runtime.page = "favorites"; runtime.changed(); }
    let before = saved(&app);
    // Filter out the first saved favorite: the visible ordinal must still
    // resolve the same identity through physical number entry.
    query(&mut app, "Movie");
    let runtime = app.ott.as_ref().unwrap();
    let rows = runtime.view().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "2  Movie Straße");
    assert_eq!(rows[0].content_title.as_deref(), Some("Movie Straße"));
    let numbers = commands::numbered_channels(runtime);
    assert_eq!(numbers.resolve("2"), Some((rows[0].id.clone(), "Movie Straße".into())));
    assert!(numbers.resolve("1").is_none());
    query(&mut app, "STRASSE");
    let filtered = ids(&app); assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().all(|id| id.starts_with("favorite:")));
    ui(&mut app, OttAction::ShowProviders);
    assert!(app.ott.as_ref().unwrap().view().search.is_none());
    ui(&mut app, OttAction::ShowCatalog);
    assert_eq!(app.ott.as_ref().unwrap().page, "favorites");
    assert_eq!(ids(&app), filtered);
    assert_eq!(saved(&app), before);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn search_capability_is_absent_outside_the_supported_projected_list() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    for page in ["providers", "profiles", "profile-setup", "settings", "categories", "torrent-results"] {
        let runtime = app.ott.as_mut().unwrap();
        runtime.page = page;
        assert!(runtime.view().search.is_none(), "unexpected search on {page}");
    }
    let runtime = app.ott.as_mut().unwrap();
    runtime.page = "channels";
    runtime.loaded = false;
    assert!(runtime.view().search.is_none());
}


fn invalid_search_then_retry_preserves_status(partial_catalog: bool) {
    let mut app = mounted();
    query(&mut app, "Café");
    if partial_catalog {
        let runtime = app.ott.as_ref().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(
            runtime.generation, Err("Some catalogue pages are unavailable"),
        )).unwrap();
        runtime_tick(&mut app);
    }
    let expected_status = if partial_catalog { "Some catalogue pages are unavailable" } else { "" };
    assert_eq!(app.ott.as_ref().unwrap().status, expected_status);
    let before = saved(&app);
    let generation = app.ott.as_ref().unwrap().generation;
    let session = begin(&mut app);
    let navigation_epoch = app.ott.as_ref().unwrap().navigation_epoch;

    // Cross the real Dispatcher/Bridge and Runtime error-handling boundary.
    // A direct submit_search call would miss the erroneous global status write.
    ui(&mut app, OttAction::SubmitSearch { session, value: "x".repeat(257) });
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.edit_session.as_ref().unwrap().id, session);
    assert_eq!(runtime.navigation_epoch, navigation_epoch,
        "a rejected draft must not acquire navigation authority");
    assert_eq!(runtime.view().search.as_ref().unwrap().query, "Café");
    assert_eq!(ids(&app), ["three"]);
    assert_eq!(runtime.status, expected_status,
        "Search validation must not replace the underlying catalog status");

    ui(&mut app, OttAction::SubmitSearch { session, value: "STRASSE".into() });
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.is_none());
    assert_eq!(runtime.view().search.as_ref().unwrap().query, "STRASSE");
    assert_eq!(ids(&app), ["one"]);
    assert_eq!(runtime.status, expected_status,
        "a corrected Search must not leave its old validation error visible");
    assert_eq!(runtime.view().status, expected_status);
    assert_eq!(runtime.generation, generation);
    assert_eq!(saved(&app), before);
    assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    if partial_catalog {
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
    }
}

#[test]
fn search_validation_error_does_not_leak_after_corrected_query() {
    let _serial = plx_base::testlock::serial();
    invalid_search_then_retry_preserves_status(false);
}

#[test]
fn search_validation_failure_and_retry_preserve_catalog_error() {
    let _serial = plx_base::testlock::serial();
    invalid_search_then_retry_preserves_status(true);
}

#[test]
fn clear_search_restores_current_category_without_reloading_or_saving() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    { let runtime = app.ott.as_mut().unwrap();
      runtime.saved.category = "Drama".into();
      runtime.status = "Some catalogue pages are unavailable".into();
      runtime.changed(); }
    query(&mut app, "no match");
    assert!(ids(&app).is_empty());
    let before = saved(&app);
    let runtime = app.ott.as_ref().unwrap();
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    let done = runtime.catalog_done.clone();
    let status = runtime.status.clone();
    ui(&mut app, OttAction::ClearSearch);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(ids(&app), ["one", "two"]);
    assert_eq!(runtime.page, "channels");
    assert!(runtime.view().search.unwrap().query.is_empty());
    assert_eq!(runtime.generation, generation);
    assert!(Arc::ptr_eq(&cancel, &runtime.cancel));
    assert!(Arc::ptr_eq(&done, &runtime.catalog_done));
    assert_eq!(runtime.status, status);
    assert!(runtime.catalog_next.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none() && !runtime.saving);
    assert_eq!(saved(&app), before);
}

#[test]
fn clear_search_retires_its_open_editor_and_rejects_late_submit() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    query(&mut app, "Straße");
    let stale = begin(&mut app);
    ui(&mut app, OttAction::ClearSearch);
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    ui(&mut app, OttAction::SubmitSearch { session: stale, value: "Café".into() });
    assert_eq!(ids(&app), ["one", "two", "three"]);
    assert!(app.ott.as_ref().unwrap().view().search.unwrap().query.is_empty());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn clear_search_keeps_mixed_favorites_and_their_origin_identity() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    { let runtime = app.ott.as_mut().unwrap();
      for (provider, title, locator) in [
        ("m3u", "News", plx_data::ott::favorites::Locator::Channel { channel_id: "live".into() }),
        ("local", "Movie", plx_data::ott::favorites::Locator::Local { root_alias: "Movies".into(), relative_path: "A/film.mp4".into() })] {
        runtime.saved.favorites.toggle(Favorite { title: title.into(),
            origin: plx_data::ott::favorites::Origin { provider: provider.into(), profile: "Fixture".into(),
                subprofile: None, source_alias: if provider == "local" { "Movies".into() } else { "Fixture".into() } }, locator }).unwrap();
      }
      runtime.saved.favorites_selected = true;
      runtime.page = "favorites";
      runtime.changed(); }
    let all_ids = ids(&app);
    assert_eq!(all_ids.len(), 2);
    let before = saved(&app);
    query(&mut app, "News");
    assert_eq!(ids(&app).len(), 1);
    ui(&mut app, OttAction::ClearSearch);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "favorites");
    assert!(runtime.view_selection().1);
    assert_eq!(ids(&app), all_ids);
    assert_eq!(saved(&app), before);
    assert!(runtime.play_target.is_none() && !runtime.saving);
}


#[test]
fn search_scope_localizes_favorites_and_preserves_profile_category_names() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.favorites_selected = false;
        runtime.saved.category = "Drama".into();
        runtime.page = "channels";
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let search = runtime.search_view().unwrap();
        assert_eq!(search.scope, "Permanent · Drama");
        assert_eq!(search.title(false), plx_platform::i18n::msg::ott_search_scope_title("Permanent · Drama"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        runtime.saved.favorites_selected = true;
        runtime.page = "favorites";
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let search = runtime.search_view().unwrap();
        assert_eq!(search.scope, plx_platform::i18n::msg::ott_favorites());
        assert_eq!(search.title(true), plx_platform::i18n::msg::ott_search_favorites());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn search_scope_resolves_organization_identity_and_renames_without_mutating_ownership() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    runtime.provider = ProviderKind::M3u;
    let profile = runtime.effective_profile().unwrap().clone();
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"one\" group-title=\"Drama\",One\nhttps://media.invalid/one.mp4\n",
        &profile,
    ).unwrap();
    runtime.catalog_revision += 1;
    let category = runtime.organization_projection().unwrap().categories.iter()
        .find(|row| row.label == "Drama").unwrap().id.clone();
    assert!(category.starts_with("organization:provider:"));
    runtime.saved.category = category.clone();
    let source = catalog_organization::Source::new(&profile).unwrap();
    let mut organization = catalog_organization::Document::read(&runtime.saved.settings).unwrap();
    for label in ["Drama", "Evening films"] {
        if label != "Drama" {
            organization.apply(&source, &runtime.catalog,
                catalog_organization::Operation::Rename { category: category.clone(), label: label.into() }).unwrap();
            organization.write_into(&mut runtime.saved.settings).unwrap();
        }
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let page_id = runtime.search_page_id();
        let catalog = runtime.catalog.clone();
        let search = runtime.view().search.unwrap();
        let friendly = format!("Permanent · {label}");
        assert_eq!(search.scope, friendly);
        assert_eq!(search.title(false), plx_platform::i18n::msg::ott_search_scope_title(&friendly));
        assert!(!search.scope.contains(&category));
        assert!(!search.title(false).contains("organization:"));
        assert_eq!(runtime.saved.category, category);
        assert_eq!(runtime.search_page_id(), page_id);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn search_scope_omits_unresolved_category_identity_without_erasing_saved_selection() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.category = "organization:custom:missing".into();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let page_id = runtime.search_page_id();
    let search = runtime.view().search.unwrap();
    assert_eq!(search.scope, "Permanent");
    assert_eq!(search.title(false), plx_platform::i18n::msg::ott_search_scope_title("Permanent"));
    assert!(!search.scope.contains("organization:"));
    assert_eq!(runtime.search_page_id(), page_id);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}


#[test]
fn classic_actions_search_publishes_catalogue_before_capturing_editor_return() {
    use plx_machine::machine::{Edge, InputEvent, InputKind, Key, Source, TextEdit, UiMode};

    fn press(app: &mut App, key: Key) {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
        runtime_tick(app);
        settle_pages(app);
    }

    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    // Actions admits real provider identities, not the short IDs in the general
    // search fixture. Use the existing parser and retire its previous projection.
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1,First\nhttps://media.invalid/first.ts\n\
         #EXTINF:-1,Origin\nhttps://media.invalid/origin.ts\n\
         #EXTINF:-1,Match\nhttps://media.invalid/match.ts\n", &profile,
    ).unwrap();
    runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.entropy = Some([7; 16]);
    runtime.saved.category.clear();
    runtime.saved.favorites_selected = false;
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    let origin = runtime.catalog.entries[1].id.clone();
    let matching = runtime.catalog.entries[2].id.clone();
    runtime.selected = Some(origin.clone());
    runtime.sync_saved_context();
    runtime.changed();
    app.bridge.apply_ui_mode(UiMode::Classic);
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some(origin.as_str()));
    let origin_focus = app.pages.focus().expect("mounted catalogue has a focused row");
    let owner = app.pages.top_page().unwrap();
    let before = saved(&app);
    let runtime = app.ott.as_ref().unwrap();
    let catalogue = runtime.catalog.clone();
    let generation = runtime.generation;
    let play_generation = runtime.play_generation;
    let cancel = runtime.cancel.clone();
    assert!(runtime.view().order_context.is_some(), "parsed complete catalogue offers Actions");
    {
        let parts = app.pages.parts(Tick { ms: app.loop_t, dt_us: 16_000 });
        let split = plx_ui::dispatch::Rig::<bridge::AppHost>::split(&mut app.bridge);
        let cx = parts.cx::<bridge::AppHost>(split.views, split.measure);
        let screen = &app.pages.nav.entry(origin_focus.entry).unwrap().inst.as_ref().unwrap().screen;
        assert_eq!(screen.press_kind(&origin_focus.elem, &cx), Some(plx_ui::screen::ElemKind::Card),
            "the saved FocusKey must be a catalogue row, not a footer control");
    }
    // Read the actual Yellow effect before replaying it unchanged through the
    // ordinary application drain. This proves the screen names Origin rather
    // than trusting Runtime.selected or inventing a ShowActionsAt target.
    let tick = next_tick(&mut app);
    let input = [Edge::Down, Edge::Up].into_iter().map(|edge| InputEvent {
        at: tick, source: Source::RemoteFifo,
        kind: InputKind::Key { key: Key::Other, sym: 0,
            wcode: plx_ui::consts::WCODE_YELLOW, edge, at_edge: false },
    }).collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    let requests = app.bridge.take_ott_reqs();
    let emitted_ids: Vec<_> = requests.iter().filter_map(|request| match request {
        OttOut::Action { reply, action: OttAction::ShowActionsAt { id, .. } } if *reply == owner => Some(id.as_str()),
        _ => None,
    }).collect();
    assert_eq!(emitted_ids, vec![origin.as_str()], "Yellow must capture the actual Origin row once");
    for request in requests {
        app.pages.emit(MachineId::Instance(owner), Fx::App(AppFx::Ott(request)));
    }
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "actions");
    let screen = app.pages.nav.instance(owner).unwrap().screen.as_any().unwrap()
        .downcast_ref::<plx_screens::ott::OttScreen>().unwrap();
    assert!(screen.channel_number_context("actions", runtime.revision),
        "the real Actions snapshot must be installed before opening Search");
    let search = runtime.view().rows.into_iter()
        .find(|row| row.id == "order.search" && row.enabled).expect("enabled Actions Search").action;
    ui(&mut app, search);
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert_eq!(app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().owner, owner);
    assert!(bridge::text_owns_input(&app.pages), "the actual screen editor is open");

    // Submit through the actual editor so its pending owner accepts the Runtime
    // ACK and commits the screen's search-return memory.
    let tick = next_tick(&mut app);
    let input = [TextEdit::Clear, TextEdit::Commit("Match".into())].into_iter()
        .map(|edit| InputEvent { at: tick, source: Source::Script, kind: InputKind::Text(edit) }).collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    settle_pages(&mut app);
    press(&mut app, Key::Down);
    press(&mut app, Key::Ok);
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(!bridge::text_owns_input(&app.pages));
    assert_eq!(ids(&app), [matching]);
    assert_ne!(app.pages.focus(), Some(origin_focus), "the result is a different row");

    // Without the pre-editor publication, its snapshot was still Actions:
    // no catalogue origin was captured, and clearing keeps the matching row.
    press(&mut app, Key::Back);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(runtime.view().search.unwrap().query.is_empty());
    assert_eq!(app.pages.focus(), Some(origin_focus), "Back restores the pre-Actions catalogue row");
    assert_eq!(app.pages.top_page(), Some(owner));
    assert_eq!(runtime.catalog, catalogue);
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.play_generation, play_generation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.catalog_next.is_none());
    assert_eq!(saved(&app), before);
    assert!(!plx_media::route::has_url(&app.player.session));
}


// Use the published picker action and existing revoked-writer seam. The injected
// completion below covers settlement, not real protected-storage durability.
fn stage_reselected_list(app: &mut App, favorites: bool) -> Saved {
    ui(app, OttAction::ShowCategories);
    let mut runtime = app.ott.take().unwrap();
    runtime.entropy = Some([1; 16]);
    runtime.plex_refresh_pending = true;
    let choice = if favorites { "favorites" } else { "all" };
    let action = runtime.view().rows.into_iter().find(|row| row.id == choice)
        .expect("published category picker choice").action;
    let candidate = runtime.category_choice_candidate(if favorites { None } else { Some("") })
        .unwrap();
    handle_action(&mut runtime, app, action).unwrap();
    assert!(runtime.saving && runtime.pending_category_choice.is_some());
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Job::Saved(Err(_), None, false, _)));
    app.ott = Some(runtime);
    candidate
}
fn settle_reselected_list(app: &mut App, result: Result<Saved, &'static str>) {
    app.ott.as_ref().unwrap().tx.send(Job::Saved(result, None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn explicit_all_and_favorites_reselection_clear_search_only_after_success() {
    let _serial = plx_base::testlock::serial();
    for favorites in [false, true] {
        let mut app = mounted();
        {
            let runtime = app.ott.as_mut().unwrap();
            runtime.public_preferences.accept(None);
            runtime.saved.settings.insert("interfaceMode".into(), json!(0));
            if favorites {
                runtime.saved.favorites.toggle(Favorite {
                    title: "News".into(),
                    origin: plx_data::ott::favorites::Origin { provider: "m3u".into(),
                        profile: "Permanent".into(), subprofile: None, source_alias: "Fixture".into() },
                    locator: plx_data::ott::favorites::Locator::Channel { channel_id: "one".into() },
                }).unwrap();
                runtime.saved.favorites_selected = true;
                runtime.page = "favorites";
            }
            runtime.changed();
        }
        app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
        let all = ids(&app);
        assert!(!all.is_empty());
        query(&mut app, "no matching title");
        assert!(ids(&app).is_empty());
        let runtime = app.ott.as_ref().unwrap();
        let catalog = runtime.catalog.clone();
        let document = runtime.saved.favorites.clone();
        let generation = runtime.generation;
        let cancel = runtime.cancel.clone();
        let candidate = stage_reselected_list(&mut app, favorites);
        assert_eq!(app.ott.as_ref().unwrap().page, "categories");
        settle_reselected_list(&mut app, Ok(candidate));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(ids(&app), all, "explicitly reselecting the list must reveal its rows");
        assert!(runtime.view().search.unwrap().query.is_empty());
        assert_eq!(runtime.view_selection().1, favorites);
        assert_eq!(runtime.saved.favorites, document);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.generation, generation);
        assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
        assert!(runtime.catalog_next.is_none() && runtime.resolver_next.is_none()
            && runtime.play_target.is_none() && !runtime.saving);
        assert!(!plx_media::route::has_url(&app.player.session));
    }
}

#[test]
fn failed_or_stale_explicit_all_reselection_keeps_search() {
    let _serial = plx_base::testlock::serial();
    for stale in [false, true] {
        let mut app = mounted();
        query(&mut app, "no matching title");
        let candidate = stage_reselected_list(&mut app, false);
        if stale {
            ui(&mut app, OttAction::ShowSettings);
            settle_reselected_list(&mut app, Ok(candidate));
            assert_eq!(app.ott.as_ref().unwrap().page, "settings");
        } else {
            settle_reselected_list(&mut app, Err("Fixture write refused"));
        }
        ui(&mut app, OttAction::ShowCatalog);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.view().search.unwrap().query, "no matching title");
        assert!(ids(&app).is_empty());
        assert!(runtime.pending_category_choice.is_none());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none()
            && runtime.catalog_next.is_none());
    }
}

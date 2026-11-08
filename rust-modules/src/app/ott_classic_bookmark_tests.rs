//! Pure projection tests and held-slot runtime regressions. No writer, network,
//! decoder or real device is started; supplied public snapshots model receipts.
use super::*;
use plx_data::ott::{CategoryHierarchy, CategoryNode, CategorySourceIdentity, PlexCategoryKind};

fn provenance(section: i64, key: &str) -> CategorySourceIdentity {
    CategorySourceIdentity::Plex { section, kind: PlexCategoryKind::Season, source_id: key.into() }
}
fn node(id: &str, label: &str, identity: Option<CategorySourceIdentity>) -> CategoryNode {
    CategoryNode { id: id.into(), label: label.into(), parent: None,
        anonymous_fingerprint: None, source_identity: identity }
}
fn categories(nodes: Vec<CategoryNode>) -> Catalog {
    Catalog { entries: Vec::new(), hierarchy: Some(CategoryHierarchy { nodes, memberships: Vec::new() }) }
}

#[test]
fn category_hint_uses_provider_provenance_across_new_temporary_ids_and_labels() {
    let before = categories(vec![node("classic-old:season-7", "Old title", Some(provenance(2, "7")))]);
    let after = categories(vec![node("classic-new:season-7", "Renamed title", Some(provenance(2, "7")))]);
    let digest = category_key(&before, "classic-old:season-7").unwrap().unwrap();
    assert_eq!(category_key(&before, "Old title"), Some(Some(digest.clone())));
    assert_eq!(category_key(&after, "classic-new:season-7"), Some(Some(digest.clone())));
    assert_eq!(category_id(&after, Some(&digest)).as_deref(), Some("classic-new:season-7"));
    assert_ne!(digest, "classic-old:season-7");
    assert_ne!(digest, provenance(3, "7").fingerprint(), "section is part of provider provenance");
}

#[test]
fn unknown_missing_or_ambiguous_provenance_never_falls_back_to_a_label_or_id() {
    let digest = provenance(2, "7").fingerprint();
    let unrelated = categories(vec![node(&digest, &digest, Some(provenance(2, "8")))]);
    assert_eq!(category_id(&unrelated, Some(&digest)), None);
    assert_eq!(category_key(&unrelated, "missing"), None);
    let missing = categories(vec![node("season", "Season", None)]);
    assert_eq!(category_key(&missing, "season"), None);
    assert_eq!(category_id(&missing, Some(&digest)), None);
    // Even malformed input presented directly to this helper must not select
    // an arbitrary first node; normal catalog validation rejects it earlier.
    let duplicate = categories(vec![
        node("first", "First", Some(provenance(2, "7"))),
        node("second", "Second", Some(provenance(2, "7"))),
    ]);
    assert_eq!(category_id(&duplicate, Some(&digest)), None);
    let invalid = categories(vec![node("bad", "Bad", Some(provenance(0, "7")))]);
    assert_eq!(category_key(&invalid, "bad"), None);
    assert_eq!(category_id(&invalid, Some(&provenance(0, "7").fingerprint())), None);
}

#[test]
fn explicit_all_roundtrips_without_a_catalog_or_implicit_missing_category_fallback() {
    assert_eq!(category_key(&Catalog::default(), ""), Some(None));
    assert_eq!(category_id(&Catalog::default(), None), Some(String::new()));
    assert_eq!(category_key(&Catalog::default(), "unknown"), None);
    assert_eq!(category_id(&Catalog::default(), Some(&provenance(2, "7").fingerprint())), None);
}

#[test]
fn public_bookmark_is_not_a_protected_state_overlay_or_an_arbitrary_write_door() {
    let value = ClassicBookmark { source: "a".repeat(64), category: Some("b".repeat(64)), focus: Some("42".into()) };
    let mut state = public_preferences::State::default();
    state.accept(Some(public_preferences::Snapshot {
        value: plx_platform::storage::public_preferences::Preferences {
            classic_bookmarks: vec![value.clone()], ..Default::default()
        }, revision: 4, epoch: 3,
    }));
    let before = Saved::default();
    let mut after = before.clone();
    public_preferences::overlay(&state, &mut after);
    assert_eq!(serde_json::to_value(&before).unwrap(), serde_json::to_value(&after).unwrap());
    let patch = public_preferences::Patch::ClassicBookmark(value.clone());
    assert!(public_preferences::validate_candidate(&before, &after, &patch).is_ok());
    after.category = "not a public bookmark".into();
    assert!(public_preferences::validate_candidate(&before, &after, &patch).is_err());
    after = before.clone();
    after.settings.insert("unrelatedPrivateSetting".into(), json!("must-not-cross"));
    assert!(public_preferences::validate_candidate(&before, &after, &patch).is_err());
    assert_eq!(public_preferences::classic_bookmark(&state, &value.source), Some(Some(value)));
    assert_eq!(public_preferences::classic_bookmark(&state, &"c".repeat(64)), Some(None));
    assert_eq!(public_preferences::classic_bookmark(&public_preferences::State::default(), &"a".repeat(64)), None);
}

#[cfg(feature = "hostsim")]
mod runtime {
    use super::*;
    use plx_data::ott::CategoryMembership;
    use plx_plex::plex::session::{HomeUserRef, Session, UserRef};
    use plx_session::auth::{owner::SessionInit, Phase};

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
    fn fixture() -> (Box<App>, Box<Runtime>) {
        let mut app = Box::new(run::lifecycle_regression_tests::app());
        let current = UserRef { id: 12, uuid: "home-12".into(), ..Default::default() };
        let mut init = SessionInit::captured(Session { account_token: "synthetic-account-token".into(),
            user: current.clone(), home_users: vec![
                HomeUserRef { id: 7, uuid: "owner-7".into(), admin: true, ..Default::default() },
                HomeUserRef { id: 12, uuid: "home-12".into(), ..Default::default() },
            ], ..Default::default() });
        init.active_profile = Some(current); init.phase = Phase::Ready; init.epoch = 10;
        app.bridge = bridge::Bridge::for_session_test(init);
        assert!(app.bridge.session_ready_current());
        let origin = plx_plex::plex::Origin::parse("https://bookmark.invalid:443").unwrap();
        let server = plx_plex::plex::register_pinned_with_client_id("bookmark-server", &origin,
            "synthetic-pms-token", None, "synthetic-client", Default::default());
        assert!(plx_plex::plex::set_current(server));
        let mut runtime = Box::new(Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test()));
        runtime.remote = Remote::disabled_for_test("dev_fixture", "bookmark-fixture");
        runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
        runtime.loaded = true; runtime.entropy = Some([1; 16]);
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true; runtime.plex_refresh_pending = true;
        runtime.resume_source_work();
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        assert!(runtime.classic_bookmark.view.is_some(), "real admitted account/server owns the hint");
        (app, runtime)
    }
    fn catalog(runtime: &Runtime) -> (Catalog, String, String, String) {
        let profile = classic_source::binding_profile(runtime).unwrap();
        let category = format!("{}:season-7", profile.id);
        let mut result = categories(vec![node(&category, "Season", Some(provenance(2, "7")))]);
        for key in ["42", "43"] {
            let id = format!("{}:{key}", profile.id);
            result.entries.push(plx_data::ott::CatalogEntry {
                synopsis: None,
                id: id.clone(), title: format!("Episode {key}"), category: "Season".into(),
                description: String::new(), poster: String::new(),
                media: plx_data::ott::MediaLocator::Plex { server_id: profile.source.clone(), rating_key: key.into() },
                duration_ms: Some(10_000), provider_id: Some(key.into()), provider_id_field: Some("ratingKey".into()), live: None,
            });
            result.hierarchy.as_mut().unwrap().memberships.push(CategoryMembership {
                entry_id: id, category_id: category.clone(),
            });
        }
        let first = result.entries[0].id.clone(); let second = result.entries[1].id.clone();
        (result, category, first, second)
    }
    fn snapshot(value: ClassicBookmark) -> public_preferences::Snapshot {
        public_preferences::Snapshot { value: plx_platform::storage::public_preferences::Preferences {
            classic_bookmarks: vec![value], ..Default::default()
        }, revision: 1, epoch: 1 }
    }

    #[test]
    fn new_temporary_binding_restores_only_after_the_category_arrives_without_autoplay() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (mut app, mut runtime) = fixture();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let (rows, category, first, _) = catalog(&runtime); runtime.catalog = rows;
        classic_source::set_view(&mut runtime, &category);
        record_category(&mut runtime, &category); record_focus(&mut runtime, &first);
        let hint = runtime.classic_bookmark.view.as_ref().unwrap().value.clone().unwrap();
        let old_profile = runtime.effective_profile().unwrap().id.clone();
        classic_source::release(&mut runtime);
        assert!(classic_source::ensure_catalog(&mut runtime, &mut app).unwrap());
        assert_ne!(runtime.effective_profile().unwrap().id, old_profile);
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().source, hint.source);
        runtime.public_preferences.accept(Some(snapshot(hint.clone())));
        runtime.catalog = Catalog::default(); runtime.catalog_complete = false;
        restore(&mut runtime);
        assert!(!runtime.classic_bookmark.view.as_ref().unwrap().category_applied);
        assert_eq!(runtime.view_selection().0, ""); assert_eq!(selected(&runtime), None);
        runtime.catalog_complete = true; restore(&mut runtime);
        assert!(!runtime.classic_bookmark.view.as_ref().unwrap().category_applied,
            "completed absence cannot silently turn a saved category into All");
        let (rows, category, first, _) = catalog(&runtime); runtime.catalog = rows;
        runtime.catalog_revision += 1; restore(&mut runtime);
        assert_eq!(runtime.view_selection().0, category);
        assert_eq!(selected(&runtime), Some(first));
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&hint));
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.playing.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving);
    }

    #[test]
    fn focus_records_typed_rating_key_and_rejects_foreign_media() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (_app, mut runtime) = fixture();
        let (rows, category, _, _) = catalog(&runtime); runtime.catalog = rows;
        runtime.catalog.entries[0].id = "row-display-id-not-a-rating-key".into();
        classic_source::set_view(&mut runtime, "");
        record_focus(&mut runtime, "row-display-id-not-a-rating-key");
        let hint = runtime.classic_bookmark.view.as_ref().unwrap().value.clone().unwrap();
        assert_eq!(hint.focus.as_deref(), Some("42")); assert_eq!(hint.category, None);
        runtime.catalog.entries[0].media = plx_data::ott::MediaLocator::Plex {
            server_id: "foreign-server".into(), rating_key: "43".into(),
        };
        record_focus(&mut runtime, "row-display-id-not-a-rating-key");
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&hint));
        // Unknown categories do not overwrite a known explicit All hint.
        record_category(&mut runtime, &(category + "-missing"));
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&hint));
    }

    #[test]
    fn late_ack_cannot_clear_newer_intent_or_another_source_and_failure_keeps_dirty() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (_app, mut runtime) = fixture();
        let (rows, category, first, second) = catalog(&runtime); runtime.catalog = rows;
        classic_source::set_view(&mut runtime, &category);
        record_category(&mut runtime, &category); record_focus(&mut runtime, &first);
        let old = runtime.classic_bookmark.view.as_ref().unwrap().value.clone().unwrap();
        runtime.classic_bookmark.writing = Some(Write { value: old.clone(), category_choice: false }); // model an already admitted public write
        record_focus(&mut runtime, &second);
        let latest = runtime.classic_bookmark.view.as_ref().unwrap().value.clone().unwrap();
        assert_ne!(old, latest);
        runtime.public_preferences.accept(Some(snapshot(old.clone())));
        restore(&mut runtime);
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&latest),
            "user intent supersedes a delayed public load");
        let sent = take_write(&mut runtime).unwrap();
        assert_eq!(sent.value, old); assert!(!sent.category_choice);
        assert!(take_write(&mut runtime).is_none());
        acknowledged(&mut runtime, &old, true);
        assert!(runtime.classic_bookmark.view.as_ref().unwrap().dirty);
        let mut foreign = latest.clone(); foreign.source = "f".repeat(64);
        acknowledged(&mut runtime, &foreign, true);
        acknowledged(&mut runtime, &latest, false);
        assert!(runtime.classic_bookmark.view.as_ref().unwrap().dirty);
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&latest));
        acknowledged(&mut runtime, &latest, true);
        let view = runtime.classic_bookmark.view.as_ref().unwrap();
        assert!(!view.dirty && view.due.is_none());
    }

    #[test]
    fn foreign_public_hint_and_revoked_live_authority_cannot_restore_or_publish() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (app, mut runtime) = fixture();
        let (rows, category, first, _) = catalog(&runtime); runtime.catalog = rows;
        let own = runtime.classic_bookmark.view.as_ref().unwrap().source.clone();
        let foreign = ClassicBookmark { source: "f".repeat(64),
            category: category_key(&runtime.catalog, &category).unwrap(), focus: Some("42".into()) };
        assert_ne!(foreign.source, own);
        runtime.public_preferences.accept(Some(snapshot(foreign)));
        restore(&mut runtime);
        assert_eq!(runtime.view_selection().0, ""); assert_eq!(selected(&runtime), None);
        classic_source::set_view(&mut runtime, &category);
        record_category(&mut runtime, &category); record_focus(&mut runtime, &first);
        assert!(shutdown_candidate(&runtime, &app).is_some());
        plx_plex::plex::grant::network_changed();
        assert!(shutdown_candidate(&runtime, &app).is_none());
        tick(&mut runtime, &app); // stale-authority early return, before any writer path
        assert!(runtime.classic_bookmark.view.is_none()); assert!(!runtime.saving);
    }

    #[test]
    fn public_category_dispatch_uses_public_gate_and_modeled_ack_keeps_saved_untouched() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (mut app, mut runtime) = fixture();
        let (rows, category, first, _) = catalog(&runtime); runtime.catalog = rows;
        runtime.page = "categories";
        runtime.state_load = StateLoad::Failed("protected storage unavailable");
        runtime.public_preferences.accept(None); restore(&mut runtime);
        record_focus(&mut runtime, &first);
        assert!(shutdown_candidate(&runtime, &app).is_some(), "old All-category focus is dirty");
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        assert_eq!(runtime.settings_write_refusal(), Some("protected storage unavailable"));
        assert!(public_preferences::write_refusal(&runtime).is_none());
        let value = category_patch(&runtime, &category).unwrap().unwrap();
        assert!(public_preferences::validate_candidate(&runtime.saved, &runtime.saved,
            &public_preferences::Patch::ClassicBookmark(value.clone())).is_ok());
        // Exercise the real routing branch with its writer already occupied.
        // The public refusal proves this did not fall through to the protected
        // storage failure, and prevents spawning any worker in this fixture.
        runtime.saving = true;
        assert_eq!(runtime.save_category_choice(Some(category.clone())),
            Err("Another settings operation is in progress"));
        assert!(runtime.pending_category_choice.is_none());
        assert!(take_write(&mut runtime).is_none());
        // Model the otherwise admitted public write exactly at its existing
        // Pending constructor, then run the real completion handler. This is
        // not a storage durability or successful worker-execution claim.
        runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some(&category)));
        category_started(&mut runtime, value.clone());
        assert!(shutdown_candidate(&runtime, &app).is_none(),
            "shutdown must not capture old dirty focus over the pending category write");
        runtime.status = "Saving…".into();
        assert_eq!(runtime.view_selection().0, "");
        public_preferences::saved(&mut runtime, &mut app, Ok(snapshot(value.clone())), None, Value::Null);
        assert_eq!(runtime.view_selection().0, category);
        assert_eq!(runtime.page, "channels"); assert!(runtime.status.is_empty());
        assert!(!runtime.saving && runtime.pending_category_choice.is_none());
        assert_eq!(runtime.classic_bookmark.view.as_ref().unwrap().value.as_ref(), Some(&value));
        assert!(!runtime.classic_bookmark.view.as_ref().unwrap().dirty);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(runtime.settings_write_refusal(), Some("protected storage unavailable"));
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && runtime.playing.is_none());
    }

    #[test]
    fn public_category_ack_respects_navigation_source_and_failure_guards() {
        let _serial = plx_base::testlock::serial();
        for mode in ["navigation", "source", "failure"] {
            let _registry = Registry::new();
            let (mut app, mut runtime) = fixture();
            let (rows, category, _, _) = catalog(&runtime); runtime.catalog = rows;
            runtime.page = "categories";
            runtime.public_preferences.accept(None); restore(&mut runtime);
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            let value = category_patch(&runtime, &category).unwrap().unwrap();
            runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some(&category)));
            category_started(&mut runtime, value.clone()); runtime.saving = true;
            if mode == "navigation" { runtime.navigation_epoch += 1; runtime.page = "profiles"; }
            if mode == "source" { plx_plex::plex::grant::network_changed(); }
            let result = if mode == "failure" {
                Err(public_preferences::WriteFailure::Rejected(private::Error::ProtectionUnavailable))
            } else { Ok(snapshot(value)) };
            let page = runtime.page;
            public_preferences::saved(&mut runtime, &mut app, result, None, Value::Null);
            assert_eq!(runtime.view_selection().0, "", "{mode}");
            assert_eq!(runtime.page, page, "{mode}");
            assert!(runtime.pending_category_choice.is_none()); assert!(!runtime.saving);
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved, "{mode}");
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        }
    }

    #[test]
    fn entering_or_leaving_global_favorites_still_requires_protected_write() {
        let _serial = plx_base::testlock::serial(); let _registry = Registry::new();
        let (_app, mut runtime) = fixture();
        let (rows, category, _, _) = catalog(&runtime); runtime.catalog = rows;
        runtime.state_load = StateLoad::Failed("protected storage unavailable");
        runtime.public_preferences.accept(None);
        assert!(public_preferences::write_refusal(&runtime).is_none());
        runtime.saved.favorites_selected = true;
        assert_eq!(category_patch(&runtime, &category).unwrap(), None);
        let candidate = runtime.category_choice_candidate(Some(&category)).unwrap();
        assert!(!candidate.favorites_selected && runtime.saved.favorites_selected);
        assert_eq!(runtime.save_category_choice(Some(category)), Err("protected storage unavailable"));
        assert!(runtime.saved.favorites_selected);
        runtime.saved.favorites_selected = false;
        assert_eq!(runtime.save_category_choice(None), Err("protected storage unavailable"));
        assert!(!runtime.saved.favorites_selected);
        assert!(runtime.pending_category_choice.is_none() && take_write(&mut runtime).is_none());
        assert!(!runtime.saving);
    }

}

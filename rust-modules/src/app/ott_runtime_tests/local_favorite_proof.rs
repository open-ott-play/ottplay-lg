//! Admission-level regression: a matching receiving filename is not import proof.
use super::*;
use plx_data::ott::favorites::{Locator, Origin};

fn local_profile() -> ProviderProfile {
    ProviderProfile { id: "local-one".into(), name: "Home".into(), kind: ProviderKind::Local,
        source: "/fixture/Movies".into(), source_alias: "Movies".into(), enabled: true }
}
fn entry(path: &str) -> CatalogEntry {
    let mut row = synthetic_entry(); row.id = format!("local-one:{path}"); row.title = "Clip".into();
    row.media = MediaLocator::Local { relative_path: path.into() }; row
}

#[test]
fn imported_basename_is_rejected_before_native_selection_or_resolver_admission() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.play_work_allowed(), "the actual foreground tick admitted source work");
    let directory = plx_base::testscratch::process_dir("favorite-basename-admission")
        .canonicalize().expect("owned Local fixture directory");
    let mut profile = local_profile();
    profile.source = directory.to_str().unwrap().into();
    let source = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        crate::app::ott_local_session::BoundSource {
            id: "favorite-basename-root".into(), name: "Movies".into(),
            kind: crate::app::ott_local_session::SourceKind::Folder,
            available: true, authority_ref: crate::app::ott_local_session::AuthorityRef(7),
            generation: 7, canonical_path: Some(profile.source.clone()),
        }, &mut || true,
    ).expect("owned Local directory grant");
    assert!(source.is_live());
    runtime.generation = 7;
    runtime.local_inventory.install(vec![source], runtime.generation);
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::Local; runtime.resolver_busy = true;
    runtime.catalog.entries = vec![entry("Clip.mp4"), entry("Nested/Clip.mp4")];
    let selected = runtime.local_profile_selection(&profile).expect("profile binds to its retained root");
    assert!(runtime.capture_local_source(&selected).is_some_and(|read| read.is_live()),
        "positive admission requires the actual receiving-device read grant");
    let imported = Favorite { title: "Clip".into(), origin: Origin {
        provider: "local".into(), profile: "Home".into(), subprofile: None,
        source_alias: "Movies".into() }, locator: Locator::Local {
        root_alias: "Movies".into(), relative_path: "Clip.mp4".into() } };
    runtime.saved.favorites = Document::parse(&serde_json::to_vec(&json!({"version":1,"entries":[imported]})).unwrap()).unwrap();
    let id = format!("favorite:{}", runtime.saved.favorites.entries()[0].identity());
    let before_ticket = plx_media::route::play_request_ticket();
    let before_generation = runtime.play_generation;
    assert!(selection::bind_bound(&runtime.selection_bindings().unwrap(), &runtime.saved.favorites.entries()[0]).is_ok());
    assert_eq!(runtime.play_at(&id, 0),
        Err(selection::BindingError::LocalPathUnproven.message()));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.play_generation, before_generation);
    assert!(plx_media::route::play_request_is_current(before_ticket));

    // An explicit receiving-device capture authorizes only the actual root row.
    let row = runtime.catalog.entries[0].clone();
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    assert!(runtime.saved.local_favorite_proofs.toggle_capture(&mut runtime.saved.favorites, &bindings,
        selection::SelectionOwner::Profile(&profile), &runtime.catalog.entries, &row).unwrap());
    runtime.play_at(&id, 0).unwrap();
    assert!(runtime.play_target.is_some());
    assert!(runtime.resolver_next.as_ref().unwrap().local_proof.is_some());
    assert!(runtime.resolver_next.as_ref().unwrap().local_read.as_ref()
        .is_some_and(|read| read.is_live() && read.selection == selected),
        "the queued resolver retains that same authorized root");
}

#[test]
fn native_private_save_contains_proof_but_favorites_and_profile_exports_do_not() {
    let _serial = plx_base::testlock::serial();
    let profile = local_profile(); let row = entry("Clip.mp4"); let mut saved = Saved::default();
    saved.profiles.profiles = vec![profile.clone()];
    let bindings = selection::SelectionBindings::profiles_only(&saved.profiles);
    let favorite = selection::capture_bound(&bindings, selection::SelectionOwner::Profile(&profile), &row).unwrap();
    saved.favorites.toggle(favorite).unwrap();
    saved.local_favorite_proofs.record_capture(&bindings, &saved.favorites,
        selection::SelectionOwner::Profile(&profile), &[row.clone()], &row).unwrap();
    let private = serde_json::to_value(&saved).unwrap();
    assert_eq!(private["local_favorite_proofs"]["entries"].as_array().unwrap().len(), 1);
    let exported = String::from_utf8(saved.favorites.encode().unwrap()).unwrap();
    assert!(!exported.contains("local_favorite_proofs") && !exported.contains("fingerprint"));
    let profile_doc = json!({"version":1,"provider":"offline","name":"Home",
        "binding_id":"01234567-89ab-4def-8123-456789abcdef",
        "sources":[{"key":"s0","kind":"folder","name":"Movies"}]});
    saved.settings.insert("offlineProfile:1".into(), profile_doc.clone());
    let mut app = app_fixture(); let mut runtime = app.ott.take().unwrap();
    runtime.saved = saved; runtime.provider = ProviderKind::Local;
    let mut inv = runtime.remote.request_for_test("profile_export", json!({"number":1}));
    inv.deadline_ms = u64::MAX;
    let response = commands::command(&mut runtime, &mut app, &inv).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["profile"], profile_doc);
    let profile_export = response.to_string();
    assert!(!profile_export.contains("local_favorite_proofs") && !profile_export.contains("fingerprint"));
}

#[test]
fn first_and_repeated_profile_export_require_an_explicit_save_without_starting_a_writer() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.provider = ProviderKind::Local;
    runtime.saved.profiles.profiles = vec![local_profile()];
    runtime.persistence_uncertain = true;
    runtime.write_permit.revoke();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let request = runtime.remote.request_for_test("profile_export", json!({"number":1}));
    for request_id in ["0123456789abcdef0123456789abcdef", "1123456789abcdef0123456789abcdef"] {
        let mut inv = request.clone();
        inv.id = request_id.into();
        inv.deadline_ms = u64::MAX;
        assert!(crate::app::ott_runtime::input_policy::read_only(&inv));
        let response = commands::command(&mut runtime, &mut app, &inv)
            .expect("export must finish synchronously without a storage worker");
        assert_eq!(response, rejected("Save this local profile before exporting it"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        assert!(runtime.persistence_uncertain);
        assert!(runtime.rx.try_recv().is_err());
    }
}

#[test]
fn saved_profile_exports_keep_the_binding_identity_without_storage_or_private_paths() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.provider = ProviderKind::Local;
    runtime.saved.profiles.profiles = vec![local_profile()];
    let document = json!({"version":1,"provider":"offline","name":"Home",
        "binding_id":"01234567-89ab-4def-8123-456789abcdef",
        "sources":[{"key":"s0","kind":"folder","name":"Movies"}],
        "selected":{"source_key":"s0","name":"Nested/Clip.mp4"}});
    runtime.saved.settings.insert("offlineProfile:1".into(), document.clone());
    runtime.saved.settings.insert("offlineBinding:01234567-89ab-4def-8123-456789abcdef".into(),
        json!({"root":"/fixture/Movies","sources":document["sources"]}));
    runtime.persistence_uncertain = true;
    runtime.write_permit.revoke();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let request = runtime.remote.request_for_test("profile_export", json!({"number":1}));
    for request_id in ["0123456789abcdef0123456789abcdef", "1123456789abcdef0123456789abcdef"] {
        let mut inv = request.clone();
        inv.id = request_id.into();
        inv.deadline_ms = u64::MAX;
        let response = commands::command(&mut runtime, &mut app, &inv)
            .expect("saved export must not defer to persistence");
        assert_eq!(response, ok(json!({"provider":"offline","number":1,"profile":document})));
        let text = response.to_string();
        assert!(!text.contains("/fixture/") && !text.contains("offlineBinding"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        assert!(runtime.persistence_uncertain);
        assert!(runtime.rx.try_recv().is_err());

        // The exported document still enters the existing receiving-device binder.
        // Binding supplies that device's own path without changing portable identity.
        let mut receiver = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        receiver.saved.settings.insert("offlineProfile:15".into(), response["data"]["profile"].clone());
        let bound = commands::bind_local(&receiver, 15, "/receiver/OtherFolder").unwrap();
        assert_eq!(bound.settings["offlineProfile:15"], document);
        assert!(bound.validate().is_ok());
    }
}

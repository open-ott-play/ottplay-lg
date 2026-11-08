//! Exact candidate migration and the existing writer's terminal event contract.
//! Fake completions below prove settlement, not storage durability or TV behavior.
use super::*;
use plx_data::ott::favorites::Locator;

fn profile() -> ProviderProfile {
    ProviderProfile { id: "m3u:1".into(), name: "Old label".into(), kind: ProviderKind::M3u,
        source: "https://media.invalid/fixture.m3u".into(), source_alias: "Stable alias".into(), enabled: true }
}

fn ids(saved: &mut Saved) {
    saved.favorite_items = Some(serde_json::from_value(json!({"version":1,"rows":
        saved.favorites.entries().iter().enumerate().map(|(index, favorite)| json!({
            "identity":plx_data::ott::favorites_wire::hash(favorite.identity().as_bytes()),
            "id":format!("00000000-0000-4000-8000-{:012}",index + 1)
        })).collect::<Vec<_>>()
    })).unwrap());
}

fn fixture(runtime: &mut Runtime) -> (Saved, Favorite) {
    let profile = profile();
    let catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"Family\",Fixture channel\nhttps://media.invalid/one.m3u8\n", &profile)
        .unwrap();
    let favorite = selection::capture(&profile, &catalog.entries[0]).unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.saved.favorites_selected = true;
    ids(&mut runtime.saved);
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    let selected = format!("favorite:{}", favorite.identity());
    runtime.selected = Some(selected.clone());
    runtime.checkpoints.insert("favorites".into(), SavedResume { entry_id: selected.clone(),
        position_ms: 32_100, source_fingerprint: profile_fingerprint(&profile) });
    runtime.checkpoints.insert(profile.id.clone(), SavedResume { entry_id: catalog.entries[0].id.clone(),
        position_ms: 12_345, source_fingerprint: profile_fingerprint(&profile) });
    runtime.saved.settings.insert("nativeBrowseFocusV1".into(), json!({"profiles":{
        (profile.id.clone()):{"fingerprint":profile_fingerprint(&profile),"category":"Family","entry_id":catalog.entries[0].id}
    },"favorite":null}));
    browse_focus::record(runtime, &selected);
    let mut candidate = runtime.saved.clone();
    candidate.profiles.profiles[0].name = "Home".into();
    (candidate, favorite)
}

#[test]
fn rename_candidate_preserves_uuid_resume_portability_and_exact_owner() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (mut candidate, old) = fixture(&mut runtime);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let old_ids = favorite_item_ids::ordered(&runtime.saved).unwrap();
    let plan = profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    category_state::prepare_profile_change(&runtime.saved, &mut candidate).unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before, "preparation is not publication");
    let restored: Saved = serde_json::from_value(serde_json::to_value(&candidate).unwrap()).unwrap();
    let favorite = &restored.favorites.entries()[0];
    assert_eq!(favorite.origin.profile, "Home");
    assert_eq!(favorite.origin.source_alias, "Home", "capture used the old name, not the explicit alias");
    assert_eq!(favorite.locator, old.locator);
    assert_eq!(favorite.title, old.title);
    assert_eq!(favorite_item_ids::ordered(&restored).unwrap(), old_ids);
    assert_eq!(selection::resolve_profile(&restored.profiles, favorite).unwrap().id, profile().id);
    let bindings = selection::SelectionBindings::profiles_only(&restored.profiles);
    assert!(matches!(selection::bind_bound(&bindings, favorite), Ok(selection::SelectionOwner::Profile(_))));
    assert_eq!(restored.resume["favorites"].position_ms, 32_100);
    assert_eq!(restored.resume["favorites"].entry_id, plan.selection(&format!("favorite:{}", old.identity())).unwrap());
    assert_eq!(restored.resume[&profile().id].position_ms, 12_345);
    assert_eq!(restored.resume[&profile().id].source_fingerprint, profile_fingerprint(&restored.profiles.profiles[0]));
    assert_eq!(restored.settings["nativeBrowseFocusV1"]["profiles"][&profile().id]["fingerprint"],
        profile_fingerprint(&restored.profiles.profiles[0]));
    assert!(restored.favorites_selected);
    let portable = Document::parse(&restored.favorites.encode().unwrap()).unwrap();
    assert_eq!(portable, restored.favorites);
    let mut toggled = portable;
    assert!(!toggled.toggle(favorite.clone()).unwrap(), "new capture removes rather than duplicates");
    assert!(toggled.entries().is_empty());
}

#[test]
fn rename_keeps_local_root_alias_and_only_rebinds_existing_private_proof() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let old = ProviderProfile { id: "local:1".into(), name: "Old label".into(), kind: ProviderKind::Local,
        source: "/fixture/Old label".into(), source_alias: "Old label".into(), enabled: true };
    let mut entry = synthetic_entry();
    entry.id = "actual-local-row".into();
    entry.media = MediaLocator::Local { relative_path: "Clip.mp4".into() };
    runtime.saved.profiles.profiles = vec![old.clone()];
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    let owner = selection::SelectionOwner::Profile(&old);
    let favorite = selection::capture_bound(&bindings, owner, &entry).unwrap();
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.saved.local_favorite_proofs.record_capture(&bindings, &runtime.saved.favorites,
        owner, &[entry.clone()], &entry).unwrap();
    ids(&mut runtime.saved);
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    let new = &candidate.favorites.entries()[0];
    assert_eq!(new.origin.profile, "Home");
    assert_eq!(new.origin.source_alias, "Old label");
    assert_eq!(new.locator, favorite.locator);
    let bindings = selection::SelectionBindings::profiles_only(&candidate.profiles);
    let permit = candidate.local_favorite_proofs.authorize(&bindings, &candidate.favorites,
        new, &candidate.profiles.profiles[0]).unwrap().unwrap();
    assert_eq!(permit.item_id(), entry.id);
    assert!(selection::LocalFavoriteProofs::default().authorize(&bindings, &candidate.favorites,
        new, &candidate.profiles.profiles[0]).is_err(), "the portable export did not acquire a grant");
}

#[test]
fn rename_configured_local_profile_named_local_videos_uses_actual_profile_owner() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let old = ProviderProfile { id: "local:1".into(), name: "Local videos".into(), kind: ProviderKind::Local,
        source: "/fixture/Movies".into(), source_alias: "Movies".into(), enabled: true };
    let mut entry = synthetic_entry();
    entry.id = "actual-local-row".into();
    entry.media = MediaLocator::Local { relative_path: "Clip.mp4".into() };
    runtime.saved.profiles.profiles = vec![old.clone()];
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    let owner = selection::SelectionOwner::Profile(&old);
    let favorite = selection::capture_bound(&bindings, owner, &entry).unwrap();
    assert!(matches!(selection::bind_bound(&bindings, &favorite),
        Ok(selection::SelectionOwner::Profile(profile)) if profile == &old));
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.saved.local_favorite_proofs.record_capture(&bindings, &runtime.saved.favorites,
        owner, &[entry.clone()], &entry).unwrap();
    ids(&mut runtime.saved);
    let uuids = favorite_item_ids::ordered(&runtime.saved).unwrap();
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    let new = &candidate.favorites.entries()[0];
    assert_eq!(new.origin.profile, "Home");
    assert_eq!(new.origin.source_alias, "Movies");
    assert_eq!(new.locator, favorite.locator);
    assert_eq!(favorite_item_ids::ordered(&candidate).unwrap(), uuids);
    let bindings = selection::SelectionBindings::profiles_only(&candidate.profiles);
    let permit = candidate.local_favorite_proofs.authorize(&bindings, &candidate.favorites,
        new, &candidate.profiles.profiles[0]).unwrap().unwrap();
    assert_eq!(permit.item_id(), entry.id);
}

#[test]
fn rename_retains_actual_canonical_local_root_favorite_unchanged() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    fixture(&mut runtime);
    let directory = plx_base::testscratch::process_dir("rename-canonical-local-root")
        .canonicalize().expect("owned Local root fixture");
    let root = crate::app::ott_local_session::BoundSource {
        id: "receiving-root".into(), name: "Movies".into(),
        kind: crate::app::ott_local_session::SourceKind::Folder, available: true,
        authority_ref: crate::app::ott_local_session::AuthorityRef(7), generation: 7,
        canonical_path: Some(directory.to_str().unwrap().into()),
    };
    let source = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(), root.clone(), &mut || true).unwrap();
    runtime.local_inventory.install(vec![source], 7);
    let bindings = runtime.selection_bindings().unwrap();
    let mut entry = synthetic_entry();
    entry.media = MediaLocator::Local { relative_path: "Family/Clip.mp4".into() };
    let favorite = selection::capture_bound(&bindings, selection::SelectionOwner::LocalRoot(&root), &entry).unwrap();
    assert!(matches!(selection::bind_bound(&bindings, &favorite),
        Ok(selection::SelectionOwner::LocalRoot(actual)) if actual.id == root.id
            && actual.authority_ref == root.authority_ref && actual.generation == root.generation
            && actual.canonical_path == root.canonical_path));
    assert_eq!(favorite.origin.profile, "Local videos");
    runtime.saved.favorites.toggle(favorite.clone()).unwrap(); ids(&mut runtime.saved);
    let uuids = favorite_item_ids::ordered(&runtime.saved).unwrap();
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    assert_eq!(candidate.favorites.entries()[1], favorite);
    assert_eq!(favorite_item_ids::ordered(&candidate).unwrap(), uuids);
    let bindings = selection::SelectionBindings::new(&candidate.profiles,
        runtime.local_inventory.sources.iter().map(crate::app::ott_local_root::LocalSource::descriptor), 7).unwrap();
    assert!(matches!(selection::bind_bound(&bindings, &favorite),
        Ok(selection::SelectionOwner::LocalRoot(actual)) if actual.id == root.id
            && actual.authority_ref == root.authority_ref && actual.generation == root.generation
            && actual.canonical_path == root.canonical_path));
}

#[test]
fn rename_refuses_collateral_binding_or_document_change_atomically() {
    let _serial = plx_base::testlock::serial();
    for collision in [false, true] {
        let mut runtime = runtime_fixture();
        let (_, mut missing) = fixture(&mut runtime);
        missing.origin.profile = "Home".into(); missing.origin.source_alias = "Home".into();
        if !collision { missing.locator = Locator::ChannelName { name: "Unrelated".into(), group: None }; }
        runtime.saved.favorites.toggle(missing).unwrap();
        ids(&mut runtime.saved);
        let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
        let before = serde_json::to_value(&candidate).unwrap();
        assert!(profile_rename::prepare(&runtime, &mut candidate).is_err());
        assert_eq!(serde_json::to_value(&candidate).unwrap(), before);
    }
    let mut runtime = runtime_fixture();
    let (mut candidate, old) = fixture(&mut runtime);
    candidate.favorites.remove(&old.identity());
    let before = serde_json::to_value(&candidate).unwrap();
    assert!(profile_rename::prepare(&runtime, &mut candidate).is_err());
    assert_eq!(serde_json::to_value(&candidate).unwrap(), before);
}

#[test]
fn rename_retains_unrelated_missing_favorite_and_explicit_alias() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (_, mut favorite) = fixture(&mut runtime);
    runtime.saved.favorites = Document::default();
    favorite.origin.source_alias = profile().source_alias;
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let mut missing = favorite.clone(); missing.origin.profile = "Absent".into();
    runtime.saved.favorites.toggle(missing.clone()).unwrap(); ids(&mut runtime.saved);
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    assert_eq!(candidate.favorites.entries()[0].origin.source_alias, favorite.origin.source_alias);
    assert_eq!(candidate.favorites.entries()[1], missing);
    assert_eq!(selection::resolve_profile(&candidate.profiles, &missing).err(), Some(selection::BindingError::MissingProfile));
}

#[test]
fn rename_checks_attachment_bindings_and_kiosk_origin_outside_document() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (_, mut attachment) = fixture(&mut runtime);
    attachment.origin.profile = "Home".into();
    attachment.origin.source_alias = "Home".into();
    attachment.locator = Locator::VPortal { id_field: "id".into(), id: "42".into() };
    runtime.saved.favorites.toggle(attachment).unwrap(); ids(&mut runtime.saved);
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    assert!(profile_rename::prepare(&runtime, &mut candidate).is_err(),
        "real SelectionBindings changes missing parent into missing attachment; plain resolver misses this");

    let mut runtime = runtime_fixture();
    let (_, favorite) = fixture(&mut runtime);
    let kiosk = crate::app::ott_kiosk_stored::Stored::armed(favorite.origin, true).unwrap();
    runtime.saved.settings.insert(crate::app::ott_kiosk_stored::KEY.into(), kiosk.encode().unwrap());
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    let before = serde_json::to_value(&candidate).unwrap();
    assert!(profile_rename::prepare(&runtime, &mut candidate).is_err());
    assert_eq!(serde_json::to_value(&candidate).unwrap(), before);
}

#[test]
fn rename_keeps_canonical_plex_identity_and_uuid() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let mut old = profile(); old.kind = ProviderKind::Plex; old.source = "fixture-server".into();
    let mut entry = synthetic_entry();
    entry.media = MediaLocator::Plex { server_id: old.source.clone(), rating_key: "42".into() };
    let favorite = selection::capture(&old, &entry).unwrap();
    runtime.saved.profiles.profiles = vec![old];
    runtime.saved.favorites.toggle(favorite.clone()).unwrap(); ids(&mut runtime.saved);
    let uuid = favorite_item_ids::ordered(&runtime.saved).unwrap();
    let mut candidate = runtime.saved.clone(); candidate.profiles.profiles[0].name = "Home".into();
    profile_rename::prepare(&runtime, &mut candidate).unwrap().unwrap();
    assert_eq!(candidate.favorites.entries(), &[favorite]);
    assert_eq!(favorite_item_ids::ordered(&candidate).unwrap(), uuid);
}

#[test]
fn rename_refuses_active_profile_before_starting_writer() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (mut candidate, _) = fixture(&mut runtime);
    runtime.saved.profiles.active_profile = Some(profile().id.clone());
    candidate.profiles.active_profile = runtime.saved.profiles.active_profile.clone();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.save(candidate, None, false).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving && runtime.pending_profile_rename.is_none());
}

#[test]
fn rename_reservation_blocks_only_affected_play_without_mutating_request_owner() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (mut candidate, old) = fixture(&mut runtime);
    runtime.pending_profile_rename = profile_rename::prepare(&runtime, &mut candidate).unwrap();
    runtime.saving = true;
    let generation = runtime.play_generation;
    let selected = runtime.selected.clone();
    let request = PlaybackRequest::ordinary(&format!("favorite:{}", old.identity()), 0);
    assert!(runtime.prepare_play_request(request).is_err());
    assert!(profile_rename::guard(&runtime, &profile()).is_err());
    let mut unrelated = profile(); unrelated.id = "m3u:2".into();
    assert!(profile_rename::guard(&runtime, &unrelated).is_ok());
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.selected, selected);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn rename_terminal_settlement_prevents_shutdown_final_merge_restoring_old_references() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    let (mut candidate, old) = fixture(&mut runtime);
    runtime.pending_profile_rename = profile_rename::prepare(&runtime, &mut candidate).unwrap();
    let new_id = format!("favorite:{}", candidate.favorites.entries()[0].identity());
    // A later focus observation/checkpoint still belongs to the old exact owner.
    browse_focus::record(&mut runtime, &format!("favorite:{}", old.identity()));
    runtime.checkpoints.get_mut("favorites").unwrap().position_ms = 45_600;
    profile_rename::settle(&mut runtime, Some(&candidate));
    runtime.saved = candidate;
    let mut final_image = runtime.saved.clone();
    kiosk_resume::merge(&mut final_image, &runtime.checkpoints);
    browse_focus::prepare(&runtime, &mut final_image).unwrap();
    let reloaded: Saved = serde_json::from_value(serde_json::to_value(&final_image).unwrap()).unwrap();
    assert_eq!(reloaded.resume["favorites"].entry_id, new_id);
    assert_eq!(reloaded.resume["favorites"].position_ms, 45_600);
    assert_eq!(reloaded.settings["nativeBrowseFocusV1"]["favorite"],
        plx_data::ott::favorites_wire::hash(new_id.as_bytes()));
    assert_eq!(runtime.selected.as_deref(), Some(new_id.as_str()));
    assert!(runtime.pending_profile_rename.is_none());
}

#[test]
fn rename_saved_and_rejected_events_use_actual_terminal_handler() {
    let _serial = plx_base::testlock::serial();
    for success in [false, true] {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let (mut candidate, _) = fixture(runtime);
        let before = serde_json::to_value(&runtime.saved).unwrap();
        runtime.pending_profile_rename = profile_rename::prepare(runtime, &mut candidate).unwrap();
        runtime.saving = true;
        runtime.save_navigation = SaveNavigation::Preserve;
        runtime.navigation_epoch += 1;
        runtime.selected = Some("newer-unrelated-focus".into());
        runtime.tx.send(Job::Saved(if success { Ok(candidate.clone()) } else { Err("fixture write refused") },
            None, false, json!({"status":"ok"}))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving && runtime.pending_profile_rename.is_none());
        assert_eq!(runtime.selected.as_deref(), Some("newer-unrelated-focus"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), if success { serde_json::to_value(&candidate).unwrap() } else { before });
        let original_profile = profile();
        assert_eq!(runtime.checkpoints["favorites"].source_fingerprint,
            profile_fingerprint(if success { &candidate.profiles.profiles[0] } else { &original_profile }));
    }
}

#[test]
fn rename_uncertain_event_never_publishes_usable_authority() {
    let _serial = plx_base::testlock::serial();
    for visible in [false, true] {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let (mut candidate, _) = fixture(runtime);
        let old_fingerprint = runtime.checkpoints["favorites"].source_fingerprint.clone();
        runtime.pending_profile_rename = profile_rename::prepare(runtime, &mut candidate).unwrap();
        runtime.saving = true;
        runtime.tx.send(Job::Uncertain(visible.then_some(candidate), None)).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.persistence_uncertain && !runtime.loaded);
        assert!(!runtime.saving && runtime.pending_profile_rename.is_none());
        assert_eq!(runtime.checkpoints["favorites"].source_fingerprint, old_fingerprint);
        assert!(runtime.settings_write_refusal().is_some());
    }
}

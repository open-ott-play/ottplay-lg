//! Pure after-receipt projection tests. Filesystem/worker validation remains in
//! its existing tests; these fixtures do not construct VerifiedRelocation.
use super::*;
use crate::app::ott_local_session::AuthorityRef;
use plx_data::ott::favorites::{Document, Origin};
use plx_data::ott::{CatalogEntry, MediaLocator, ProviderProfiles};
use serde_json::{json, Value};

fn descriptor(root: &BoundSource, binding: &UsbBinding) -> FolderDescriptor {
    FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: root.id.clone(),
        name: root.name.clone(),
        path: root.canonical_path.clone().unwrap(),
        usb: Some(binding.clone()),
    }
}
fn set_folder(saved: &mut Saved, root: &BoundSource, binding: &UsbBinding) {
    RememberedFolders::default()
        .with_connected(descriptor(root, binding))
        .unwrap()
        .put_into(&mut saved.settings)
        .unwrap();
}
fn configured(root: &BoundSource) -> ProviderProfile {
    ProviderProfile {
        id: "usb-profile".into(),
        name: "Offline".into(),
        kind: ProviderKind::Local,
        source: root.canonical_path.clone().unwrap(),
        source_alias: root.name.clone(),
        enabled: true,
    }
}
fn fingerprint(root: &BoundSource, configured_profile: bool) -> String {
    if configured_profile {
        return profile_fingerprint(&configured(root));
    }
    let mut profile = configured(root);
    profile.id = format!("transient:{}", root.id);
    profile.name = "Local videos".into();
    profile_fingerprint(&profile)
}
fn fixture(
    configured_profile: bool,
    path: &str,
) -> (Saved, Saved, BoundSource, BoundSource, UsbBinding) {
    let binding = UsbBinding::from_observed("ext4", "fixture-UUID", "/", "Movies").unwrap();
    let old = BoundSource {
        id: "usb-movies".into(),
        name: "Movies".into(),
        kind: SourceKind::Folder,
        available: false,
        canonical_path: Some("/tmp/usb/old/part/Movies".into()),
        authority_ref: AuthorityRef(9),
        generation: 11,
    };
    let mut new = old.clone();
    new.available = true;
    new.canonical_path = Some("/tmp/usb/new/part/Movies".into());
    let favorite = Favorite {
        title: "Fixture movie".into(),
        origin: Origin {
            provider: "local".into(),
            profile: if configured_profile {
                "Offline"
            } else {
                "Local videos"
            }
            .into(),
            subprofile: None,
            source_alias: old.name.clone(),
        },
        locator: Locator::Local {
            root_alias: old.name.clone(),
            relative_path: path.into(),
        },
    };
    let mut current = Saved::default();
    current.favorites_selected = true;
    current.favorites.toggle(favorite.clone()).unwrap();
    current.resume.insert(
        "favorites".into(),
        SavedResume {
            entry_id: format!("favorite:{}", favorite.identity()),
            position_ms: 123_456,
            source_fingerprint: fingerprint(&old, configured_profile),
        },
    );
    if configured_profile {
        current.profiles = ProviderProfiles {
            profiles: vec![configured(&old)],
            active_profile: Some("usb-profile".into()),
            ..Default::default()
        };
    }
    set_folder(&mut current, &old, &binding);
    let mut candidate = current.clone();
    if configured_profile {
        candidate.profiles.profiles[0] = configured(&new);
    }
    set_folder(&mut candidate, &new, &binding);
    (current, candidate, old, new, binding)
}
fn value(saved: &Saved) -> Value {
    serde_json::to_value(saved).unwrap()
}
fn proof(saved: &mut Saved, root: &BoundSource, configured_profile: bool, item_id: &str) {
    let mut available = root.clone();
    available.available = true;
    let profile = configured(root);
    let bindings = selection::SelectionBindings::new(
        &saved.profiles,
        if configured_profile {
            Vec::new()
        } else {
            vec![&available]
        },
        available.generation,
    )
    .unwrap();
    let owner = if configured_profile {
        selection::SelectionOwner::Profile(&profile)
    } else {
        selection::SelectionOwner::LocalRoot(&available)
    };
    let favorite = &saved.favorites.entries()[0];
    let Locator::Local { relative_path, .. } = &favorite.locator else {
        panic!("Local fixture")
    };
    let entry = CatalogEntry { synopsis: None, live: None,
        id: item_id.into(),
        title: favorite.title.clone(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        media: MediaLocator::Local {
            relative_path: relative_path.clone(),
        },
    };
    saved
        .local_favorite_proofs
        .record_capture(&bindings, &saved.favorites, owner, &[entry.clone()], &entry)
        .unwrap();
}

#[test]
fn root_and_profile_favorites_keep_identity_position_and_document() {
    for configured_profile in [false, true] {
        let (current, mut candidate, old, new, binding) =
            fixture(configured_profile, "Сериалы/第1話.mp4");
        let original = value(&current);
        let document = candidate.favorites.clone();
        let before = candidate.resume["favorites"].clone();
        project(&current, &mut candidate, &old, &new, &binding).unwrap();
        assert_eq!(candidate.resume["favorites"].entry_id, before.entry_id);
        assert_eq!(
            candidate.resume["favorites"].position_ms,
            before.position_ms
        );
        assert_eq!(
            candidate.resume["favorites"].source_fingerprint,
            fingerprint(&new, configured_profile)
        );
        assert_eq!(candidate.favorites, document);
        assert!(candidate.favorites_selected);
        assert_eq!(value(&current), original);
    }
}

#[test]
fn checkpoint_plan_is_inert_until_applied_and_keeps_latest_same_item_position() {
    let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
    let mut checkpoints = current.resume.clone();
    let expected = checkpoints.clone();
    let plan = project(&current, &mut candidate, &old, &new, &binding).unwrap();
    assert!(
        checkpoints == expected,
        "candidate preparation must not publish checkpoints"
    );
    checkpoints.get_mut("favorites").unwrap().position_ms = 234_567;
    assert!(plan.apply(&mut checkpoints));
    assert_eq!(checkpoints["favorites"].position_ms, 234_567);
    assert_eq!(
        checkpoints["favorites"].source_fingerprint,
        fingerprint(&new, false)
    );
    assert!(!plan.apply(&mut checkpoints), "applying twice is inert");
}

#[test]
fn checkpoint_plan_cannot_replace_a_newer_item_or_source() {
    let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
    let plan = project(&current, &mut candidate, &old, &new, &binding).unwrap();
    for other_item in [true, false] {
        let mut checkpoints = current.resume.clone();
        let item = checkpoints.get_mut("favorites").unwrap();
        if other_item {
            item.entry_id.push_str("other");
        } else {
            item.source_fingerprint = "a".repeat(64);
        }
        let before = checkpoints.clone();
        assert!(!plan.apply(&mut checkpoints));
        assert!(checkpoints == before);
    }
}

#[test]
fn candidate_with_newer_merged_checkpoint_keeps_its_position() {
    let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
    candidate.resume.get_mut("favorites").unwrap().position_ms = 777_000;
    project(&current, &mut candidate, &old, &new, &binding).unwrap();
    assert_eq!(candidate.resume["favorites"].position_ms, 777_000);
}

#[test]
fn repeated_projection_retains_a_usable_checkpoint_plan() {
    let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
    project(&current, &mut candidate, &old, &new, &binding).unwrap();
    let expected = value(&candidate);
    let repeated = project(&current, &mut candidate, &old, &new, &binding).unwrap();
    assert_eq!(value(&candidate), expected);
    let mut checkpoints = current.resume.clone();
    assert!(repeated.apply(&mut checkpoints));
}

#[test]
fn one_segment_favorite_needs_matching_private_proof_at_both_paths() {
    for configured_profile in [false, true] {
        for state in ["missing", "old-only", "different-item", "matched"] {
            let (mut current, mut candidate, old, new, binding) =
                fixture(configured_profile, "Clip.mp4");
            if state != "missing" {
                proof(&mut current, &old, configured_profile, "item:1");
            }
            if state == "different-item" || state == "matched" {
                proof(
                    &mut candidate,
                    &new,
                    configured_profile,
                    if state == "matched" {
                        "item:1"
                    } else {
                        "item:2"
                    },
                );
            }
            project(&current, &mut candidate, &old, &new, &binding).unwrap();
            let expected = if state == "matched" { &new } else { &old };
            assert_eq!(
                candidate.resume["favorites"].source_fingerprint,
                fingerprint(expected, configured_profile),
                "{state}"
            );
        }
    }
}

#[test]
fn unrelated_or_unproven_resume_fingerprint_is_not_rewritten() {
    for case in ["fingerprint", "entry", "nonfavorite"] {
        let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
        let resume = candidate.resume.get_mut("favorites").unwrap();
        match case {
            "fingerprint" => resume.source_fingerprint = "a".repeat(64),
            "entry" => resume.entry_id = "favorite:unknown".into(),
            _ => resume.entry_id = "ordinary-catalog-entry".into(),
        }
        let before = candidate.resume.clone();
        let plan = project(&current, &mut candidate, &old, &new, &binding).unwrap();
        assert!(candidate.resume == before);
        assert!(!plan.apply(&mut candidate.resume));
    }
}

#[test]
fn changed_favorites_or_selection_reject_atomically() {
    for selection in [false, true] {
        let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
        if selection {
            candidate.favorites_selected = false;
        } else {
            candidate.favorites = Document::default();
        }
        let before = value(&candidate);
        assert!(project(&current, &mut candidate, &old, &new, &binding).is_err());
        assert_eq!(value(&candidate), before);
    }
}

#[test]
fn duplicate_root_alias_does_not_donate_a_bookmark() {
    let (current, mut candidate, old, new, binding) = fixture(false, "Season/Clip.mp4");
    let mut second = descriptor(&new, &binding);
    second.id = "another-source".into();
    second.path = "/tmp/usb/other/part/Movies".into();
    let before = value(&candidate);
    assert_eq!(RememberedFolders::read(&candidate.settings).unwrap()
        .with_connected(second.clone()).err(),
        Some("Duplicate remembered USB folder identity"));
    assert_eq!(value(&candidate), before);

    // Model malformed persisted input directly: the supported connection API
    // correctly refuses to create this ambiguous remembered identity.
    candidate.settings.get_mut("nativeLocalSources").unwrap()["folders"]
        .as_array_mut().unwrap().push(serde_json::to_value(second).unwrap());
    let before = value(&candidate);
    assert_eq!(project(&current, &mut candidate, &old, &new, &binding).err(),
        Some("Duplicate remembered USB folder identity"));
    assert_eq!(value(&candidate), before, "ambiguous roots cannot donate a bookmark");
}

#[test]
fn changed_source_authority_or_binding_is_atomic_failure() {
    for field in ["generation", "authority", "unavailable", "binding", "path"] {
        let (current, mut candidate, old, mut new, mut binding) = fixture(false, "Season/Clip.mp4");
        match field {
            "generation" => new.generation += 1,
            "authority" => new.authority_ref = AuthorityRef(10),
            "unavailable" => new.available = false,
            "binding" => binding.uuid.push_str("-other"),
            _ => new.canonical_path = Some("/tmp/usb/other/part/Movies".into()),
        }
        let before = value(&candidate);
        assert!(
            project(&current, &mut candidate, &old, &new, &binding).is_err(),
            "{field}"
        );
        assert_eq!(value(&candidate), before);
    }
}

#[test]
fn active_category_survives_generic_profile_change_before_or_after_rebase() {
    for generic_first in [false, true] {
        let (mut current, mut candidate, old, new, binding) = fixture(true, "Season/Clip.mp4");
        current.category = "Сериалы/第1季".into();
        candidate.category.clone_from(&current.category);
        if generic_first {
            super::super::category_state::prepare_profile_change(&current, &mut candidate).unwrap();
        }
        project(&current, &mut candidate, &old, &new, &binding).unwrap();
        super::super::category_state::prepare_profile_change(&current, &mut candidate).unwrap();
        assert_eq!(candidate.category, current.category);
        assert!(candidate.favorites_selected);
    }
}

#[test]
fn remembered_missing_category_is_retained_instead_of_all_fallback() {
    let (mut current, mut candidate, old, new, binding) = fixture(true, "Season/Clip.mp4");
    let mut preferences = category_preferences::Preferences::default();
    preferences
        .choose_category(
            category_preferences::Source {
                profile_id: "usb-profile",
                fingerprint: &fingerprint(&old, true),
            },
            "Temporarily absent",
        )
        .unwrap();
    preferences.write_into(&mut current.settings).unwrap();
    preferences.write_into(&mut candidate.settings).unwrap();
    current.category.clear();
    candidate.category.clear();
    project(&current, &mut candidate, &old, &new, &binding).unwrap();
    assert_eq!(candidate.category, "Temporarily absent");
    assert!(candidate.favorites_selected);
}

#[test]
fn inactive_profile_preferences_rebase_without_changing_active_selection() {
    let (mut current, mut candidate, old, new, binding) = fixture(true, "Season/Clip.mp4");
    let other = ProviderProfile {
        id: "other".into(),
        name: "Channels".into(),
        kind: ProviderKind::M3u,
        source: "https://fixture.invalid/list.m3u".into(),
        source_alias: "Channels".into(),
        enabled: true,
    };
    current.profiles.profiles.push(other.clone());
    candidate.profiles.profiles.push(other);
    current.profiles.active_profile = Some("other".into());
    candidate.profiles.active_profile = Some("other".into());
    current.category = "News".into();
    candidate.category = "News".into();
    let mut preferences = category_preferences::Preferences::default();
    preferences
        .choose_category(
            category_preferences::Source {
                profile_id: "usb-profile",
                fingerprint: &fingerprint(&old, true),
            },
            "Series",
        )
        .unwrap();
    preferences.write_into(&mut current.settings).unwrap();
    preferences.write_into(&mut candidate.settings).unwrap();
    project(&current, &mut candidate, &old, &new, &binding).unwrap();
    let preferences = category_preferences::Preferences::read(&candidate.settings).unwrap();
    assert_eq!(
        preferences
            .remembered(category_preferences::Source {
                profile_id: "usb-profile",
                fingerprint: &fingerprint(&new, true)
            })
            .unwrap(),
        Some("Series")
    );
    assert_eq!(candidate.category, "News");
    assert_eq!(candidate.profiles.active_profile.as_deref(), Some("other"));
}

#[test]
fn profile_reconfiguration_beyond_path_does_not_donate_preferences_or_resume() {
    let (mut current, mut candidate, old, new, binding) = fixture(true, "Season/Clip.mp4");
    current.category = "Series".into();
    candidate.profiles.profiles[0].source_alias = "Different source".into();
    candidate.category = "".into();
    let before = candidate.resume.clone();
    project(&current, &mut candidate, &old, &new, &binding).unwrap();
    assert!(candidate.resume == before);
    assert!(candidate.category.is_empty());
}

#[test]
fn invalid_category_preferences_fail_before_any_bookmark_mutation() {
    let (current, mut candidate, old, new, binding) = fixture(true, "Season/Clip.mp4");
    candidate
        .settings
        .insert("nativeCategoryPreferencesV1".into(), json!({"version":99}));
    let before = value(&candidate);
    assert!(project(&current, &mut candidate, &old, &new, &binding).is_err());
    assert_eq!(value(&candidate), before);
}

#[test]
fn same_verified_usb_relocation_rebases_kiosk_point_and_only_matching_checkpoint() {
    use crate::app::ott_kiosk_stored as stored;
    let (mut current, mut candidate, old, new, binding)=fixture(false,"movie.mp4");
    let make_policy=|root:&BoundSource| stored::Stored::local_folder(
        Origin {provider:"local".into(),profile:"Local videos".into(),subprofile:None,source_alias:root.name.clone()},
        true,root.id.clone(),selection::local_root_fingerprint(root).unwrap()).unwrap();
    current.settings.insert(stored::KEY.into(),make_policy(&old).encode().unwrap());
    candidate.settings.insert(stored::KEY.into(),make_policy(&new).encode().unwrap());
    let runtime=super::super::Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    super::super::kiosk_resume::reserve(&runtime,&mut current,"same-relative-file");
    current.resume.get_mut(super::super::kiosk_resume::KEY).unwrap().position_ms=27123;
    candidate.resume=current.resume.clone();
    let original=current.resume[super::super::kiosk_resume::KEY].clone();
    let plan=project(&current,&mut candidate,&old,&new,&binding).unwrap();
    let moved=&candidate.resume[super::super::kiosk_resume::KEY];
    assert_eq!(moved.entry_id,original.entry_id);assert_eq!(moved.position_ms,27123);
    assert_ne!(moved.source_fingerprint,original.source_fingerprint);
    let mut checkpoints=current.resume.clone();
    assert!(plan.apply(&mut checkpoints));
    assert!(checkpoints[super::super::kiosk_resume::KEY]==*moved);
    let mut newer=current.resume.clone();
    newer.get_mut(super::super::kiosk_resume::KEY).unwrap().entry_id="newer-file".into();
    plan.apply(&mut newer);
    assert_eq!(newer[super::super::kiosk_resume::KEY].source_fingerprint,original.source_fingerprint);
    super::super::kiosk_resume::sanitize(&mut candidate);
    assert!(candidate.resume.contains_key(super::super::kiosk_resume::KEY));
}

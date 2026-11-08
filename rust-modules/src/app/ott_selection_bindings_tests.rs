//! Pure fixtures: all authority is synthetic and supplied by the owner. No IO.
use super::*;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};

fn root(id: &str, name: &str, authority: u64) -> BoundSource {
    BoundSource {
        id: id.into(),
        name: name.into(),
        kind: SourceKind::Folder,
        available: true,
        authority_ref: AuthorityRef(authority),
        generation: 3,
        canonical_path: Some(format!("/fixture/{id}")),
    }
}

fn media(path: &str) -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: "receiver-row".into(),
        title: "Film".into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Local {
            relative_path: path.into(),
        },
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}

fn portable(path: &str) -> Favorite {
    Favorite {
        title: "Film".into(),
        origin: Origin {
            provider: "local".into(),
            profile: "Local videos".into(),
            subprofile: None,
            source_alias: "Movies".into(),
        },
        locator: Locator::Local {
            root_alias: "Movies".into(),
            relative_path: path.into(),
        },
    }
}

fn profile(id: &str, name: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: name.into(),
        kind: ProviderKind::Local,
        source: "/fixture/named".into(),
        source_alias: "Movies".into(),
        enabled: true,
    }
}

#[test]
fn transient_root_capture_bind_index_and_toggle_use_one_global_identity() {
    let profiles = ProviderProfiles::default();
    let roots = [root("first", "Movies", 11)];
    let view = SelectionBindings::new(&profiles, &roots, 8).unwrap();
    let owner = SelectionOwner::LocalRoot(&roots[0]);
    let row = media("Family/Film.mp4");
    let favorite = capture_bound(&view, owner, &row).unwrap();
    assert_eq!(favorite, portable("Family/Film.mp4"));
    assert_eq!(
        serde_json::to_value(&favorite).unwrap(),
        serde_json::json!({
            "title":"Film", "origin":{"provider":"local", "profile":"Local videos",
            "subprofile":null, "sourceAlias":"Movies"},
            "locator":{"kind":"local-media", "rootAlias":"Movies", "relativePath":"Family/Film.mp4"}
        })
    );
    assert!(
        matches!(bind_bound(&view, &favorite), Ok(SelectionOwner::LocalRoot(r)) if r.id == "first")
    );
    let mut document = Document::default();
    assert_eq!(
        existing_identity_bound(&view, &document, owner, &row),
        Ok(None)
    );
    document.toggle(favorite.clone()).unwrap();
    let id = existing_identity_bound(&view, &document, owner, &row)
        .unwrap()
        .unwrap();
    assert_eq!(id, favorite.identity());
    let index = FavoriteIndex::new_bound(&view, &document, owner).unwrap();
    assert_eq!(index.identity(&row), Ok(Some(id.as_str())));
    assert!(!index.contains(&media("Archive/Film.mp4")));
    assert!(document.remove(&id));
    assert!(document.entries().is_empty());
    assert!(profiles.profiles.is_empty());
    assert!(profiles.active_profile.is_none());
}

#[test]
fn receiver_uses_its_own_root_without_changing_portable_identity() {
    let profiles = ProviderProfiles::default();
    let sender = [root("sender-id", "Movies", 12)];
    let receiver = [root("unrelated-receiver-id", "Movies", 99)];
    let one = SelectionBindings::new(&profiles, &sender, 1).unwrap();
    let two = SelectionBindings::new(&profiles, &receiver, 40).unwrap();
    let row = media("Family/Film.mp4");
    let favorite = capture_bound(&one, SelectionOwner::LocalRoot(&sender[0]), &row).unwrap();
    assert!(
        matches!(bind_bound(&two, &favorite), Ok(SelectionOwner::LocalRoot(r)) if r.authority_ref == AuthorityRef(99))
    );
    let recaptured = capture_bound(&two, SelectionOwner::LocalRoot(&receiver[0]), &row).unwrap();
    assert_eq!(favorite.identity(), recaptured.identity());
    assert_eq!(favorite, recaptured);
}

#[test]
fn import_without_authority_is_missing_and_never_connects_a_root() {
    let profiles = ProviderProfiles::default();
    let roots = [];
    let view = SelectionBindings::new(&profiles, &roots, 7).unwrap();
    let favorite = portable("Film.mp4");
    assert!(matches!(
        bind_bound(&view, &favorite),
        Err(BindingError::MissingProfile)
    ));
    assert!(roots.is_empty());
    assert!(profiles.profiles.is_empty());
}

#[test]
fn all_32_connections_are_independent_of_the_15_numbered_slots() {
    let profiles = ProviderProfiles {
        version: 1,
        active_profile: Some("slot-0".into()),
        profiles: (0..15)
            .map(|n| profile(&format!("slot-{n}"), &format!("Named {n}")))
            .collect(),
    };
    profiles.validate().unwrap();
    let before = profiles.clone();
    let mut roots: Vec<_> = (0..32)
        .map(|n| root(&format!("root-{n}"), &format!("Folder {n}"), n + 1))
        .collect();
    {
        let view = SelectionBindings::new(&profiles, &roots, 7).unwrap();
        for source in &roots {
            let favorite =
                capture_bound(&view, SelectionOwner::LocalRoot(source), &media("Film.mp4"))
                    .unwrap();
            assert!(
                matches!(bind_bound(&view, &favorite), Ok(SelectionOwner::LocalRoot(r)) if r.id == source.id)
            );
        }
    }
    assert_eq!(profiles, before);
    roots.push(root("root-32", "Extra", 33));
    assert!(matches!(
        SelectionBindings::new(&profiles, &roots, 8),
        Err(BindingError::SourceMismatch)
    ));
    assert_eq!(profiles, before);
}

#[test]
fn offline_duplicate_alias_remains_ambiguous_for_capture_bind_and_index() {
    let profiles = ProviderProfiles::default();
    let mut roots = [root("a", "Movies", 1), root("b", "Movies", 2)];
    roots[1].available = false;
    let view = SelectionBindings::new(&profiles, &roots, 3).unwrap();
    let row = media("Film.mp4");
    let favorite = portable("Film.mp4");
    assert!(matches!(
        bind_bound(&view, &favorite),
        Err(BindingError::AmbiguousSource)
    ));
    assert_eq!(
        capture_bound(&view, SelectionOwner::LocalRoot(&roots[0]), &row),
        Err(BindingError::AmbiguousSource)
    );
    let mut document = Document::default();
    document.toggle(favorite).unwrap();
    let owner = SelectionOwner::LocalRoot(&roots[0]);
    assert_eq!(
        existing_identity_bound(&view, &document, owner, &row),
        Err(BindingError::AmbiguousSource)
    );
    assert_eq!(
        FavoriteIndex::new_bound(&view, &document, owner)
            .unwrap()
            .identity(&row),
        Err(BindingError::AmbiguousSource)
    );
}

#[test]
fn profile_default_name_collision_cannot_deduplicate_by_equal_path() {
    let profiles = ProviderProfiles {
        version: 1,
        active_profile: None,
        profiles: vec![profile("named", "Local videos")],
    };
    let mut roots = [root("local", "Movies", 1)];
    roots[0].canonical_path = Some(profiles.profiles[0].source.clone());
    let view = SelectionBindings::new(&profiles, &roots, 1).unwrap();
    let favorite = portable("Film.mp4");
    assert!(matches!(
        bind_bound(&view, &favorite),
        Err(BindingError::AmbiguousSource)
    ));
    assert_eq!(
        capture_bound(
            &view,
            SelectionOwner::Profile(&profiles.profiles[0]),
            &media("Film.mp4")
        ),
        Err(BindingError::AmbiguousSource)
    );
}

#[test]
fn named_profile_bindings_remain_exact_and_do_not_fall_back_to_root_alias() {
    let profiles = ProviderProfiles {
        version: 1,
        active_profile: None,
        profiles: vec![profile("slot", "Home")],
    };
    let roots = [root("local", "Movies", 1)];
    let view = SelectionBindings::new(&profiles, &roots, 1).unwrap();
    let row = media("Film.mp4");
    let named = capture_bound(&view, SelectionOwner::Profile(&profiles.profiles[0]), &row).unwrap();
    assert_eq!(named.origin.profile, "Home");
    assert!(matches!(bind_bound(&view, &named), Ok(SelectionOwner::Profile(p)) if p.id == "slot"));
    let no_profiles = ProviderProfiles::default();
    let missing = SelectionBindings::new(&no_profiles, &roots, 1).unwrap();
    assert!(matches!(
        bind_bound(&missing, &named),
        Err(BindingError::MissingProfile)
    ));
    let mut old_temporary = portable("Film.mp4");
    old_temporary.origin.profile = "Temporary root".into();
    assert!(matches!(
        bind_bound(&view, &old_temporary),
        Err(BindingError::MissingProfile)
    ));
}

#[test]
fn unavailable_and_stale_owners_are_not_playable() {
    let profiles = ProviderProfiles::default();
    let old = root("local", "Movies", 1);
    let mut changed = old.clone();
    changed.generation += 1;
    let roots = [changed];
    let view = SelectionBindings::new(&profiles, &roots, 1).unwrap();
    assert_eq!(
        capture_bound(&view, SelectionOwner::LocalRoot(&old), &media("Film.mp4")),
        Err(BindingError::SourceMismatch)
    );
    let mut offline = [old.clone()];
    offline[0].available = false;
    let view = SelectionBindings::new(&profiles, &offline, 2).unwrap();
    assert!(matches!(
        bind_bound(&view, &portable("Film.mp4")),
        Err(BindingError::SourceUnavailable)
    ));

}

#[test]
fn malformed_paths_and_unportable_aliases_do_not_become_favorites() {
    let profiles = ProviderProfiles::default();
    let roots = [root("local", "Movies", 1)];
    let view = SelectionBindings::new(&profiles, &roots, 1).unwrap();
    for path in [
        "../Film.mp4",
        "/Film.mp4",
        "a//Film.mp4",
        "a/./Film.mp4",
        "C:\\Film.mp4",
    ] {
        assert_eq!(
            capture_bound(&view, SelectionOwner::LocalRoot(&roots[0]), &media(path)),
            Err(BindingError::InvalidFavorite)
        );
    }
    let long = [root("long", &"x".repeat(257), 2)];
    let view = SelectionBindings::new(&profiles, &long, 2).unwrap();
    assert_eq!(
        capture_bound(
            &view,
            SelectionOwner::LocalRoot(&long[0]),
            &media("Film.mp4")
        ),
        Err(BindingError::InvalidFavorite)
    );
}

#[test]
fn delayed_receipt_rechecks_registry_source_authority_and_ambiguity() {
    let profiles = ProviderProfiles::default();
    let roots = [root("local", "Movies", 1)];
    let view = SelectionBindings::new(&profiles, &roots, 4).unwrap();
    let favorite = portable("Film.mp4");
    let receipt: LocalBindingReceipt = view
        .receipt(SelectionOwner::LocalRoot(&roots[0]))
        .unwrap()
        .unwrap();
    assert!(view.receipt_is_current(&receipt, &favorite));
    let newer = SelectionBindings::new(&profiles, &roots, 5).unwrap();
    assert!(!newer.receipt_is_current(&receipt, &favorite));
    let mut replaced = roots.clone();
    replaced[0].authority_ref = AuthorityRef(99);
    let newer = SelectionBindings::new(&profiles, &replaced, 4).unwrap();
    assert!(!newer.receipt_is_current(&receipt, &favorite));
    replaced[0] = roots[0].clone();
    replaced[0].generation += 1;
    let newer = SelectionBindings::new(&profiles, &replaced, 4).unwrap();
    assert!(!newer.receipt_is_current(&receipt, &favorite));
    let duplicates = [roots[0].clone(), root("other", "Movies", 2)];
    let newer = SelectionBindings::new(&profiles, &duplicates, 4).unwrap();
    assert!(!newer.receipt_is_current(&receipt, &favorite));
    let missing = SelectionBindings::new(&profiles, &[], 5).unwrap();
    assert!(!missing.receipt_is_current(&receipt, &favorite));
}

#[test]
fn server_rows_require_matching_root_id_and_portable_root_proof() {
    let profiles = ProviderProfiles::default();
    let roots = [root("local", "Movies", 1)];
    let view = SelectionBindings::new(&profiles, &roots, 1).unwrap();
    let owner = SelectionOwner::LocalRoot(&roots[0]);
    let mut row = media("Film.mp4");
    row.media = MediaLocator::ServerLibrary {
        source_id: "local".into(),
        item_id: "server-only".into(),
        root_alias: Some("Movies".into()),
        relative_path: Some("Film.mp4".into()),
    };
    let favorite = capture_bound(&view, owner, &row).unwrap();
    assert_eq!(favorite, portable("Film.mp4"));
    let identity = favorite.identity();
    let mut document = Document::default();
    document.toggle(favorite).unwrap();
    let before = document.clone();
    let index = FavoriteIndex::new_bound(&view, &document, owner).unwrap();
    assert_eq!(index.identity(&row), Ok(Some(identity.as_str())));
    assert!(index.contains(&row));
    assert_eq!(
        existing_identity_bound(&view, &document, owner, &row),
        Ok(Some(identity.clone()))
    );

    // Every mismatch retains the same alias/path when possible. The wrong
    // source ID alone reproduced v1's erroneous match/removal opportunity.
    for (source_id, alias, path) in [
        ("other", Some("Movies"), Some("Film.mp4")),
        ("local", None, Some("Film.mp4")),
        ("local", Some("Other"), Some("Film.mp4")),
        ("local", Some("Movies"), None),
    ] {
        let mut foreign = row.clone();
        foreign.media = MediaLocator::ServerLibrary {
            source_id: source_id.into(),
            item_id: "server-only".into(),
            root_alias: alias.map(str::to_owned),
            relative_path: path.map(str::to_owned),
        };
        assert_eq!(
            capture_bound(&view, owner, &foreign),
            Err(BindingError::SourceMismatch)
        );
        assert_eq!(index.identity(&foreign), Err(BindingError::SourceMismatch));
        assert!(!index.contains(&foreign));
        let lookup = existing_identity_bound(&view, &document, owner, &foreign);
        assert_eq!(lookup, Err(BindingError::SourceMismatch));
        // Model the real toggle's removal branch: a refused lookup must never
        // provide an identity to delete, and the complete document survives.
        if let Ok(Some(id)) = lookup {
            document.remove(&id);
        }
        assert_eq!(document, before);
    }
    let empty = Document::default();
    let empty_index = FavoriteIndex::new_bound(&view, &empty, owner).unwrap();
    let mut foreign = row.clone();
    if let MediaLocator::ServerLibrary { source_id, .. } = &mut foreign.media {
        *source_id = "other".into();
    }
    assert_eq!(
        empty_index.identity(&foreign),
        Err(BindingError::SourceMismatch)
    );
    assert_eq!(
        existing_identity_bound(&view, &empty, owner, &foreign),
        Err(BindingError::SourceMismatch)
    );

    // Correct ownership still permits removal of precisely the saved entry.
    let id = existing_identity_bound(&view, &document, owner, &row)
        .unwrap()
        .unwrap();
    assert_eq!(id, identity);
    assert!(document.remove(&id));
    assert!(document.entries().is_empty());
}

#[test]
fn legacy_profiles_only_entry_points_keep_their_contract() {
    let profiles = ProviderProfiles {
        version: 1,
        active_profile: None,
        profiles: vec![profile("slot", "Home")],
    };
    let view = SelectionBindings::profiles_only(&profiles);
    let owner = SelectionOwner::Profile(&profiles.profiles[0]);
    let row = media("Film.mp4");
    let favorite = capture(&profiles.profiles[0], &row).unwrap();
    assert_eq!(capture_bound(&view, owner, &row), Ok(favorite.clone()));
    assert_eq!(resolve_profile(&profiles, &favorite), Ok(&profiles.profiles[0]));
    let mut document = Document::default();
    document.toggle(favorite).unwrap();
    assert_eq!(
        existing_identity(&profiles, &document, &profiles.profiles[0], &row),
        existing_identity_bound(&view, &document, owner, &row)
    );
    assert_eq!(
        FavoriteIndex::new(&profiles, &document, &profiles.profiles[0])
            .unwrap()
            .identity(&row),
        FavoriteIndex::new_bound(&view, &document, owner)
            .unwrap()
            .identity(&row)
    );
    assert!(view.receipt(owner).unwrap().is_none());
}


fn single_file_source() -> BoundSource {
    let mut source = root("one-file", "Movies", 41);
    source.kind = SourceKind::File;
    source.canonical_path = Some("/fixture/Film.mp4".into());
    source
}

#[test]
fn single_file_capture_and_binding_accept_only_the_exact_basename() {
    let profiles = ProviderProfiles::default();
    let roots = [single_file_source()];
    let bindings = SelectionBindings::new(&profiles, &roots, 9).unwrap();
    let owner = SelectionOwner::LocalRoot(&roots[0]);
    let favorite = capture_bound(&bindings, owner, &media("Film.mp4")).unwrap();
    assert_eq!(favorite, portable("Film.mp4"));
    assert!(matches!(bindings.resolve(&favorite), Ok(SelectionOwner::LocalRoot(source))
        if source.id == roots[0].id));
    for path in ["Other.mp4", "film.mp4", "Nested/Film.mp4", "%46ilm.mp4"] {
        assert!(matches!(bindings.resolve(&portable(path)), Err(BindingError::SourceMismatch)), "{path}");
        assert_eq!(capture_bound(&bindings, owner, &media(path)), Err(BindingError::SourceMismatch), "{path}");
        assert!(!bindings::owner_matches(owner, &portable(path)));
    }
}

#[test]
fn single_file_basename_preserves_unicode_case_and_literal_percent_bytes() {
    let profiles = ProviderProfiles::default();
    let mut source = single_file_source();
    source.canonical_path = Some("/fixture/éпизод%20.MKV".into());
    let roots = [source];
    let bindings = SelectionBindings::new(&profiles, &roots, 9).unwrap();
    let owner = SelectionOwner::LocalRoot(&roots[0]);
    assert!(capture_bound(&bindings, owner, &media("éпизод%20.MKV")).is_ok());
    for path in ["e\u{301}пизод%20.MKV", "éпизод .MKV", "éпизод%20.mkv"] {
        assert_eq!(capture_bound(&bindings, owner, &media(path)), Err(BindingError::SourceMismatch));
    }
}

#[test]
fn file_and_folder_alias_collisions_remain_ambiguous_even_when_offline() {
    let profiles = ProviderProfiles::default();
    for unavailable in [false, true] {
        let mut folder = root("folder", "Movies", 42);
        folder.available = !unavailable;
        let sources = [single_file_source(), folder];
        let bindings = SelectionBindings::new(&profiles, &sources, 9).unwrap();
        assert!(matches!(bindings.resolve(&portable("Film.mp4")), Err(BindingError::AmbiguousSource)));
        assert_eq!(capture_bound(&bindings, SelectionOwner::LocalRoot(&sources[0]), &media("Film.mp4")),
            Err(BindingError::AmbiguousSource));
    }
    let mut second = single_file_source();
    second.id = "different-file".into();
    second.canonical_path = Some("/fixture/Other.mp4".into());
    let sources = [single_file_source(), second];
    let bindings = SelectionBindings::new(&profiles, &sources, 9).unwrap();
    assert!(matches!(bindings.resolve(&portable("Film.mp4")), Err(BindingError::AmbiguousSource)),
        "different basenames cannot silently disambiguate the same source alias");
}

#[test]
fn single_file_delayed_receipt_includes_kind_and_exact_canonical_path() {
    let profiles = ProviderProfiles::default();
    let original = [single_file_source()];
    let bindings = SelectionBindings::new(&profiles, &original, 9).unwrap();
    let favorite = portable("Film.mp4");
    let receipt = bindings.receipt(SelectionOwner::LocalRoot(&original[0])).unwrap().unwrap();
    assert!(bindings.receipt_is_current(&receipt, &favorite));
    for field in ["kind", "path", "available", "generation", "authority"] {
        let mut changed = original.clone();
        match field {
            "kind" => changed[0].kind = SourceKind::Folder,
            "path" => changed[0].canonical_path = Some("/elsewhere/Film.mp4".into()),
            "available" => changed[0].available = false,
            "generation" => changed[0].generation += 1,
            "authority" => changed[0].authority_ref = AuthorityRef(43),
            _ => unreachable!(),
        }
        let current = SelectionBindings::new(&profiles, &changed, 9).unwrap();
        assert!(!current.receipt_is_current(&receipt, &favorite), "{field}");
    }
}

#[test]
fn known_file_cannot_be_reinterpreted_as_a_numbered_folder_profile() {
    let sources = [single_file_source()];
    let mut named = profile("named", "Home");
    named.source = sources[0].canonical_path.clone().unwrap();
    let profiles = ProviderProfiles { profiles: vec![named.clone()], ..Default::default() };
    let bindings = SelectionBindings::new(&profiles, &sources, 9).unwrap();
    let mut favorite = portable("Nested/Film.mp4");
    favorite.origin.profile = named.name.clone();
    assert!(matches!(bindings.resolve(&favorite), Err(BindingError::UnsupportedSource)));
    assert_eq!(capture_bound(&bindings, SelectionOwner::Profile(&named), &media("Film.mp4")),
        Err(BindingError::UnsupportedSource));
}

#[test]
fn malformed_or_missing_file_path_never_resolves_from_its_display_name() {
    let profiles = ProviderProfiles::default();
    for path in [None, Some("/"), Some("/fixture/../Film.mp4"), Some("relative/Film.mp4")] {
        let mut source = single_file_source();
        source.canonical_path = path.map(str::to_owned);
        let sources = [source];
        let bindings = SelectionBindings::new(&profiles, &sources, 9).unwrap();
        assert!(matches!(bindings.resolve(&portable("Film.mp4")), Err(BindingError::SourceUnavailable)));
    }
}

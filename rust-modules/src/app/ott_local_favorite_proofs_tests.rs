use super::*;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use plx_data::ott::{MediaLocator, ProviderProfiles};

fn root() -> BoundSource {
    BoundSource { id: "receiving-root".into(), name: "Movies".into(), kind: SourceKind::Folder,
        available: true, authority_ref: AuthorityRef(7), generation: 11,
        canonical_path: Some("/fixture/Movies".into()) }
}
fn profile() -> ProviderProfile {
    ProviderProfile { id: "local-one".into(), name: "Home".into(), kind: ProviderKind::Local,
        source: "/fixture/Movies".into(), source_alias: "Movies".into(), enabled: true }
}
fn transient_profile(root: &BoundSource) -> ProviderProfile {
    ProviderProfile { id: format!("transient:{}", root.id), name: "Local videos".into(),
        kind: ProviderKind::Local, source: root.canonical_path.clone().unwrap(),
        source_alias: root.name.clone(), enabled: true }
}
fn row(path: &str) -> CatalogEntry {
    CatalogEntry { id: format!("actual:{path}"), title: "Clip".into(), category: String::new(),
        description: String::new(), poster: String::new(), duration_ms: None,
        provider_id: None, provider_id_field: None,
        live: None,
        media: MediaLocator::Local { relative_path: path.into() } }
}
fn imported(root: &BoundSource, path: &str) -> (Favorite, Document) {
    let favorite = Favorite { title: "Clip".into(), origin: plx_data::ott::favorites::Origin {
        provider: "local".into(), profile: "Local videos".into(), subprofile: None,
        source_alias: root.name.clone() }, locator: Locator::Local {
            root_alias: root.name.clone(), relative_path: path.into() } };
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"entries":[favorite]})).unwrap();
    let document = Document::parse(&bytes).unwrap();
    (document.entries()[0].clone(), document)
}

#[test]
fn imported_legacy_basename_does_not_authorize_unrelated_root_file() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let (favorite, document) = imported(&sources[0], "Clip.mp4");
    let actual = row("Clip.mp4");
    assert!(super::super::matches_entry(&favorite, &actual), "baseline name match alone is unsafe");
    assert!(super::super::bind_bound(&bindings, &favorite).is_ok(), "receiving root exists");
    assert!(matches!(LocalFavoriteProofs::default().authorize(&bindings, &document, &favorite,
        &transient_profile(&sources[0])), Err(BindingError::LocalPathUnproven)));
}

#[test]
fn explicit_root_add_reattests_imported_basename_then_next_toggle_removes() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]);
    let (favorite, mut document) = imported(&sources[0], "Clip.mp4");
    let row = row("Clip.mp4"); let mut proofs = LocalFavoriteProofs::default();
    let index = super::super::FavoriteIndex::new_proven(&bindings, &document, owner, &proofs).unwrap();
    assert!(!index.contains(&row));
    assert!(proofs.toggle_capture(&mut document, &bindings, owner, &[row.clone()], &row).unwrap());
    assert_eq!(document.entries().len(), 1, "re-attestation must not duplicate the portable identity");
    assert!(super::super::FavoriteIndex::new_proven(&bindings, &document, owner, &proofs).unwrap().contains(&row));
    assert!(proofs.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])).unwrap().is_some());
    let mut replaced = row.clone(); replaced.id = "different-catalog-file".into();
    assert!(!super::super::FavoriteIndex::new_proven(&bindings, &document, owner, &proofs).unwrap().contains(&replaced));
    assert!(!proofs.toggle_capture(&mut document, &bindings, owner, &[row.clone()], &row).unwrap());
    assert!(document.entries().is_empty());
    assert!(proofs.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])).is_err());
}

#[test]
fn actual_capture_persists_private_proof_but_portable_export_drops_it() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let favorite = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_capture(&bindings, &document, owner, &[entry.clone()], &entry).unwrap();
    let restored: LocalFavoriteProofs = serde_json::from_slice(&serde_json::to_vec(&proofs).unwrap()).unwrap();
    let permit = restored.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])).unwrap().unwrap();
    assert_eq!(permit.item_id(), entry.id);
    let encoded = document.encode().unwrap();
    let exported = String::from_utf8(encoded.clone()).unwrap();
    assert!(!exported.contains("receiving-root") && !exported.contains("/fixture") && !exported.contains("item_id"));
    let received = Document::parse(&encoded).unwrap();
    assert!(matches!(LocalFavoriteProofs::default().authorize(&bindings, &received, &favorite,
        &transient_profile(&sources[0])), Err(BindingError::LocalPathUnproven)));
}

#[test]
fn full_relative_path_stays_supported_without_private_proof() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let (favorite, document) = imported(&sources[0], "Nested/Clip.mp4");
    assert!(LocalFavoriteProofs::default().authorize(&bindings, &document, &favorite,
        &transient_profile(&sources[0])).unwrap().is_none());
}

#[test]
fn changed_root_or_source_and_explicit_invalidation_cannot_reuse_proof() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let favorite = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_capture(&bindings, &document, owner, &[entry.clone()], &entry).unwrap();
    let mut changed = sources.clone(); changed[0].canonical_path = Some("/fixture/OtherMovies".into());
    let changed_bindings = SelectionBindings::new(&profiles, &changed, 12).unwrap();
    assert!(proofs.authorize(&changed_bindings, &document, &favorite, &transient_profile(&changed[0])).is_err());
    // Owner7 invokes this in its existing Saved transaction for an unknown
    // remount/replacement, even when its path and label happen to be unchanged.
    proofs.invalidate_root(&sources[0].id);
    assert!(proofs.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])).is_err());
}

#[test]
fn failed_or_ambiguous_capture_is_atomic_and_cannot_mint_proof() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let favorite = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let mut document = Document::default(); document.toggle(favorite).unwrap();
    let mut proofs = LocalFavoriteProofs::default(); let before = proofs.clone();
    assert!(proofs.record_capture(&bindings, &document, owner, &[row("Nested/Clip.mp4")], &entry).is_err());
    assert_eq!(proofs, before);
    let mut duplicate = entry.clone(); duplicate.id = "different-item".into();
    assert!(proofs.record_capture(&bindings, &document, owner, &[entry.clone(), duplicate], &entry).is_err());
    assert_eq!(proofs, before);
}

#[test]
fn profile_configuration_and_document_edits_invalidate_private_proof() {
    let profile = profile(); let profiles = ProviderProfiles { profiles: vec![profile.clone()], active_profile: Some(profile.id.clone()), ..Default::default() };
    let bindings = SelectionBindings::profiles_only(&profiles); let owner = SelectionOwner::Profile(&profile);
    let entry = row("Clip.mp4"); let favorite = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_capture(&bindings, &document, owner, &[entry.clone()], &entry).unwrap();
    assert!(proofs.authorize(&bindings, &document, &favorite, &profile).unwrap().is_some());
    let mut changed = profile.clone(); changed.source = "/fixture/OtherMovies".into();
    proofs.reconcile_profiles(&[changed]);
    assert!(proofs.authorize(&bindings, &document, &favorite, &profile).is_err());
    proofs.record_capture(&bindings, &document, owner, &[entry.clone()], &entry).unwrap();
    document.remove(&favorite.identity()); proofs.prune(&document);
    document.toggle(favorite.clone()).unwrap();
    assert!(proofs.authorize(&bindings, &document, &favorite, &profile).is_err());
}

#[test]
fn two_server_roots_with_same_basename_keep_separate_proof_item_ids() {
    let mut server = profile(); server.source = "https://library.invalid/api".into();
    let profiles = ProviderProfiles { profiles: vec![server.clone()], ..Default::default() };
    let bindings = SelectionBindings::profiles_only(&profiles); let owner = SelectionOwner::Profile(&server);
    let mut first = row("Clip.mp4"); first.id = "root-a-clip".into();
    first.media = MediaLocator::ServerLibrary { source_id: "root-a".into(), item_id: "item-a".into(),
        root_alias: Some("Root A".into()), relative_path: Some("Clip.mp4".into()) };
    let mut second = first.clone(); second.id = "root-b-clip".into();
    second.media = MediaLocator::ServerLibrary { source_id: "root-b".into(), item_id: "item-b".into(),
        root_alias: Some("Root B".into()), relative_path: Some("Clip.mp4".into()) };
    let catalog = [first.clone(), second.clone()];
    let mut document = Document::default(); let mut proofs = LocalFavoriteProofs::default();
    for entry in &catalog {
        document.toggle(super::super::capture_bound(&bindings, owner, entry).unwrap()).unwrap();
        proofs.record_capture(&bindings, &document, owner, &catalog, entry).unwrap();
    }
    let index = super::super::FavoriteIndex::new_proven(&bindings, &document, owner, &proofs).unwrap();
    assert!(index.contains(&first)); assert!(index.contains(&second));
    let mut crossed = first.clone(); crossed.id = second.id.clone();
    assert!(!index.contains(&crossed));
}

#[test]
fn kiosk_root_fingerprint_keeps_the_existing_proof_format() {
    let root = root();
    let expected = "a57b9266bd55090a21882b09c96b9b4a2f4477e89c39f096bb3c5397795406fe";
    assert_eq!(super::super::local_root_fingerprint(&root).unwrap(), expected);
    assert_eq!(source(SelectionOwner::LocalRoot(&root)).unwrap(),
        Source::Root { id: root.id.clone(), fingerprint: expected.into() });
    let mut renewed = root.clone(); renewed.generation += 1; renewed.authority_ref = AuthorityRef(91);
    assert_eq!(super::super::local_root_fingerprint(&renewed).unwrap(), expected,
        "live receipt identity is checked separately, never persisted in this hash");
    renewed.canonical_path = Some("/fixture/Replaced".into());
    assert_ne!(super::super::local_root_fingerprint(&renewed).unwrap(), expected);
    renewed.canonical_path = None;
    assert_eq!(super::super::local_root_fingerprint(&renewed), Err(BindingError::SourceUnavailable));
}

#[test]
fn kiosk_only_capture_persists_one_proof_without_adding_a_favorite() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let pin = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let pins = [pin.clone(), pin.clone()]; let document = Document::default();
    let mut proofs = LocalFavoriteProofs::default();
    assert!(matches!(proofs.authorize_pin(&bindings, &pins, &pin, &transient_profile(&sources[0])),
        Err(BindingError::LocalPathUnproven)));
    proofs.record_pin_capture(&bindings, &document, &pins, owner, &[entry.clone()], &entry).unwrap();
    proofs.record_pin_capture(&bindings, &document, &pins, owner, &[entry.clone()], &entry).unwrap();
    assert!(document.entries().is_empty()); assert_eq!(proofs.entries.len(), 1);
    let restored: LocalFavoriteProofs = serde_json::from_slice(&serde_json::to_vec(&proofs).unwrap()).unwrap();
    let permit = restored.authorize_pin(&bindings, &pins, &pin, &transient_profile(&sources[0])).unwrap().unwrap();
    assert_eq!(permit.item_id(), entry.id);
    assert!(restored.authorize(&bindings, &document, &pin, &transient_profile(&sources[0])).is_err(),
        "a kiosk proof does not insert its pin into global Favorites");
    let imported: Vec<Favorite> = serde_json::from_slice(&serde_json::to_vec(&pins).unwrap()).unwrap();
    assert!(LocalFavoriteProofs::default().authorize_pin(&bindings, &imported, &pin,
        &transient_profile(&sources[0])).is_err(), "portable pins do not import private proof");
}

#[test]
fn kiosk_proof_survives_regular_favorite_add_remove_and_prune() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let pinned = row("Pinned.mp4");
    let other = row("Other.mp4"); let catalog = [pinned.clone(), other.clone()];
    let pin = super::super::capture_bound(&bindings, owner, &pinned).unwrap();
    let pins = [pin.clone()]; let mut document = Document::default();
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_pin_capture(&bindings, &document, &pins, owner, &catalog, &pinned).unwrap();
    assert!(proofs.toggle_capture_with_pins(&mut document, &pins, &bindings, owner, &catalog, &other).unwrap());
    assert_eq!(proofs.entries.len(), 2);
    assert!(!proofs.toggle_capture_with_pins(&mut document, &pins, &bindings, owner, &catalog, &other).unwrap());
    assert!(document.entries().is_empty()); assert_eq!(proofs.entries.len(), 1);
    assert!(proofs.toggle_capture_with_pins(&mut document, &pins, &bindings, owner, &catalog, &pinned).unwrap());
    assert!(!proofs.toggle_capture_with_pins(&mut document, &pins, &bindings, owner, &catalog, &pinned).unwrap());
    proofs.prune_with_pins(&document, &pins);
    assert!(proofs.authorize_pin(&bindings, &pins, &pin, &transient_profile(&sources[0])).unwrap().is_some());
    assert_eq!(proofs.entries.len(), 1);
    proofs.prune_with_pins(&document, &[]);
    assert!(proofs.entries.is_empty(), "removing the last owner removes the private proof");
    assert!(proofs.authorize_pin(&bindings, &pins, &pin, &transient_profile(&sources[0])).is_err());
}

#[test]
fn kiosk_capture_requires_exact_pin_and_real_unique_catalog_row_atomically() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let pin = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let document = Document::default(); let mut proofs = LocalFavoriteProofs::default();
    let before = proofs.clone();
    assert!(proofs.record_pin_capture(&bindings, &document, &[], owner, &[entry.clone()], &entry).is_err());
    let mut renamed = pin.clone(); renamed.title = "Different title".into();
    assert!(proofs.record_pin_capture(&bindings, &document, &[renamed], owner, &[entry.clone()], &entry).is_err());
    assert!(proofs.record_pin_capture(&bindings, &document, &[pin.clone()], owner,
        &[row("Nested/Clip.mp4")], &entry).is_err());
    let mut duplicate = entry.clone(); duplicate.id = "second-file".into();
    assert!(proofs.record_pin_capture(&bindings, &document, &[pin], owner, &[entry.clone(), duplicate], &entry).is_err());
    assert_eq!(proofs, before);
}

#[test]
fn kiosk_authorization_rechecks_pin_membership_root_and_profile() {
    let sources = [root()]; let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]); let entry = row("Clip.mp4");
    let pin = super::super::capture_bound(&bindings, owner, &entry).unwrap();
    let pins = [pin.clone()]; let mut proofs = LocalFavoriteProofs::default();
    proofs.record_pin_capture(&bindings, &Document::default(), &pins, owner, &[entry.clone()], &entry).unwrap();
    let profile = transient_profile(&sources[0]);
    assert!(proofs.authorize_pin(&bindings, &[], &pin, &profile).is_err());
    let mut changed = sources.clone(); changed[0].canonical_path = Some("/fixture/Changed".into());
    let next = SelectionBindings::new(&profiles, &changed, 12).unwrap();
    assert!(proofs.authorize_pin(&next, &pins, &pin, &transient_profile(&changed[0])).is_err());
    changed[0] = sources[0].clone(); changed[0].available = false;
    let next = SelectionBindings::new(&profiles, &changed, 12).unwrap();
    assert!(proofs.authorize_pin(&next, &pins, &pin, &profile).is_err());
    proofs.invalidate_root(&sources[0].id);
    assert!(proofs.authorize_pin(&bindings, &pins, &pin, &profile).is_err());
    let full = row("Nested/Clip.mp4");
    let full_pin = super::super::capture_bound(&bindings, owner, &full).unwrap();
    assert!(proofs.authorize_pin(&bindings, &[full_pin.clone()], &full_pin, &profile).unwrap().is_none());
    assert!(proofs.authorize_pin(&bindings, &[], &full_pin, &profile).is_err());
    let mut wrong_profile = profile.clone(); wrong_profile.source = "/fixture/Wrong".into();
    assert!(proofs.authorize_pin(&bindings, &[full_pin.clone()], &full_pin, &wrong_profile).is_err());
}


fn file_root() -> BoundSource {
    let mut source = root();
    source.kind = SourceKind::File;
    source.canonical_path = Some("/fixture/Clip.mp4".into());
    source
}

#[test]
fn imported_single_file_requires_receiver_capture_and_keeps_portable_identity() {
    let sources = [file_root()];
    let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let owner = SelectionOwner::LocalRoot(&sources[0]);
    let (favorite, mut document) = imported(&sources[0], "Clip.mp4");
    let before = document.encode().unwrap();
    let selected = row("Clip.mp4");
    let mut proofs = LocalFavoriteProofs::default();
    assert!(matches!(proofs.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])),
        Err(BindingError::LocalPathUnproven)));
    assert!(proofs.toggle_capture(&mut document, &bindings, owner, &[selected.clone()], &selected).unwrap());
    assert_eq!(document.encode().unwrap(), before, "private re-attestation preserves the global document");
    let permit = proofs.authorize(&bindings, &document, &favorite, &transient_profile(&sources[0])).unwrap().unwrap();
    assert_eq!(permit.item_id(), selected.id);
    let received = Document::parse(&document.encode().unwrap()).unwrap();
    assert!(matches!(LocalFavoriteProofs::default().authorize(&bindings, &received, &favorite,
        &transient_profile(&sources[0])), Err(BindingError::LocalPathUnproven)));
}

#[test]
fn single_file_nested_and_sibling_paths_do_not_bypass_private_proof_checks() {
    let source = file_root();
    let owner = SelectionOwner::LocalRoot(&source);
    let proofs = LocalFavoriteProofs::default();
    for path in ["Nested/Clip.mp4", "Other.mp4", "clip.mp4"] {
        let (favorite, _) = imported(&source, path);
        assert_eq!(proofs.item_for(owner, &favorite), Err(BindingError::SourceMismatch));
    }
}

#[test]
fn single_file_and_folder_private_proof_domains_cannot_alias() {
    let file = file_root();
    let mut folder = file.clone();
    folder.kind = SourceKind::Folder;
    let file_digest = local_root_fingerprint(&file).unwrap();
    let folder_digest = local_root_fingerprint(&folder).unwrap();
    assert_ne!(file_digest, folder_digest);
    let profiles = ProviderProfiles::default();
    let sources = [file.clone()];
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let selected = row("Clip.mp4");
    let favorite = super::super::capture_bound(&bindings, SelectionOwner::LocalRoot(&sources[0]), &selected).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_capture(&bindings, &document, SelectionOwner::LocalRoot(&sources[0]), &[selected.clone()], &selected).unwrap();
    assert_eq!(proofs.item_for(SelectionOwner::LocalRoot(&folder), &favorite), Err(BindingError::LocalPathUnproven));
    let mut renewed = file.clone(); renewed.authority_ref = AuthorityRef(99); renewed.generation += 1;
    assert_eq!(local_root_fingerprint(&renewed).unwrap(), file_digest, "a hash never substitutes for a live receipt");
}

#[test]
fn single_file_proof_never_authorizes_the_parent_directory_profile() {
    let sources = [file_root()];
    let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let selected = row("Clip.mp4");
    let (favorite, document) = imported(&sources[0], "Clip.mp4");
    let mut proofs = LocalFavoriteProofs::default();
    proofs.record_capture(&bindings, &document, SelectionOwner::LocalRoot(&sources[0]), &[selected.clone()], &selected).unwrap();
    let mut parent = transient_profile(&sources[0]);
    parent.source = "/fixture".into();
    assert!(matches!(proofs.authorize(&bindings, &document, &favorite, &parent), Err(BindingError::SourceMismatch)));
}

#[test]
fn single_file_capture_rejects_wrong_row_atomically_and_requires_exact_receiver() {
    let sources = [file_root()];
    let profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&profiles, &sources, 11).unwrap();
    let selected = row("Clip.mp4");
    let (favorite, document) = imported(&sources[0], "Clip.mp4");
    let mut proofs = LocalFavoriteProofs::default();
    let before = proofs.clone();
    assert!(proofs.record_capture(&bindings, &document, SelectionOwner::LocalRoot(&sources[0]),
        &[row("Other.mp4")], &row("Other.mp4")).is_err());
    assert_eq!(proofs, before);
    proofs.record_capture(&bindings, &document, SelectionOwner::LocalRoot(&sources[0]), &[selected.clone()], &selected).unwrap();
    let mut receiver = sources.clone();
    receiver[0].id = "other-installation-source".into();
    receiver[0].canonical_path = Some("/receiver/Clip.mp4".into());
    let receiving = SelectionBindings::new(&profiles, &receiver, 12).unwrap();
    assert!(matches!(proofs.authorize(&receiving, &document, &favorite, &transient_profile(&receiver[0])),
        Err(BindingError::LocalPathUnproven)));
}

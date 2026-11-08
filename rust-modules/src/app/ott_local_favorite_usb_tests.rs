//! Real held-directory/worker-receipt tests; mount and identity observations are
//! injected, but the worker still opens and checks the actual directory FD.
use super::*;
use crate::app::ott_local_session::{AuthorityRef, SourceKind};
use crate::app::ott_local_sources::UsbBinding;
use crate::app::ott_usb::{self as usb, Answer, Task};
use plx_base::task::OffFrame;
use plx_data::ott::MediaLocator;
use plx_platform::tv::removable::{identity, MountCandidate};
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ott-proof-usb-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("Movies")).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn old(&self) -> BoundSource {
        BoundSource {
            id: "saved-usb".into(),
            name: "Movies".into(),
            kind: SourceKind::Folder,
            available: false,
            authority_ref: AuthorityRef(3),
            generation: 19,
            canonical_path: Some("/tmp/usb/old/partition/Movies".into()),
        }
    }

    fn mount(&self) -> MountCandidate {
        let device =
            identity::directory_device(&OffFrame::for_test(), &File::open(&self.0).unwrap())
                .unwrap();
        MountCandidate::directory_device_for_test(self.0.to_str().unwrap(), 7, device).unwrap()
    }

    fn restored(&self) -> (LocalSource, VerifiedRelocation) {
        let binding =
            UsbBinding::from_observed("ext4", "proof-fixture-uuid", "/", "Movies").unwrap();
        let mount = self.mount();
        let mut observations = 0;
        let answer = usb::execute_with_identity(
            &OffFrame::for_test(),
            Task::Restore {
                binding,
                source: self.old(),
            },
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![mount.clone()]),
            &mut |_, mounts, _| {
                observations += 1;
                Ok(mounts
                    .iter()
                    .map(|mount| {
                        identity::IdentityObservation::for_test(
                            mount.clone(),
                            identity::StableIdentity::new("proof-fixture-uuid", mount.filesystem())
                                .unwrap(),
                        )
                    })
                    .collect())
            },
        )
        .unwrap();
        assert_eq!(
            observations, 2,
            "receipt requires observations before and after the real root open"
        );
        let Answer::Restored { source, relocation } = answer else {
            panic!("restored receipt");
        };
        assert!(relocation.is_current(&source));
        (source, relocation)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn profiles(path: &str) -> ProviderProfiles {
    ProviderProfiles {
        profiles: vec![ProviderProfile {
            id: "local-one".into(),
            name: "Home".into(),
            kind: ProviderKind::Local,
            source: path.into(),
            source_alias: "Movies".into(),
            enabled: true,
        }],
        ..Default::default()
    }
}

fn row(path: &str) -> CatalogEntry {
    CatalogEntry { live: None,
        id: format!("captured:{path}"),
        title: path.into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        media: MediaLocator::Local {
            relative_path: path.into(),
        },
    }
}

/// Capture through the existing catalog admission path; no handcrafted proof.
fn captured(old: &BoundSource, profiles: &ProviderProfiles) -> (LocalFavoriteProofs, Document) {
    let mut admitted = old.clone();
    admitted.available = true;
    let bindings = SelectionBindings::new(profiles, [&admitted], old.generation).unwrap();
    let mut proofs = LocalFavoriteProofs::default();
    let mut document = Document::default();
    for (owner, entry) in [
        (SelectionOwner::LocalRoot(&admitted), row("Root.mp4")),
        (
            SelectionOwner::Profile(&profiles.profiles[0]),
            row("Profile.mp4"),
        ),
    ] {
        document
            .toggle(super::super::capture_bound(&bindings, owner, &entry).unwrap())
            .unwrap();
        proofs
            .record_capture(&bindings, &document, owner, &[entry.clone()], &entry)
            .unwrap();
    }
    (proofs, document)
}

fn after_profiles(current: &LocalSource) -> ProviderProfiles {
    profiles(current.descriptor().canonical_path.as_deref().unwrap())
}

#[test]
fn real_usb_receipt_restores_exact_root_and_path_only_profile_proofs() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let new_profiles = after_profiles(&current);
    let (baseline, document) = captured(&f.old(), &old_profiles);
    let mut candidate = baseline.clone();
    candidate.invalidate_root(&f.old().id);
    candidate.reconcile_profiles(&new_profiles.profiles);
    assert!(
        candidate.entries.is_empty(),
        "ordinary invalidation runs before restoration"
    );
    assert_eq!(
        candidate.restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &old_profiles,
            &new_profiles
        ),
        Ok(2)
    );
    for (before, after) in baseline.entries.iter().zip(&candidate.entries) {
        assert_eq!(before.favorite, after.favorite);
        assert_eq!(before.item_id, after.item_id);
        assert_ne!(before.source, after.source);
    }
    assert_eq!(
        candidate.entries[0].source,
        source(SelectionOwner::LocalRoot(current.descriptor())).unwrap()
    );
    assert_eq!(
        candidate.entries[1].source,
        source(SelectionOwner::Profile(&new_profiles.profiles[0])).unwrap()
    );
    candidate.prune_with_pins(&document, &[]);
    candidate.reconcile_profiles(&new_profiles.profiles);
    assert_eq!(candidate.entries.len(), 2);
    let stable = candidate.clone();
    assert_eq!(
        candidate.restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &old_profiles,
            &new_profiles
        ),
        Ok(0)
    );
    assert_eq!(
        candidate, stable,
        "repeated receipt never duplicates or recaptures"
    );
}

#[test]
fn real_usb_receipt_never_mints_missing_proofs() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let mut candidate = LocalFavoriteProofs::default();
    assert_eq!(
        candidate.restore_verified_relocation(
            &LocalFavoriteProofs::default(),
            &receipt,
            &current,
            &profiles,
            &after_profiles(&current)
        ),
        Ok(0)
    );
    assert!(candidate.entries.is_empty());
}

#[test]
fn real_usb_receipt_does_not_restore_other_root_fingerprints_or_changed_profiles() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let (mut baseline, _) = captured(&f.old(), &old_profiles);
    let mut other_root = f.old();
    other_root.canonical_path = Some("/tmp/usb/other/Movies".into());
    baseline.entries[0].source = source(SelectionOwner::LocalRoot(&other_root)).unwrap();
    for changed in ["name", "alias", "enabled", "path"] {
        let mut new_profiles = after_profiles(&current);
        let profile = &mut new_profiles.profiles[0];
        match changed {
            "name" => profile.name = "Changed".into(),
            "alias" => profile.source_alias = "Other".into(),
            "enabled" => profile.enabled = false,
            "path" => profile.source.push_str("/other"),
            _ => unreachable!(),
        }
        let mut candidate = LocalFavoriteProofs::default();
        assert_eq!(
            candidate.restore_verified_relocation(
                &baseline,
                &receipt,
                &current,
                &old_profiles,
                &new_profiles
            ),
            Ok(0),
            "{changed}"
        );
        assert!(candidate.entries.is_empty());
    }
}

#[test]
fn real_usb_receipt_preserves_newer_conflicting_candidate_attestation() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let (baseline, _) = captured(&f.old(), &old_profiles);
    let mut candidate = baseline.clone();
    candidate.entries[0].item_id = "newer:Root.mp4".into();
    candidate.entries[1].favorite.title = "Renamed".into();
    let expected = candidate.clone();
    assert_eq!(
        candidate.restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &old_profiles,
            &after_profiles(&current)
        ),
        Ok(0)
    );
    assert_eq!(candidate, expected);
}

#[test]
fn real_usb_receipt_validates_whole_documents_before_any_mutation() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let (baseline, _) = captured(&f.old(), &old_profiles);
    let mut broken_baseline = baseline.clone();
    broken_baseline.entries[1].item_id.clear();
    let mut candidate = LocalFavoriteProofs::default();
    let expected = candidate.clone();
    assert!(candidate
        .restore_verified_relocation(
            &broken_baseline,
            &receipt,
            &current,
            &old_profiles,
            &after_profiles(&current)
        )
        .is_err());
    assert_eq!(candidate, expected);
    let mut broken_candidate = baseline.clone();
    broken_candidate.entries[1].item_id.clear();
    let expected = broken_candidate.clone();
    assert!(broken_candidate
        .restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &old_profiles,
            &after_profiles(&current)
        )
        .is_err());
    assert_eq!(broken_candidate, expected);
    let mut invalid_profiles = old_profiles.clone();
    invalid_profiles
        .profiles
        .push(old_profiles.profiles[0].clone());
    assert!(candidate
        .restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &invalid_profiles,
            &after_profiles(&current)
        )
        .is_err());
    assert_eq!(candidate, LocalFavoriteProofs::default());
}

#[test]
fn real_usb_receipt_rejects_another_live_handle_and_a_withdrawn_root_atomically() {
    let f = Fixture::new();
    let (mut current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let new_profiles = after_profiles(&current);
    let (baseline, _) = captured(&f.old(), &old_profiles);
    let another = LocalSource::acquire_authorized(
        &OffFrame::for_test(),
        current.descriptor().clone(),
        &mut || true,
    )
    .unwrap();
    let mut candidate = LocalFavoriteProofs::default();
    assert!(candidate
        .restore_verified_relocation(&baseline, &receipt, &another, &old_profiles, &new_profiles)
        .is_err());
    assert_eq!(candidate, LocalFavoriteProofs::default());
    current.withdraw();
    assert!(candidate
        .restore_verified_relocation(&baseline, &receipt, &current, &old_profiles, &new_profiles)
        .is_err());
    assert_eq!(candidate, LocalFavoriteProofs::default());
}

#[test]
fn restored_proofs_still_obey_document_pins_and_profile_pruning() {
    let f = Fixture::new();
    let (current, receipt) = f.restored();
    let old_profiles = profiles(f.old().canonical_path.as_deref().unwrap());
    let new_profiles = after_profiles(&current);
    let (baseline, _) = captured(&f.old(), &old_profiles);
    let mut candidate = LocalFavoriteProofs::default();
    candidate
        .restore_verified_relocation(&baseline, &receipt, &current, &old_profiles, &new_profiles)
        .unwrap();
    let pin = baseline.entries[1].favorite.clone();
    candidate.prune_with_pins(&Document::default(), &[pin.clone()]);
    assert_eq!(candidate.entries.len(), 1);
    assert_eq!(candidate.entries[0].favorite, pin);
    candidate.reconcile_profiles(&[]);
    assert!(
        candidate.entries.is_empty(),
        "removed profile cannot retain a rebased proof"
    );
}

fn intent(root: &BoundSource) -> FolderDescriptor {
    FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: root.id.clone(),
        name: root.name.clone(),
        path: root.canonical_path.clone().unwrap(),
        usb: Some(UsbBinding::from_observed("ext4", "proof-fixture-uuid", "/", "Movies").unwrap()),
    }
}

fn remembered(folder: FolderDescriptor) -> RememberedFolders {
    RememberedFolders::default().with_connected(folder).unwrap()
}

#[test]
fn detached_exact_stable_intent_survives_but_unavailable_root_still_cannot_bind() {
    let f = Fixture::new();
    let old = f.old();
    let old_profiles = profiles(old.canonical_path.as_deref().unwrap());
    let (baseline, document) = captured(&old, &old_profiles);
    let folder = intent(&old);
    let folders = remembered(folder.clone());
    let mut candidate = baseline.clone();
    candidate.invalidate_unavailable_root(&old, Some(&folder), Some(&folders));
    assert_eq!(candidate, baseline);
    let empty_profiles = ProviderProfiles::default();
    let bindings = SelectionBindings::new(&empty_profiles, [&old], old.generation).unwrap();
    let transient = ProviderProfile {
        id: "transient:saved-usb".into(),
        name: "Local videos".into(),
        kind: ProviderKind::Local,
        source: folder.path.clone(),
        source_alias: folder.name.clone(),
        enabled: true,
    };
    assert!(
        matches!(
            candidate.authorize(
                &bindings,
                &document,
                &baseline.entries[0].favorite,
                &transient
            ),
            Err(BindingError::SourceUnavailable)
        ),
        "durable proof intent is not a live grant"
    );
    let (current, receipt) = f.restored();
    assert_eq!(
        candidate.restore_verified_relocation(
            &baseline,
            &receipt,
            &current,
            &old_profiles,
            &after_profiles(&current)
        ),
        Ok(2)
    );
}

#[test]
fn detached_changed_removed_or_malformed_candidate_intent_revokes_only_that_root() {
    let f = Fixture::new();
    let old = f.old();
    let (baseline, _) = captured(&old, &profiles(old.canonical_path.as_deref().unwrap()));
    let folder = intent(&old);
    for change in [
        "id",
        "name",
        "path",
        "uuid",
        "filesystem",
        "relative",
        "removed",
        "invalid",
    ] {
        let mut changed = folder.clone();
        match change {
            "id" => changed.id.push_str("-other"),
            "name" => changed.name.push_str(" other"),
            "path" => changed.path.push_str("/other"),
            "uuid" => changed.usb.as_mut().unwrap().uuid.push_str("-other"),
            "filesystem" => changed.usb.as_mut().unwrap().filesystem_type = "vfat".into(),
            "relative" => changed
                .usb
                .as_mut()
                .unwrap()
                .filesystem_relative_folder
                .push_str("/other"),
            "removed" | "invalid" => {}
            _ => unreachable!(),
        }
        let folders = if change == "removed" {
            RememberedFolders::default()
        } else {
            remembered(changed)
        };
        let mut candidate = baseline.clone();
        candidate.invalidate_unavailable_root(
            &old,
            Some(&folder),
            (change != "invalid").then_some(&folders),
        );
        assert_eq!(
            candidate.entries,
            vec![baseline.entries[1].clone()],
            "{change}"
        );
    }
}

#[test]
fn detached_stable_intent_does_not_retain_wrong_fingerprint_or_legacy_folder() {
    let f = Fixture::new();
    let old = f.old();
    let (baseline, _) = captured(&old, &profiles(old.canonical_path.as_deref().unwrap()));
    let folder = intent(&old);
    let mut candidate = baseline.clone();
    let mut wrong = old.clone();
    wrong.canonical_path = Some("/tmp/usb/other/Movies".into());
    candidate.entries[0].source = source(SelectionOwner::LocalRoot(&wrong)).unwrap();
    candidate.invalidate_unavailable_root(&old, Some(&folder), Some(&remembered(folder.clone())));
    assert_eq!(candidate.entries, vec![baseline.entries[1].clone()]);
    for retained in [
        None,
        Some(FolderDescriptor {
            usb: None,
            ..folder.clone()
        }),
    ] {
        let mut candidate = baseline.clone();
        let folders = remembered(retained.clone().unwrap_or_else(|| folder.clone()));
        candidate.invalidate_unavailable_root(&old, retained.as_ref(), Some(&folders));
        assert_eq!(candidate.entries, vec![baseline.entries[1].clone()]);
    }
}

//! Pure candidate projection tests, not authorization or filesystem tests.
//! Only the production wrapper accepts a worker-created VerifiedRelocation.
use super::*;
use crate::app::ott_local_session::AuthorityRef;
use plx_data::ott::favorites::{Favorite, Origin};
use serde_json::{json, Value};

fn origin() -> Origin {
    Origin {
        provider: "local".into(),
        profile: "Local videos".into(),
        subprofile: None,
        source_alias: "Movies".into(),
    }
}

fn descriptor(root: &BoundSource, binding: &UsbBinding) -> FolderDescriptor {
    FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: root.id.clone(),
        name: root.name.clone(),
        path: root.canonical_path.clone().unwrap(),
        usb: Some(binding.clone()),
    }
}

fn folders(saved: &mut Saved, folder: FolderDescriptor) {
    RememberedFolders::default()
        .with_connected(folder).unwrap()
        .put_into(&mut saved.settings).unwrap();
}

fn fixture() -> (Saved, Saved, BoundSource, BoundSource, UsbBinding) {
    let binding = UsbBinding::from_observed("ext4", "ABCD-1234", "/", "Movies").unwrap();
    let old = BoundSource {
        id: "usb-movies".into(), name: "Movies".into(), kind: SourceKind::Folder,
        available: false, authority_ref: AuthorityRef(9), generation: 11,
        canonical_path: Some("/tmp/usb/old/part/Movies".into()),
    };
    let mut new = old.clone();
    new.canonical_path = Some("/tmp/usb/new/part/Movies".into());
    new.available = true;
    let mut current = Saved::default();
    folders(&mut current, descriptor(&old, &binding));
    current.settings.insert(stored::KEY.into(), Stored::local_folder(
        origin(), true, old.id.clone(), selection::local_root_fingerprint(&old).unwrap(),
    ).unwrap().encode().unwrap());
    let mut candidate = current.clone();
    folders(&mut candidate, descriptor(&new, &binding));
    (current, candidate, old, new, binding)
}

fn value(saved: &Saved) -> Value {
    serde_json::to_value(saved).unwrap()
}

#[test]
fn verified_usb_projection_rebases_only_exact_local_folder_fingerprint() {
    for strict in [false, true] {
        let (mut current, mut candidate, old, new, binding) = fixture();
        let pin = stored::read(&current.settings).unwrap().unwrap().with_strict(strict);
        current.settings.insert(stored::KEY.into(), pin.encode().unwrap());
        candidate.settings.insert(stored::KEY.into(), pin.encode().unwrap());
        let mut expected = candidate.clone();
        expected.settings.insert(stored::KEY.into(), Stored::local_folder(
            pin.origin().clone(), strict, old.id.clone(),
            selection::local_root_fingerprint(&new).unwrap(),
        ).unwrap().encode().unwrap());
        let original = value(&current);
        assert_eq!(rebase_candidate(&current, &mut candidate, &old, &new, &binding), Ok(true));
        assert_eq!(value(&candidate), value(&expected));
        assert_eq!(value(&current), original);
    }
}

#[test]
fn verified_usb_projection_is_idempotent_for_the_same_candidate_and_receipt() {
    let (current, mut candidate, old, new, binding) = fixture();
    assert_eq!(rebase_candidate(&current, &mut candidate, &old, &new, &binding), Ok(true));
    let expected = value(&candidate);
    assert_eq!(rebase_candidate(&current, &mut candidate, &old, &new, &binding), Ok(false));
    assert_eq!(value(&candidate), expected);
}

#[test]
fn verified_usb_projection_preserves_other_kiosk_target_kinds_and_folder_ids() {
    let entry: Favorite = serde_json::from_value(json!({
        "title":"Fixture channel", "origin":{"provider":"m3u","profile":"Home",
            "subprofile":null,"sourceAlias":"Home"},
        "locator":{"kind":"channel","channelId":"fixture-1"}
    })).unwrap();
    let policies = [
        None,
        Some(Stored::armed(origin(), true).unwrap()),
        Some(Stored::item(entry.clone(), true).unwrap()),
        Some(Stored::queue(vec![entry], false).unwrap()),
        Some(Stored::local_folder(origin(), true, "other-folder".into(), "a".repeat(64)).unwrap()),
    ];
    for policy in policies {
        let (mut current, mut candidate, old, new, binding) = fixture();
        current.settings.remove(stored::KEY);
        candidate.settings.remove(stored::KEY);
        if let Some(policy) = policy {
            current.settings.insert(stored::KEY.into(), policy.encode().unwrap());
            candidate.settings.insert(stored::KEY.into(), policy.encode().unwrap());
        }
        let expected = value(&candidate);
        assert_eq!(rebase_candidate(&current, &mut candidate, &old, &new, &binding), Ok(false));
        assert_eq!(value(&candidate), expected);
    }
}

#[test]
fn verified_usb_projection_rejects_a_pin_from_another_old_root() {
    let (mut current, mut candidate, old, new, binding) = fixture();
    let wrong = Stored::local_folder(origin(), true, old.id.clone(), "a".repeat(64)).unwrap().encode().unwrap();
    current.settings.insert(stored::KEY.into(), wrong.clone());
    candidate.settings.insert(stored::KEY.into(), wrong);
    let expected = value(&candidate);
    assert!(rebase_candidate(&current, &mut candidate, &old, &new, &binding).is_err());
    assert_eq!(value(&candidate), expected);
}

#[test]
fn verified_usb_projection_does_not_overwrite_a_different_candidate_policy() {
    for replacement in [None, Some(Value::Null), Some(Stored::armed(origin(), false).unwrap().encode().unwrap())] {
        let (current, mut candidate, old, new, binding) = fixture();
        candidate.settings.remove(stored::KEY);
        if let Some(replacement) = replacement {
            candidate.settings.insert(stored::KEY.into(), replacement);
        }
        let expected = value(&candidate);
        assert!(rebase_candidate(&current, &mut candidate, &old, &new, &binding).is_err());
        assert_eq!(value(&candidate), expected);
    }
}

#[test]
fn verified_usb_projection_requires_exact_saved_and_candidate_usb_descriptors() {
    for change_current in [false, true] {
        for field in ["path", "name", "uuid", "relative", "missing"] {
            let (mut current, mut candidate, old, new, binding) = fixture();
            let root = if change_current { &old } else { &new };
            let mut changed = descriptor(root, &binding);
            match field {
                "path" => changed.path.push_str("/different"),
                "name" => changed.name.push_str(" changed"),
                "uuid" => changed.usb.as_mut().unwrap().uuid.push_str("-other"),
                "relative" => changed.usb.as_mut().unwrap().filesystem_relative_folder.push_str("/other"),
                "missing" => changed.usb = None,
                _ => unreachable!(),
            }
            folders(if change_current { &mut current } else { &mut candidate }, changed);
            let expected = value(&candidate);
            assert!(rebase_candidate(&current, &mut candidate, &old, &new, &binding).is_err(), "{field}");
            assert_eq!(value(&candidate), expected);
        }
    }
}

#[test]
fn verified_usb_projection_rejects_changed_live_descriptor_identity() {
    for field in ["id", "name", "kind", "authority", "generation", "available"] {
        let (current, mut candidate, old, mut new, binding) = fixture();
        match field {
            "id" => new.id.push_str("-other"),
            "name" => new.name.push_str(" changed"),
            "kind" => new.kind = SourceKind::File,
            "authority" => new.authority_ref = AuthorityRef(10),
            "generation" => new.generation += 1,
            "available" => new.available = false,
            _ => unreachable!(),
        }
        let expected = value(&candidate);
        assert!(rebase_candidate(&current, &mut candidate, &old, &new, &binding).is_err(), "{field}");
        assert_eq!(value(&candidate), expected);
    }
}

#[test]
fn verified_usb_projection_rejects_malformed_saved_policy_without_mutation() {
    let (mut current, mut candidate, old, new, binding) = fixture();
    current.settings.insert(stored::KEY.into(), Value::Null);
    let expected = value(&candidate);
    assert!(rebase_candidate(&current, &mut candidate, &old, &new, &binding).is_err());
    assert_eq!(value(&candidate), expected);
}

#[test]
fn verified_usb_projection_same_path_needs_no_policy_change() {
    let (current, mut candidate, old, mut new, binding) = fixture();
    new.canonical_path = old.canonical_path.clone();
    folders(&mut candidate, descriptor(&new, &binding));
    let expected = value(&candidate);
    assert_eq!(rebase_candidate(&current, &mut candidate, &old, &new, &binding), Ok(false));
    assert_eq!(value(&candidate), expected);
}

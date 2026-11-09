//! Pure captured-source tests; no loader, filesystem, network, or worker runs.
use super::*;
use crate::app::ott_m3u_attachment::{Authorities, Change, Receipt, Registry, SETTINGS_KEY};
use serde_json::json;
use std::collections::BTreeMap;

fn setup(parent_id: &str, attachment_id: &str, endpoint: &str) -> (ProviderProfiles, Registry, Authorities) {
    let parent = ProviderProfile {
        id: parent_id.into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: format!("https://{endpoint}/tv.m3u"), source_alias: "Living room".into(), enabled: true,
    };
    let mut profiles = ProviderProfiles::default();
    profiles.profiles.push(parent);
    profiles.active_profile = Some(parent_id.into());
    let settings = BTreeMap::from([(SETTINGS_KEY.into(), json!({"version":1,"entries":[{
        "id":attachment_id,"parent_profile_id":parent_id,"kind":"vportal",
        "link":format!("portal::[key:private-key]https://{endpoint}/portal"),
        "legacy_source_id":"private-legacy"
    }]}))]);
    let registry = Registry::read(&settings, &profiles).unwrap();
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &profiles).unwrap();
    (profiles, registry, authorities)
}

fn row(receipt: &Receipt) -> CatalogEntry {
    let mut request = plx_data::ott::PortalRequest::root();
    request.category = "Family / Season 2".into();
    request.parse_response(&json!({"type":"category","items":[{
        "type":"stream","title":"Film","fid":7,"request":{"id":12},
        "description":"Complete metadata"
    }]}).to_string(), &receipt.effective_profile().unwrap()).unwrap().catalog.entries.remove(0)
}

#[test]
fn nested_capture_rebinds_to_receiver_parent_without_exporting_private_source() {
    let (sender, _, first) = setup("m3u-1", "attachment-one", "sender.invalid");
    let (receiver, _, second) = setup("m3u-14", "attachment-other", "receiver.invalid");
    let one = SelectionBindings::profiles_only(&sender).with_attachments(&first);
    let two = SelectionBindings::profiles_only(&receiver).with_attachments(&second);
    let receipt = first.receipt("m3u-1").unwrap();
    let favorite = capture_bound(&one, SelectionOwner::M3uAttachment(receipt), &row(receipt)).unwrap();
    assert_eq!(favorite.origin.provider, "m3u");
    assert_eq!(favorite.origin.profile, "Home");
    assert_eq!(favorite.origin.source_alias, "Living room");
    assert_eq!(favorite.origin.subprofile.as_deref(), Some("Family / Season 2"));
    assert_eq!(favorite.locator, Locator::VPortal { id_field:"fid".into(), id:"7".into() });
    let bound = bind_bound(&two, &favorite).unwrap();
    assert!(matches!(bound, SelectionOwner::M3uAttachment(r) if r.parent().id == "m3u-14"));
    assert_eq!(capture_bound(&two, bound, &row(second.receipt("m3u-14").unwrap())).unwrap(), favorite);
    let exported = json!({"version":1,"entries":[favorite]}).to_string();
    for secret in ["private-key", "sender.invalid", "receiver.invalid", "private-legacy", "attachment-one", "m3u-14"] {
        assert!(!exported.contains(secret));
    }
    let imported = Document::parse(exported.as_bytes()).unwrap();
    assert!(bind_bound(&two, &imported.entries()[0]).is_ok());
}

#[test]
fn profile_only_binding_still_refuses_nested_portal_and_missing_attachment_is_explicit() {
    let (profiles, _, authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let view = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    let receipt = authorities.receipt("m3u-1").unwrap();
    let favorite = capture_bound(&view, SelectionOwner::M3uAttachment(receipt), &row(receipt)).unwrap();
    assert_eq!(resolve_profile(&profiles, &favorite), Err(BindingError::UnsupportedSource));
    assert!(matches!(bind_bound(&SelectionBindings::profiles_only(&profiles), &favorite), Err(BindingError::MissingAttachment)));
    assert!(view.validate_owner(SelectionOwner::Profile(&receipt.effective_profile().unwrap())).is_err());
    assert_eq!(profiles.profiles.len(), 1);
    assert_eq!(profiles.profiles[0].kind, ProviderKind::M3u);
}

#[test]
fn ambiguous_parent_names_do_not_choose_first_attachment() {
    let (mut profiles, registry, mut authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let receipt = authorities.capture("m3u-1").unwrap();
    let favorite = capture_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities),
        SelectionOwner::M3uAttachment(&receipt), &row(&receipt)).unwrap();
    let mut other = profiles.profiles[0].clone(); other.id = "m3u-3".into();
    profiles.profiles.push(other);
    authorities.observe(&registry, &profiles).unwrap();
    let view = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    assert!(matches!(bind_bound(&view, &favorite), Err(BindingError::AmbiguousSource)));
    assert_eq!(capture_bound(&view, SelectionOwner::M3uAttachment(&receipt), &row(&receipt)), Err(BindingError::AmbiguousSource));
}

#[test]
fn changed_parent_or_descriptor_and_aba_retire_captured_receipt() {
    let (mut profiles, registry, mut authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let old = authorities.capture("m3u-1").unwrap();
    let favorite = capture_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities),
        SelectionOwner::M3uAttachment(&old), &row(&old)).unwrap();
    let initial_parent = profiles.profiles[0].clone();
    profiles.profiles[0].source = "https://changed.invalid/tv.m3u".into();
    let stale = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    assert!(stale.validate_owner(SelectionOwner::M3uAttachment(&old)).is_err());
    authorities.observe(&registry, &profiles).unwrap();
    profiles.profiles[0] = initial_parent;
    authorities.observe(&registry, &profiles).unwrap();
    assert!(!SelectionBindings::profiles_only(&profiles).with_attachments(&authorities).attachment_is_current(&old, &favorite));
    let current = authorities.capture("m3u-1").unwrap();
    let replacement = registry.apply(&profiles, &profiles.profiles[0], Change::Set {
        link:"portal::[key:new-key]https://new.invalid/portal", legacy_source_id:Some("private-legacy")
    }, None).unwrap();
    authorities.observe(&replacement, &profiles).unwrap();
    authorities.observe(&registry, &profiles).unwrap();
    let view = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    assert!(!view.attachment_is_current(&current, &favorite));
    assert!(capture_bound(&view, SelectionOwner::M3uAttachment(&current), &row(&current)).is_err());
    assert!(bind_bound(&view, &favorite).is_ok(), "new binding is allowed, retired receipt is not");
}

#[test]
fn removed_attachment_retains_favorite_but_refuses_capture_and_resolution() {
    let (profiles, registry, mut authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let old = authorities.capture("m3u-1").unwrap();
    let favorite = capture_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities),
        SelectionOwner::M3uAttachment(&old), &row(&old)).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    let cleared = registry.apply(&profiles, &profiles.profiles[0], Change::Clear, None).unwrap();
    authorities.observe(&cleared, &profiles).unwrap();
    let view = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    assert!(matches!(bind_bound(&view, &favorite), Err(BindingError::MissingAttachment)));
    assert!(capture_bound(&view, SelectionOwner::M3uAttachment(&old), &row(&old)).is_err());
    assert_eq!(document.entries(), &[favorite]);
}

#[test]
fn exact_portal_identity_and_category_drive_index_toggle_and_pin_without_tv_capture() {
    let (profiles, _, authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let receipt = authorities.receipt("m3u-1").unwrap();
    let view = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    let owner = SelectionOwner::M3uAttachment(receipt);
    let item = row(receipt);
    let favorite = capture_bound(&view, owner, &item).unwrap();
    let mut document = Document::default(); document.toggle(favorite.clone()).unwrap();
    assert_eq!(existing_identity_bound(&view, &document, owner, &item), Ok(Some(favorite.identity())));
    let index = FavoriteIndex::new_bound(&view, &document, owner).unwrap();
    assert!(index.contains(&item));
    for field in ["id", "fid"] {
        let mut other = item.clone(); other.provider_id_field = Some(field.into());
        if field == "fid" { other.category = "Family / Season 1".into(); }
        assert!(!matches_entry(&favorite, &other));
        assert!(!index.contains(&other));
    }
    let mut direct_media = item.clone();
    direct_media.media = MediaLocator::Http { url:"https://media.invalid/movie.mp4".into() };
    assert_eq!(capture_bound(&view, owner, &direct_media).unwrap().locator, favorite.locator);
    let mut foreign = item.clone(); foreign.id = "other-attachment:0000".into();
    assert_eq!(capture_bound(&view, owner, &foreign), Err(BindingError::SourceMismatch));
    assert_eq!(existing_identity_bound(&view, &document, owner, &foreign), Err(BindingError::SourceMismatch));
    let mut no_attestation = item.clone(); no_attestation.provider_id = None;
    assert_eq!(capture_bound(&view, owner, &no_attestation), Err(BindingError::InvalidFavorite));
    let proof = LocalFavoriteProofs::default();
    assert!(proof.authorize_pin(&view, &[favorite.clone()], &favorite, &receipt.effective_profile().unwrap()).unwrap().is_none());
    assert!(proof.authorize_pin(&view, &[favorite.clone()], &favorite, receipt.parent()).is_err());
    assert!(attachment_profile(receipt, &favorite).is_ok());
    let mut wrong = favorite; wrong.origin.profile = "Another profile".into();
    assert_eq!(attachment_profile(receipt, &wrong), Err(BindingError::SourceMismatch.message()));
}

#[test]
fn unrelated_profile_changes_keep_binding_but_disabled_or_removed_parent_cannot_resolve() {
    let (mut profiles, registry, mut authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let receipt = authorities.capture("m3u-1").unwrap();
    let favorite = capture_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities),
        SelectionOwner::M3uAttachment(&receipt), &row(&receipt)).unwrap();
    let mut unrelated = profiles.profiles[0].clone();
    unrelated.id = "m3u-2".into(); unrelated.name = "Other".into();
    profiles.profiles.push(unrelated);
    authorities.observe(&registry, &profiles).unwrap();
    assert!(SelectionBindings::profiles_only(&profiles).with_attachments(&authorities).attachment_is_current(&receipt, &favorite));
    profiles.profiles[0].enabled = false;
    // A disabled profile cannot remain the active profile in a valid snapshot.
    profiles.active_profile = None;
    profiles.validate().unwrap();
    authorities.observe(&registry, &profiles).unwrap();
    assert!(matches!(bind_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities), &favorite), Err(BindingError::MissingProfile)));
    profiles.profiles.remove(0); profiles.active_profile = None;
    authorities.observe(&Registry::default(), &profiles).unwrap();
    assert!(matches!(bind_bound(&SelectionBindings::profiles_only(&profiles).with_attachments(&authorities), &favorite), Err(BindingError::MissingProfile)));
}

#[test]
fn explicit_attachment_alias_cannot_be_replaced_by_display_name() {
    let (profiles, _, authorities) = setup("m3u-1", "attachment-one", "sender.invalid");
    let bindings = SelectionBindings::profiles_only(&profiles).with_attachments(&authorities);
    let receipt = authorities.receipt("m3u-1").unwrap();
    let mut favorite = capture_bound(&bindings, SelectionOwner::M3uAttachment(receipt), &row(receipt)).unwrap();
    assert!(bind_bound(&bindings, &favorite).is_ok());
    favorite.origin.source_alias = profiles.profiles[0].name.clone();
    assert!(bind_bound(&bindings, &favorite).is_err());
}

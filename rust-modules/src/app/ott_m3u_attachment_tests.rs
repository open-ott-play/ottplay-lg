use super::*;
use serde_json::json;

const LINK: &str = "portal::[key:fixture-secret]https://portal.fixture.invalid/api/";
const OTHER_LINK: &str = "portal::[key:other-secret]https://second.fixture.invalid/api/";

fn parent(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: "Same display name".into(),
        kind: ProviderKind::M3u,
        source: "https://tv.fixture.invalid/list.m3u".into(),
        source_alias: "Portable profile".into(),
        enabled: true,
    }
}

fn profiles(rows: Vec<ProviderProfile>) -> ProviderProfiles {
    ProviderProfiles {
        version: 1,
        active_profile: rows.first().map(|p| p.id.clone()),
        profiles: rows,
    }
}

fn slot(link: Option<&str>) -> LegacySlot<'_> {
    LegacySlot {
        playlist: "https://tv.fixture.invalid/list.m3u",
        media_url: link,
        media_source_id: Some("legacy-private"),
    }
}

fn set(registry: &Registry, all: &ProviderProfiles, at: usize, link: &str, id: &str) -> Registry {
    registry
        .apply(
            all,
            &all.profiles[at],
            Change::Set {
                link,
                legacy_source_id: Some("legacy-private"),
            },
            Some(id),
        )
        .unwrap()
}

fn stored(registry: &Registry) -> BTreeMap<String, Value> {
    BTreeMap::from([(SETTINGS_KEY.into(), serde_json::to_value(registry).unwrap())])
}

#[test]
fn absent_registry_and_private_roundtrip_preserve_other_settings_and_profiles() {
    let all = profiles(vec![parent("parent-1")]);
    let original_profiles = all.clone();
    let mut settings = BTreeMap::from([
        ("selectedProvider".into(), json!("m3u")),
        ("other".into(), json!({"keep":true})),
    ]);
    let original_settings = settings.clone();
    let empty = Registry::read(&settings, &all).unwrap();
    assert!(empty.for_parent("parent-1").is_none());
    let configured = set(&empty, &all, 0, LINK, "attachment-1");
    configured.put_into(&mut settings, &all).unwrap();
    assert_eq!(Registry::read(&settings, &all).unwrap(), configured);
    settings.remove(SETTINGS_KEY);
    assert_eq!(settings, original_settings);
    assert_eq!(all, original_profiles);
}

#[test]
fn migration_uses_explicit_parent_not_active_slot_or_duplicate_display_name() {
    let all = profiles(vec![parent("active-1"), parent("mapped-15")]);
    let original = Registry::default();
    let migrated = original
        .migrate(
            &all,
            &all.profiles[1],
            slot(Some(LINK)),
            Some("attachment-mapped"),
        )
        .unwrap();
    assert!(migrated.for_parent("active-1").is_none());
    let attachment = migrated.for_parent("mapped-15").unwrap();
    assert_eq!(attachment.id(), "attachment-mapped");
    assert_eq!(attachment.parent_profile_id, "mapped-15");
    assert_eq!(all.active_profile.as_deref(), Some("active-1"));
    assert_eq!(original, Registry::default());
}

#[test]
fn effective_source_reuses_standalone_parser_without_decoding_opaque_escapes() {
    let all = profiles(vec![parent("parent-1")]);
    for link in [
        "  portal::[key:A%2FB%25]https://portal.fixture.invalid/api/?q=%2F  ",
        "PORTAL::%5BKEY:A%2FB%25%5Dhttps://portal.fixture.invalid/api/?q=%2F",
        "portal::%5bkey:A%2FB%25]https://portal.fixture.invalid/api/?q=%2F",
        "portal::[key:A%2FB%25%5dhttps://portal.fixture.invalid/api/?q=%2F",
    ] {
        let registry = Registry::default()
            .migrate(
                &all,
                &all.profiles[0],
                slot(Some(link)),
                Some("attachment-1"),
            )
            .unwrap();
        let projected = registry
            .for_parent("parent-1")
            .unwrap()
            .project(&all.profiles[0])
            .unwrap();
        assert_eq!(projected.kind, ProviderKind::VPortal);
        assert_eq!(projected.source, link.trim());
        assert_eq!(projected.name, all.profiles[0].name);
        assert_eq!(projected.source_alias, all.profiles[0].source_alias);
        assert_eq!(
            parse_vportal_link(&projected.source).unwrap().key,
            "A%2FB%25"
        );
        assert!(projected.validate().is_ok());
        assert_eq!(all.profiles.len(), 1);
    }
}

#[test]
fn invalid_portal_replacement_leaves_last_good_source_and_borrowed_input_untouched() {
    let all = profiles(vec![parent("parent-1")]);
    let original = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    for input in [
        "portal::[key:]https://portal.fixture.invalid/",
        "portal::[key:has space]https://portal.fixture.invalid/",
        "portal::[key:has\ncontrol]https://portal.fixture.invalid/",
        "portal::[key:fixture]https://user:secret@portal.fixture.invalid/",
        "portal::[key:fixture]https://portal.fixture.invalid/#fragment",
        "portal::[key:fixture]file:///private/portal",
        "portal::[key:bad\\key]https://portal.fixture.invalid/",
        "portal::bad",
    ] {
        let input = input.to_owned();
        let retained = input.clone();
        assert_eq!(
            classify_legacy_slot(slot(Some(&input))),
            LegacyKind::InvalidPortal
        );
        assert_eq!(
            original.migrate(&all, &all.profiles[0], slot(Some(&input)), None),
            Err(Error::InvalidLink)
        );
        assert_eq!(input, retained);
        assert_eq!(
            original
                .for_parent("parent-1")
                .unwrap()
                .project(&all.profiles[0])
                .unwrap()
                .source,
            LINK
        );
    }
    let oversized = format!(
        "portal::[key:{}]https://portal.fixture.invalid/",
        "é".repeat(513)
    );
    assert_eq!(
        classify_legacy_slot(slot(Some(&oversized))),
        LegacyKind::InvalidPortal
    );
}

#[test]
fn legacy_classification_keeps_media_only_and_unsupported_sources_distinct() {
    assert_eq!(classify_legacy_slot(slot(None)), LegacyKind::NoLink);
    assert_eq!(classify_legacy_slot(slot(Some("  "))), LegacyKind::NoLink);
    assert_eq!(classify_legacy_slot(slot(Some(LINK))), LegacyKind::Portal);
    assert_eq!(
        classify_legacy_slot(slot(Some("https://catalog.fixture.invalid/root"))),
        LegacyKind::Unsupported
    );
    let media_only = LegacySlot {
        playlist: "",
        ..slot(Some(LINK))
    };
    assert_eq!(classify_legacy_slot(media_only), LegacyKind::MediaOnly);
    assert_eq!(
        classify_legacy_slot(LegacySlot {
            playlist: "file:///playlist.m3u",
            ..slot(Some(LINK))
        }),
        LegacyKind::InvalidPlaylist
    );
    let all = profiles(vec![parent("parent-1")]);
    let original = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    assert_eq!(
        original.migrate(&all, &all.profiles[0], media_only, None),
        Err(Error::InvalidPlaylist)
    );
    assert_eq!(
        original.migrate(
            &all,
            &all.profiles[0],
            slot(Some("https://catalog.fixture.invalid/")),
            None
        ),
        Err(Error::UnsupportedMedia)
    );
    assert_eq!(original.for_parent("parent-1").unwrap().link, LINK);
}

#[test]
fn explicit_clear_removes_and_repeated_import_keeps_id() {
    let all = profiles(vec![parent("parent-1")]);
    let original = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    assert_eq!(
        original
            .migrate(
                &all,
                &all.profiles[0],
                slot(Some(LINK)),
                Some("unused-new-id")
            )
            .unwrap(),
        original
    );
    let replaced = original
        .migrate(&all, &all.profiles[0], slot(Some(OTHER_LINK)), None)
        .unwrap();
    assert_eq!(
        replaced.for_parent("parent-1").unwrap().id(),
        "attachment-1"
    );
    assert_eq!(
        original
            .apply(&all, &all.profiles[0], Change::Clear, None)
            .unwrap(),
        Registry::default()
    );
    assert_eq!(
        original
            .migrate(&all, &all.profiles[0], slot(None), None)
            .unwrap(),
        Registry::default()
    );
    assert_eq!(
        Registry::default().migrate(&all, &all.profiles[0], slot(Some(LINK)), None),
        Err(Error::MissingId)
    );
}

#[test]
fn stale_or_wrong_parent_cannot_receive_an_attachment() {
    let mut all = profiles(vec![parent("parent-1")]);
    let captured = all.profiles[0].clone();
    all.profiles[0].source = "https://changed.fixture.invalid/list.m3u".into();
    assert_eq!(
        Registry::default().migrate(&all, &captured, slot(Some(LINK)), Some("attachment-1")),
        Err(Error::StaleParent)
    );
    all.profiles.clear();
    all.active_profile = None;
    assert_eq!(
        Registry::default().migrate(&all, &captured, slot(Some(LINK)), Some("attachment-1")),
        Err(Error::StaleParent)
    );
    let mut other = parent("parent-1");
    other.kind = ProviderKind::VPortal;
    other.source = LINK.into();
    let all = profiles(vec![other]);
    assert_eq!(
        Registry::default().migrate(
            &all,
            &all.profiles[0],
            slot(Some(LINK)),
            Some("attachment-1")
        ),
        Err(Error::InvalidParent)
    );
}

#[test]
fn registry_rejects_unknown_fields_versions_kinds_and_duplicate_namespaces() {
    let all = profiles(vec![parent("parent-1"), parent("parent-2")]);
    let original = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let value = serde_json::to_value(&original).unwrap();
    let mut invalid = Vec::new();
    let mut row = value.clone();
    row["version"] = json!(2);
    invalid.push(row);
    let mut row = value.clone();
    row["active"] = json!(0);
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["authority"] = json!(42);
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["kind"] = json!("plex");
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["link"] = json!(format!("\n{LINK}"));
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["link"] = json!(format!("{}{LINK}", " ".repeat(16384)));
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["id"] = json!("parent-2");
    invalid.push(row);
    let mut row = value.clone();
    row["entries"][0]["parent_profile_id"] = json!("missing");
    invalid.push(row);
    let mut row = value.clone();
    row["entries"] = json!([value["entries"][0], value["entries"][0]]);
    invalid.push(row);
    invalid.push(Value::Null);
    for value in invalid {
        let settings = BTreeMap::from([(SETTINGS_KEY.into(), value)]);
        let before = settings.clone();
        assert!(Registry::read(&settings, &all).is_err());
        assert_eq!(settings, before);
    }
    assert_eq!(
        Registry::default().migrate(&all, &all.profiles[0], slot(Some(LINK)), Some("parent-2")),
        Err(Error::DuplicateIdentity)
    );
    assert_eq!(
        original.migrate(
            &all,
            &all.profiles[1],
            slot(Some(LINK)),
            Some("attachment-1")
        ),
        Err(Error::DuplicateIdentity)
    );
    for id in ["", "bad/id", "bad id", "bad\n", "bad💡"] {
        assert!(Registry::default()
            .migrate(&all, &all.profiles[0], slot(Some(LINK)), Some(id))
            .is_err());
    }
    let long_id = "a".repeat(129);
    assert!(Registry::default()
        .migrate(&all, &all.profiles[0], slot(Some(LINK)), Some(&long_id))
        .is_err());
}

#[test]
fn fifteen_attachments_do_not_consume_standalone_vportal_capacity() {
    let mut all = profiles((0..15).map(|n| parent(&format!("m3u-{n}"))).collect());
    for n in 0..15 {
        let mut portal = parent(&format!("portal-{n}"));
        portal.kind = ProviderKind::VPortal;
        portal.source = LINK.into();
        all.profiles.push(portal);
    }
    let before = all.clone();
    let mut registry = Registry::default();
    for n in 0..15 {
        registry = set(&registry, &all, n, LINK, &format!("attachment-{n}"));
    }
    registry.validate(&all).unwrap();
    assert_eq!(all, before);
    assert_eq!(registry.entries.len(), 15);
    let mut overflow = registry.clone();
    let mut extra = overflow.entries[0].clone();
    extra.id = "attachment-16".into();
    extra.parent_profile_id = "parent-16".into();
    overflow.entries.push(extra);
    assert_eq!(
        Registry::read(&stored(&overflow), &all),
        Err(Error::InvalidRegistry)
    );
}

#[test]
fn disabled_storage_survives_read_while_missing_parents_require_explicit_prune() {
    let mut all = profiles(vec![parent("parent-1"), parent("parent-2")]);
    let registry = set(
        &set(&Registry::default(), &all, 0, LINK, "attachment-1"),
        &all,
        1,
        LINK,
        "attachment-2",
    );
    all.profiles[0].enabled = false;
    all.active_profile = Some("parent-2".into());
    assert_eq!(Registry::read(&stored(&registry), &all).unwrap(), registry);
    assert_eq!(
        registry
            .for_parent("parent-1")
            .unwrap()
            .project(&all.profiles[0]),
        Err(Error::InvalidParent)
    );
    let mut candidate = registry.clone();
    assert!(!candidate.prune(&all).unwrap());
    all.profiles.remove(1);
    all.active_profile = None;
    assert_eq!(
        Registry::read(&stored(&registry), &all),
        Err(Error::InvalidParent)
    );
    assert!(candidate.prune(&all).unwrap());
    assert!(candidate.for_parent("parent-1").is_some());
    assert!(candidate.for_parent("parent-2").is_none());
    all.profiles[0].kind = ProviderKind::VPortal;
    all.profiles[0].source = LINK.into();
    assert!(candidate.prune(&all).unwrap());
    assert_eq!(candidate, Registry::default());
}

#[test]
fn legacy_identity_bounds_and_settings_overflow_refuse_without_partial_updates() {
    let all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    for source in ["invalid\nsource".to_owned(), "x".repeat(4097)] {
        let input = LegacySlot {
            media_source_id: Some(&source),
            ..slot(Some(LINK))
        };
        assert_eq!(classify_legacy_slot(input), LegacyKind::InvalidLegacySource);
        assert_eq!(
            registry.migrate(&all, &all.profiles[0], input, None),
            Err(Error::InvalidLegacySource)
        );
    }
    let mut settings = BTreeMap::from([("other".into(), json!("x".repeat(1024 * 1024)))]);
    let before = settings.clone();
    assert_eq!(
        registry.put_into(&mut settings, &all),
        Err(Error::StorageLimit)
    );
    assert_eq!(settings, before);
}

#[test]
fn unchanged_or_unrelated_snapshots_do_not_revoke_source_authority() {
    let mut all = profiles(vec![parent("parent-1"), parent("unrelated")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    assert!(authorities.observe(&registry, &all).unwrap());
    let receipt = authorities.capture("parent-1").unwrap();
    assert_eq!(receipt.parent(), &all.profiles[0]);
    assert_eq!(
        receipt.attachment(),
        registry.for_parent("parent-1").unwrap()
    );
    assert!(!authorities.observe(&registry, &all).unwrap());
    all.active_profile = Some("unrelated".into());
    all.profiles[1].name = "Unrelated edit".into();
    assert!(!authorities.observe(&registry, &all).unwrap());
    assert!(authorities.is_current(&receipt));
    assert_eq!(authorities.receipt("parent-1"), Some(&receipt));
    assert_eq!(receipt.effective_profile().unwrap().source, LINK);
    assert!(authorities.capture("unrelated").is_none());
}

#[test]
fn link_and_legacy_identity_changes_revoke_receipts_even_after_a_b_a() {
    let all = profiles(vec![parent("parent-1")]);
    let first = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let second = set(&first, &all, 0, OTHER_LINK, "ignored");
    let mut authorities = Authorities::default();
    authorities.observe(&first, &all).unwrap();
    let a = authorities.capture("parent-1").unwrap();
    authorities.observe(&second, &all).unwrap();
    let b = authorities.capture("parent-1").unwrap();
    assert!(b.revision() > a.revision());
    assert!(!authorities.is_current(&a));
    authorities.observe(&first, &all).unwrap();
    let new_a = authorities.capture("parent-1").unwrap();
    assert!(new_a.revision() > b.revision());
    assert!(!authorities.is_current(&a));
    let legacy_change = first
        .apply(
            &all,
            &all.profiles[0],
            Change::Set {
                link: LINK,
                legacy_source_id: Some("new-legacy-identity"),
            },
            None,
        )
        .unwrap();
    authorities.observe(&legacy_change, &all).unwrap();
    assert!(!authorities.is_current(&new_a));
}

#[test]
fn complete_parent_changes_removal_and_disable_reenable_revoke_old_authority() {
    let mut all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let first = authorities.capture("parent-1").unwrap();
    all.profiles[0].source = "https://changed.fixture.invalid/list.m3u".into();
    authorities.observe(&registry, &all).unwrap();
    assert!(!authorities.is_current(&first));
    let changed = authorities.capture("parent-1").unwrap();
    all.profiles[0].source_alias = "Renamed alias".into();
    authorities.observe(&registry, &all).unwrap();
    assert!(!authorities.is_current(&changed));
    let before_disable = authorities.capture("parent-1").unwrap();
    all.profiles[0].enabled = false;
    all.active_profile = None;
    authorities.observe(&registry, &all).unwrap();
    assert!(authorities.receipt("parent-1").is_none());
    all.profiles[0].enabled = true;
    authorities.observe(&registry, &all).unwrap();
    let restored = authorities.capture("parent-1").unwrap();
    assert!(restored.revision() > before_disable.revision());
    assert!(!authorities.is_current(&before_disable));
    authorities.observe(&Registry::default(), &all).unwrap();
    authorities.observe(&registry, &all).unwrap();
    assert!(!authorities.is_current(&restored));
}

#[test]
fn receipt_identity_compares_revision_and_complete_private_descriptor() {
    let all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let current = authorities.capture("parent-1").unwrap();
    let mut altered = current.clone();
    altered.attachment.link = OTHER_LINK.into();
    assert!(!authorities.is_current(&altered));
    let mut altered = current.clone();
    altered.parent.name = "Other name".into();
    assert!(!authorities.is_current(&altered));
    let mut altered = current.clone();
    altered.revision += 1;
    assert!(!authorities.is_current(&altered));
    assert!(authorities.is_current(&current));
}

#[test]
fn failed_observation_is_transactional_and_can_explicitly_fail_closed() {
    let all = profiles(vec![parent("parent-1"), parent("parent-2")]);
    let registry = set(
        &set(&Registry::default(), &all, 0, LINK, "attachment-1"),
        &all,
        1,
        LINK,
        "attachment-2",
    );
    let changed = set(
        &set(&registry, &all, 0, OTHER_LINK, "ignored"),
        &all,
        1,
        OTHER_LINK,
        "ignored",
    );
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let first = authorities.capture("parent-1").unwrap();
    let second = authorities.capture("parent-2").unwrap();
    authorities.next_revision = u64::MAX - 1;
    assert_eq!(
        authorities.observe(&changed, &all),
        Err(Error::RevisionExhausted)
    );
    assert_eq!(authorities.next_revision, u64::MAX - 1);
    assert!(authorities.is_current(&first));
    assert!(authorities.is_current(&second));
    authorities.invalidate_all();
    assert_eq!(authorities.context_revision(), 0);
    assert!(!authorities.is_current(&first));
    assert!(!authorities.is_current(&second));
    let mut malformed = registry.clone();
    malformed.version = 2;
    assert_eq!(
        authorities.observe(&malformed, &all),
        Err(Error::InvalidRegistry)
    );
    assert!(authorities.receipt("parent-1").is_none());
}

#[test]
fn settings_context_catches_parent_a_b_a_without_an_attachment() {
    let mut all = profiles(vec![parent("parent-1")]);
    let registry = Registry::default();
    let mut authorities = Authorities::default();
    assert_eq!(authorities.context_revision(), 0);
    assert!(!authorities.observe(&registry, &all).unwrap());
    let first = authorities.context_revision();
    assert!(first > 0);
    let original = all.profiles[0].clone();
    all.profiles[0].name = "Changed parent".into();
    assert!(!authorities.observe(&registry, &all).unwrap());
    let second = authorities.context_revision();
    assert!(second > first);
    all.profiles[0] = original;
    authorities.observe(&registry, &all).unwrap();
    assert!(authorities.context_revision() > second);
    let stable = authorities.context_revision();
    let mut unrelated = parent("other-provider");
    unrelated.kind = ProviderKind::VPortal;
    unrelated.source = LINK.into();
    all.profiles.push(unrelated);
    authorities.observe(&registry, &all).unwrap();
    assert_eq!(authorities.context_revision(), stable);
    authorities.invalidate_all();
    assert_eq!(authorities.context_revision(), 0);
    authorities.observe(&registry, &all).unwrap();
    assert!(authorities.context_revision() > stable);
}

#[test]
fn unrelated_parent_edit_changes_settings_context_but_preserves_other_receipt() {
    let mut all = profiles(vec![parent("parent-1"), parent("parent-2")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let receipt = authorities.capture("parent-1").unwrap();
    let before = authorities.context_revision();
    all.profiles[1].source_alias = "Changed unconnected parent".into();
    assert!(!authorities.observe(&registry, &all).unwrap());
    assert!(authorities.context_revision() > before);
    assert!(authorities.is_current(&receipt));
}

#[test]
fn settings_context_overflow_is_transactional_then_explicitly_revoked() {
    let mut all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let receipt = authorities.capture("parent-1").unwrap();
    authorities.context_counter = u64::MAX;
    all.profiles[0].name = "Changed parent".into();
    assert_eq!(
        authorities.observe(&registry, &all),
        Err(Error::RevisionExhausted)
    );
    assert!(authorities.is_current(&receipt));
    authorities.invalidate_all();
    assert!(!authorities.is_current(&receipt));
    assert_eq!(authorities.context_revision(), 0);
    assert_eq!(
        authorities.observe(&registry, &all),
        Err(Error::RevisionExhausted)
    );
    assert_eq!(authorities.context_revision(), 0);
}

#[test]
fn cloned_context_predicts_commit_without_activating_candidate_authority() {
    let all = profiles(vec![parent("parent-1")]);
    let current = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let candidate = set(&current, &all, 0, OTHER_LINK, "ignored");
    let mut live = Authorities::default();
    live.observe(&current, &all).unwrap();
    let receipt = live.capture("parent-1").unwrap();
    let revision = live.context_revision();
    let mut projected = live.clone();
    projected.observe(&candidate, &all).unwrap();
    assert_eq!(live.context_revision(), revision);
    assert!(live.is_current(&receipt));
    assert!(!projected.is_current(&receipt));
    let projected_receipt = projected.capture("parent-1").unwrap();
    assert!(!live.is_current(&projected_receipt));
    live.observe(&candidate, &all).unwrap();
    assert_eq!(live.context_revision(), projected.context_revision());
    assert!(live.is_current(&projected_receipt));
}

#[test]
fn active_profile_a_b_a_invalidates_settings_but_preserves_attachment_work() {
    let mut all = profiles(vec![parent("parent-1"), parent("parent-2")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let receipt = authorities.capture("parent-1").unwrap();
    let first = authorities.context_revision();
    all.active_profile = Some("parent-2".into());
    assert!(!authorities.observe(&registry, &all).unwrap());
    let middle = authorities.context_revision();
    all.active_profile = Some("parent-1".into());
    assert!(!authorities.observe(&registry, &all).unwrap());
    assert!(middle > first);
    assert!(authorities.context_revision() > middle);
    assert!(authorities.is_current(&receipt));
}

#[test]
fn debug_and_static_errors_never_reveal_private_source_material() {
    let all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authorities = Authorities::default();
    authorities.observe(&registry, &all).unwrap();
    let receipt = authorities.capture("parent-1").unwrap();
    let debug = format!(
        "{registry:?} {receipt:?} {:?} {:?}",
        receipt.attachment(),
        receipt.effective_profile().unwrap()
    );
    for secret in [
        "fixture-secret",
        "portal.fixture.invalid",
        "legacy-private",
        "tv.fixture.invalid",
    ] {
        assert!(!debug.contains(secret));
    }
    for error in [
        Error::InvalidRegistry,
        Error::InvalidParent,
        Error::StaleParent,
        Error::DuplicateIdentity,
        Error::InvalidLink,
        Error::InvalidLegacySource,
        Error::UnsupportedMedia,
        Error::InvalidPlaylist,
        Error::MissingId,
        Error::StorageLimit,
        Error::RevisionExhausted,
    ] {
        assert!(!error.message().contains("https://"));
        assert!(!error.message().contains("fixture-secret"));
    }
}


#[test]
fn portal_only_source_creation_requires_and_atomically_derives_its_descriptor() {
    let old = profiles(Vec::new());
    let mut source = parent("media-parent");
    source.source = LINK.into();
    let next = profiles(vec![source]);
    assert_eq!(Registry::read(&BTreeMap::new(), &next), Err(Error::InvalidLink));
    let mut calls = 0;
    let registry = Registry::default().reconcile_sources(&old, &next, &mut || {
        calls += 1;
        Ok("attachment-1".into())
    }).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(registry.for_parent("media-parent").unwrap().link, LINK);
    assert_eq!(Registry::read(&stored(&registry), &next).unwrap(), registry);
    let same = registry.reconcile_sources(&next, &next, &mut || panic!("unchanged source must retain ID")).unwrap();
    assert_eq!(same, registry);
    assert_eq!(registry.for_parent("media-parent").unwrap().project(&next.profiles[0]).unwrap().kind, ProviderKind::VPortal);
}

#[test]
fn portal_only_read_rejects_missing_mismatched_or_explicitly_cleared_descriptor() {
    let old = profiles(vec![parent("media-parent")]);
    let registry = set(&Registry::default(), &old, 0, LINK, "attachment-1");
    let mut next = old.clone();
    next.profiles[0].source = OTHER_LINK.into();
    assert_eq!(Registry::read(&stored(&registry), &next), Err(Error::InvalidLink));
    assert_eq!(registry.for_parent("media-parent").unwrap().project(&next.profiles[0]), Err(Error::InvalidLink));
    let current = registry.reconcile_sources(&old, &next, &mut || panic!("existing ID")).unwrap();
    assert_eq!(current.apply(&next, &next.profiles[0], Change::Clear, None), Err(Error::InvalidLink));
    assert_eq!(current.for_parent("media-parent").unwrap().legacy_source_id(), Some("legacy-private"));
    assert_eq!(current.for_parent("media-parent").unwrap().id(), "attachment-1");
    assert_eq!(registry.for_parent("media-parent").unwrap().link, LINK);
}

#[test]
fn explicit_portal_source_edits_keep_identity_through_media_and_tv_transitions() {
    let old = profiles(vec![parent("media-parent")]);
    let registry = set(&Registry::default(), &old, 0, LINK, "attachment-1");
    let mut media = old.clone();
    media.profiles[0].source = OTHER_LINK.into();
    let media_registry = registry.reconcile_sources(&old, &media, &mut || panic!("existing ID")).unwrap();
    let tv_registry = media_registry.reconcile_sources(&media, &old, &mut || panic!("existing ID")).unwrap();
    assert_eq!(tv_registry, media_registry);
    assert_eq!(tv_registry.for_parent("media-parent").unwrap().link, OTHER_LINK);
    assert_eq!(tv_registry.apply(&old, &old.profiles[0], Change::Clear, None).unwrap(), Registry::default());
    let mut disabled = media.clone();
    disabled.profiles[0].enabled = false;
    disabled.active_profile = None;
    assert_eq!(media_registry.reconcile_sources(&media, &disabled, &mut || panic!("existing ID")).unwrap(), media_registry);
    assert!(Registry::read(&stored(&media_registry), &disabled).is_ok());
    assert_eq!(media_registry.for_parent("media-parent").unwrap().project(&disabled.profiles[0]), Err(Error::InvalidParent));
}

#[test]
fn portal_source_mapping_accepts_legacy_empty_playlist_only_for_matching_real_parent() {
    let old = profiles(vec![parent("media-parent")]);
    let mut media = old.clone();
    media.profiles[0].source = LINK.into();
    let registry = Registry::default().reconcile_sources(&old, &media, &mut || Ok("attachment-1".into())).unwrap();
    let migrated = registry.migrate(&media, &media.profiles[0], LegacySlot {
        playlist: "", media_url: Some(LINK), media_source_id: Some("legacy-private"),
    }, None).unwrap();
    assert_eq!(migrated.for_parent("media-parent").unwrap().id(), "attachment-1");
    assert_eq!(migrated.for_parent("media-parent").unwrap().legacy_source_id(), Some("legacy-private"));
    assert!(registry.migrate(&media, &media.profiles[0], LegacySlot {
        playlist: "", media_url: Some(OTHER_LINK), media_source_id: None,
    }, None).is_err());
}

#[test]
fn source_reconciliation_failure_does_not_mutate_registry_or_candidate_profiles() {
    let old = profiles(Vec::new());
    let mut source = parent("media-parent");
    source.source = LINK.into();
    let next = profiles(vec![source]);
    let captured = next.clone();
    let registry = Registry::default();
    assert_eq!(registry.reconcile_sources(&old, &next, &mut || Err(Error::MissingId)), Err(Error::MissingId));
    assert_eq!(registry, Registry::default());
    assert_eq!(next, captured);
    assert_eq!(registry.reconcile_sources(&old, &next, &mut || Ok("media-parent".into())), Err(Error::DuplicateIdentity));
}

#[test]
fn portal_source_change_advances_receipt_and_old_link_cannot_revive() {
    let old = profiles(vec![parent("media-parent")]);
    let mut a = old.clone();
    a.profiles[0].source = LINK.into();
    let ar = Registry::default().reconcile_sources(&old, &a, &mut || Ok("attachment-1".into())).unwrap();
    let mut authorities = Authorities::default();
    authorities.observe(&ar, &a).unwrap();
    let original = authorities.capture("media-parent").unwrap();
    let mut b = a.clone();
    b.profiles[0].source = OTHER_LINK.into();
    let br = ar.reconcile_sources(&a, &b, &mut || panic!("stable ID")).unwrap();
    authorities.observe(&br, &b).unwrap();
    assert!(!authorities.is_current(&original));
    let restored = br.reconcile_sources(&b, &a, &mut || panic!("stable ID")).unwrap();
    authorities.observe(&restored, &a).unwrap();
    assert!(!authorities.is_current(&original));
    assert!(authorities.is_current(&authorities.capture("media-parent").unwrap()));
}

fn kiosk_receipt() -> Receipt {
    let all = profiles(vec![parent("parent-1")]);
    let registry = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let mut authority = Authorities::default();
    authority.observe(&registry, &all).unwrap();
    authority.capture("parent-1").unwrap()
}

#[test]
fn kiosk_binding_roundtrip_omits_private_config_and_runtime_revision() {
    let receipt = kiosk_receipt();
    let binding = receipt.kiosk_binding().unwrap();
    let encoded = serde_json::to_value(&binding).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 3);
    assert_eq!(encoded["parent_profile_id"], "parent-1");
    assert_eq!(encoded["attachment_id"], "attachment-1");
    assert_eq!(binding.source_fingerprint.len(), 64);
    binding.validate().unwrap();
    assert_eq!(serde_json::from_value::<KioskBinding>(encoded.clone()).unwrap(), binding);
    for private in [LINK, "fixture-secret", "tv.fixture.invalid", "legacy-private", "revision"] {
        assert!(!encoded.to_string().contains(private), "{private}");
    }
    let mut restarted = receipt.clone();
    restarted.revision += 1;
    assert_eq!(restarted.kiosk_binding().unwrap(), binding);
}

#[test]
fn kiosk_binding_fingerprints_the_exact_parent_and_attachment_configuration() {
    let receipt = kiosk_receipt();
    let binding = receipt.kiosk_binding().unwrap();
    for field in ["parent-source", "parent-name", "parent-alias", "parent-id", "attachment-id", "link", "legacy"] {
        let mut changed = receipt.clone();
        match field {
            "parent-source" => changed.parent.source = "https://other.fixture.invalid/list.m3u".into(),
            "parent-name" => changed.parent.name = "Renamed".into(),
            "parent-alias" => changed.parent.source_alias = "Other alias".into(),
            "parent-id" => {
                changed.parent.id = "other-parent".into();
                changed.attachment.parent_profile_id = changed.parent.id.clone();
            }
            "attachment-id" => changed.attachment.id = "other-attachment".into(),
            "link" => changed.attachment.link = OTHER_LINK.into(),
            "legacy" => changed.attachment.legacy_source_id = Some("other-legacy".into()),
            _ => unreachable!(),
        }
        assert_ne!(changed.kiosk_binding().unwrap().source_fingerprint, binding.source_fingerprint, "{field}");
    }
    let mut disabled = receipt.clone(); disabled.parent.enabled = false;
    assert!(disabled.kiosk_binding().is_err());
    let mut detached = receipt; detached.attachment.parent_profile_id = "other-parent".into();
    assert!(detached.kiosk_binding().is_err());
}

#[test]
fn kiosk_binding_survives_restart_but_does_not_revalidate_an_old_aba_receipt() {
    let all = profiles(vec![parent("parent-1")]);
    let a = set(&Registry::default(), &all, 0, LINK, "attachment-1");
    let b = set(&a, &all, 0, OTHER_LINK, "attachment-1");
    let mut authority = Authorities::default();
    authority.observe(&a, &all).unwrap();
    let original = authority.capture("parent-1").unwrap();
    let saved = original.kiosk_binding().unwrap();
    authority.observe(&b, &all).unwrap();
    assert_ne!(authority.capture("parent-1").unwrap().kiosk_binding().unwrap(), saved);
    authority.observe(&a, &all).unwrap();
    assert!(!authority.is_current(&original));
    let current = authority.capture("parent-1").unwrap();
    assert!(authority.is_current(&current));
    assert_eq!(current.kiosk_binding().unwrap(), saved);
    let mut restarted = Authorities::default();
    restarted.observe(&a, &all).unwrap();
    assert_eq!(restarted.capture("parent-1").unwrap().kiosk_binding().unwrap(), saved);
}

#[test]
fn kiosk_binding_rejects_unknown_fields_invalid_ids_and_noncanonical_digests() {
    let valid = serde_json::to_value(kiosk_receipt().kiosk_binding().unwrap()).unwrap();
    let mut unknown = valid.clone(); unknown["link"] = json!(LINK);
    assert!(serde_json::from_value::<KioskBinding>(unknown.clone()).is_err());
    assert!(KioskBinding::validate_json(&unknown).is_err());
    for (field, value) in [
        ("parent_profile_id", json!("")), ("parent_profile_id", json!("../parent")),
        ("parent_profile_id", json!("a".repeat(129))), ("attachment_id", json!("parent-1")),
        ("attachment_id", json!("bad id")), ("attachment_id", Value::Null),
        ("source_fingerprint", json!("A".repeat(64))), ("source_fingerprint", json!("g".repeat(64))),
        ("source_fingerprint", json!("a".repeat(63))), ("source_fingerprint", json!("a".repeat(65))),
    ] {
        let mut invalid = valid.clone(); invalid[field] = value;
        assert!(KioskBinding::validate_json(&invalid).is_err(), "{field}");
        if let Ok(decoded) = serde_json::from_value::<KioskBinding>(invalid) {
            assert!(decoded.validate().is_err(), "{field}");
        }
    }
}

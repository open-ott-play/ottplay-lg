use super::*;
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn source(id: &str) -> Source<'_> {
    Source {
        profile_id: id,
        fingerprint: A,
    }
}

#[test]
fn favorites_stays_selected_across_profiles_and_empty_provider() {
    let mut state = Preferences::default();
    for next in [
        Some(source("m3u:1")),
        Some(source("plex:1")),
        None,
        Some(source("vportal:2")),
    ] {
        assert!(
            state
                .switch_source(Some(source("m3u:1")), next, "News", true)
                .unwrap()
                .favorites_selected
        );
    }
}
#[test]
fn ordinary_categories_restore_a_b_a_with_restart_roundtrip() {
    let mut state = Preferences::default();
    let selected = state.choose_category(source("A"), "News").unwrap();
    assert!(!selected.favorites_selected);
    assert_eq!(
        state
            .switch_source(Some(source("A")), Some(source("B")), "News", false)
            .unwrap()
            .category,
        ""
    );
    state.choose_category(source("B"), "Movies").unwrap();
    let mut settings = BTreeMap::new();
    settings.insert("unrelated".into(), Value::Bool(true));
    state.write_into(&mut settings).unwrap();
    let mut restarted = Preferences::read(&settings).unwrap();
    assert_eq!(
        restarted
            .switch_source(Some(source("B")), Some(source("A")), "Movies", true)
            .unwrap(),
        Selection {
            category: "News".into(),
            favorites_selected: true
        }
    );
    assert_eq!(settings["unrelated"], Value::Bool(true));
}
#[test]
fn explicit_category_including_all_ends_favorites_preference() {
    let mut state = Preferences::default();
    for category in ["News", ""] {
        let choice = state.choose_category(source("A"), category).unwrap();
        assert_eq!(choice.category, category);
        assert!(!choice.favorites_selected);
    }
}
#[test]
fn single_legacy_category_is_seeded_once_without_overwriting_known_choice() {
    let mut state = Preferences::default();
    state
        .switch_source(Some(source("A")), Some(source("B")), "News", false)
        .unwrap();
    state
        .switch_source(Some(source("A")), Some(source("B")), "", false)
        .unwrap();
    assert_eq!(state.remembered(source("A")).unwrap(), Some("News"));
}
#[test]
fn missing_and_partial_catalog_fallback_never_erases_remembered_category() {
    let mut state = Preferences::default();
    state.choose_category(source("A"), "News").unwrap();
    let before = state.clone();
    assert_eq!(
        catalog_selection("News", ["Movies"], false),
        CatalogSelection::Awaiting("News")
    );
    let missing = catalog_selection("News", ["Movies"], true);
    assert_eq!(missing, CatalogSelection::Missing("News"));
    assert_eq!(missing.effective(), "");
    assert_eq!(
        catalog_selection("News", ["Movies", "News"], true).effective(),
        "News"
    );
    assert_eq!(state, before);
}
#[test]
fn replacement_source_cannot_inherit_old_sources_category() {
    let mut state = Preferences::default();
    state.choose_category(source("A"), "News").unwrap();
    let replacement = Source {
        profile_id: "A",
        fingerprint: B,
    };
    assert_eq!(state.remembered(replacement).unwrap(), None);
    assert_eq!(
        state
            .switch_source(Some(replacement), Some(replacement), "News", true)
            .unwrap()
            .category,
        ""
    );
    assert_eq!(state.remembered(source("A")).unwrap(), Some("News"));
    state.choose_category(replacement, "Kids").unwrap();
    assert_eq!(state.remembered(replacement).unwrap(), Some("Kids"));
    assert_eq!(state.remembered(source("A")).unwrap(), None);
}
#[test]
fn full_history_rejects_new_owner_without_eviction_or_partial_mutation() {
    let mut state = Preferences::default();
    for n in 0..PROFILE_COUNT {
        state
            .choose_category(source(&format!("p{n}")), "News")
            .unwrap();
    }
    let before = state.clone();
    assert_eq!(
        state.choose_category(source("new"), "Kids"),
        Err(Error::TooLarge)
    );
    assert_eq!(state, before);
}
#[test]
fn corrupt_or_unknown_history_is_rejected_instead_of_reset() {
    for value in [
        serde_json::json!({"version":2,"profiles":{}}),
        serde_json::json!({"version":1,"profiles":{},"extra":true}),
        Value::Null,
    ] {
        let settings = BTreeMap::from([(KEY.into(), value)]);
        assert!(Preferences::read(&settings).is_err());
    }
}
#[test]
fn categories_keep_exact_unicode_and_reject_oversize_atomically() {
    let mut state = Preferences::default();
    state
        .choose_category(source("A"), "Мультсериалы / 日本語")
        .unwrap();
    assert_eq!(
        state.remembered(source("A")).unwrap(),
        Some("Мультсериалы / 日本語")
    );
    let before = state.clone();
    assert_eq!(
        state.choose_category(source("A"), &"x".repeat(CATEGORY_BYTES + 1)),
        Err(Error::TooLarge)
    );
    assert_eq!(state, before);
}
#[test]
fn incomplete_catalog_with_present_category_does_not_fallback() {
    assert_eq!(
        catalog_selection("News", ["News"], false),
        CatalogSelection::Category("News")
    );
    assert_eq!(
        catalog_selection("", std::iter::empty(), false),
        CatalogSelection::All
    );
}

#[test]
fn deletion_prunes_only_complete_validated_profile_inventory() {
    let mut state = Preferences::default();
    state.choose_category(source("A"), "News").unwrap();
    let before = state.clone();
    let mut invalid = plx_data::ott::ProviderProfiles::default();
    invalid.active_profile = Some("missing".into());
    assert_eq!(state.prune_deleted(&invalid), Err(Error::Invalid));
    assert_eq!(state, before);
    assert_eq!(
        state.prune_deleted(&plx_data::ott::ProviderProfiles::default()),
        Ok(1)
    );
    assert_eq!(state.remembered(source("A")).unwrap(), None);
}

#[test]
fn repeated_source_edits_replace_one_slot_without_accumulating_history() {
    let mut state = Preferences::default();
    for n in 0..150 {
        let fingerprint = format!("{n:064x}");
        state
            .choose_category(
                Source {
                    profile_id: "A",
                    fingerprint: &fingerprint,
                },
                "News",
            )
            .unwrap();
    }
    assert_eq!(state.profiles.len(), 1);
}

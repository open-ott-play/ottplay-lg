use super::*;
fn channel(id: &str) -> Favorite {
    Favorite {
        title: "Fixture".into(),
        origin: Origin {
            provider: "m3u".into(),
            profile: "Home".into(),
            subprofile: None,
            source_alias: "Home".into(),
        },
        locator: Locator::Channel {
            channel_id: id.into(),
        },
    }
}
#[test]
fn stable_pin_ignores_title_but_not_profile_or_channel() {
    let item = channel("one");
    let pin = Stored::item(item.clone(), true).unwrap();
    let mut renamed = item.clone();
    renamed.title = "New title".into();
    assert!(matches_pin(&pin, &renamed, None));
    renamed.origin.profile = "Other".into();
    assert!(!matches_pin(&pin, &renamed, None));
    assert!(!matches_pin(&pin, &channel("two"), None));
}
#[test]
fn queue_preserves_duplicate_positions_and_is_not_mixed_scope() {
    let first = channel("one");
    let second = channel("two");
    let pin = Stored::queue(vec![first.clone(), second.clone(), first.clone()], true).unwrap();
    assert_eq!(pin.pins(), [first.clone(), second, first]);
    let mut foreign = channel("three");
    foreign.origin.profile = "Elsewhere".into();
    assert!(Stored::queue(vec![channel("one"), foreign], true).is_err());
}
#[test]
fn folder_pin_requires_exact_current_descriptor_and_local_locator() {
    let item = Favorite {
        title: "Video".into(),
        origin: Origin {
            provider: "local".into(),
            profile: "Local videos".into(),
            subprofile: None,
            source_alias: "Fixture".into(),
        },
        locator: Locator::Local {
            root_alias: "Fixture".into(),
            relative_path: "Folder/Video.mp4".into(),
        },
    };
    let hash = "a".repeat(64);
    let pin =
        Stored::local_folder(item.origin.clone(), true, "root-1".into(), hash.clone()).unwrap();
    assert!(matches_pin(&pin, &item, Some(("root-1", &hash))));
    assert!(!matches_pin(&pin, &item, None));
    assert!(!matches_pin(&pin, &item, Some(("root-2", &hash))));
    assert!(!matches_pin(&pin, &item, Some(("root-1", &"b".repeat(64)))));
    assert!(!matches_pin(&pin, &channel("one"), Some(("root-1", &hash))));
}
#[test]
fn armed_state_is_not_an_owned_target() {
    let item = channel("one");
    let pin = Stored::armed(item.origin.clone(), true).unwrap();
    assert!(!matches_pin(&pin, &item, None));
}

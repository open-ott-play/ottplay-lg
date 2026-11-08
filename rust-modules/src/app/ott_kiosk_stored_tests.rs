use super::*;
use serde_json::json;

fn origin() -> Value {
    json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"})
}
fn entry() -> Value {
    json!({"title":"Fixture channel","origin":origin(),"locator":{"kind":"channel","channelId":"fixture-1"}})
}
fn policy(target: Value) -> Value {
    json!({"version":1,"strict":true,"origin":origin(),"target":target})
}
fn settings(value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(KEY.into(), value)])
}

#[test]
fn absent_key_is_the_only_off_encoding() {
    assert_eq!(state(&BTreeMap::new()), Ok(State::Off));
    assert!(state(&settings(Value::Null)).is_err());
    assert!(state(&settings(json!(false))).is_err());
    assert!(state(&settings(json!({}))).is_err());
}

#[test]
fn unsupported_version_kind_and_unknown_fields_are_restricted() {
    let base = policy(json!({"kind":"armed"}));
    for (key, value) in [
        ("version", json!(2)),
        ("strict", Value::Null),
        ("future", json!(true)),
        ("target", json!({"kind":"folder"})),
    ] {
        let mut invalid = base.clone();
        invalid[key] = value;
        assert!(state(&settings(invalid)).is_err(), "{key}");
    }
}

#[test]
fn armed_keeps_its_scope_without_claiming_a_pin() {
    assert_eq!(
        state(&settings(policy(json!({"kind":"armed"})))),
        Ok(State::Armed { strict: true })
    );
    let mut invalid = policy(json!({"kind":"armed"}));
    invalid["origin"]["sourceAlias"] = json!("https://not-an-alias.invalid");
    assert!(state(&settings(invalid)).is_err());
}

#[test]
fn item_pin_uses_the_existing_favorite_validator() {
    assert_eq!(
        state(&settings(policy(json!({"kind":"item","entry":entry()})))),
        Ok(State::Locked { strict: true })
    );
    let mut invalid = entry();
    invalid["locator"]["channelId"] = json!("channel:17");
    assert!(state(&settings(policy(json!({"kind":"item","entry":invalid})))).is_err());
}

#[test]
fn queue_preserves_ordered_stable_pins_and_needs_no_new_cursor() {
    let mut value = policy(json!({"kind":"queue","entries":[entry(),entry()]}));
    value["strict"] = json!(false);
    assert_eq!(state(&settings(value)), Ok(State::Locked { strict: false }));
    assert!(state(&settings(policy(json!({"kind":"queue","entries":[]})))).is_err());
    assert!(state(&settings(policy(
        json!({"kind":"queue","entries":[entry()],"cursor":0})
    )))
    .is_err());
}

#[test]
fn mixed_scope_and_invalid_local_paths_are_restricted() {
    let mut foreign = entry();
    foreign["origin"]["profile"] = json!("Other profile");
    assert!(state(&settings(policy(
        json!({"kind":"queue","entries":[entry(),foreign]})
    )))
    .is_err());
    let mut local = policy(json!({"kind":"armed"}));
    local["origin"]["provider"] = json!("local");
    local["target"] = json!({"kind":"item","entry":{
        "title":"Fixture video","origin":local["origin"].clone(),
        "locator":{"kind":"local-media","rootAlias":"Fixture folder","relativePath":"../escape.mp4"}
    }});
    assert!(state(&settings(local)).is_err());
}

#[test]
fn local_folder_reuses_descriptor_identity_without_path_or_members() {
    let origin = Origin {
        provider: "local".into(),
        profile: "Local videos".into(),
        subprofile: None,
        source_alias: "Fixture".into(),
    };
    let pin =
        Stored::local_folder(origin.clone(), false, "source-1".into(), "a".repeat(64)).unwrap();
    assert!(pin.pins().is_empty());
    let encoded = pin.encode().unwrap();
    assert_eq!(encoded["target"].as_object().unwrap().len(), 3);
    assert_eq!(read(&settings(encoded)).unwrap().unwrap(), pin);
    assert!(Stored::local_folder(
        origin.clone(),
        false,
        "/private/path".into(),
        "a".repeat(64)
    )
    .is_err());
    assert!(Stored::local_folder(origin, false, "source-1".into(), "bad".into()).is_err());
}
#[test]
fn armed_rejects_hidden_pin_and_queue_never_truncates() {
    assert!(read(&settings(policy(json!({"kind":"armed","entry":entry()})))).is_err());
    let item: Favorite = serde_json::from_value(entry()).unwrap();
    assert!(Stored::queue(vec![item.clone(); ENTRY_COUNT], true).is_ok());
    assert!(Stored::queue(vec![item; ENTRY_COUNT + 1], true).is_err());
}
#[test]
fn borrowed_queue_is_same_json_storage_and_matches_full_identity() {
    let value = settings(policy(json!({"kind":"queue","entries":[entry(),entry()]})));
    let full = read(&value).unwrap().unwrap();
    let view = borrow_validated(&value).unwrap().unwrap();
    assert_eq!(view.state(), full.state());
    let BorrowedTarget::Queue(entries) = view.target() else {
        panic!("queue")
    };
    assert_eq!(entries.len(), 2);
    let raw = value[KEY]["target"]["entries"].as_array().unwrap();
    assert!(std::ptr::eq(entries.0, raw.as_slice()));
    for (index, item) in full.pins().iter().enumerate() {
        assert!(entries.get(index).unwrap().equals(item));
        let mut renamed = item.clone();
        renamed.title = "Renamed".into();
        assert!(entries.get(index).unwrap().matches(&renamed));
        assert!(!entries.get(index).unwrap().equals(&renamed));
        renamed.origin.profile = "Other".into();
        assert!(!entries.get(index).unwrap().matches(&renamed));
    }
    assert!(entries.get(2).is_none());
}
#[test]
fn borrowed_plex_matching_uses_canonical_full_origin() {
    let item = Favorite {
        title: "Film".into(),
        origin: Origin {
            provider: "plex".into(),
            profile: "Plex".into(),
            subprofile: None,
            source_alias: "server-1".into(),
        },
        locator: plx_data::ott::favorites::Locator::Plex {
            server_id: "server-1".into(),
            rating_key: "123".into(),
        },
    };
    let value = settings(Stored::item(item.clone(), false).unwrap().encode().unwrap());
    let BorrowedTarget::Item(pin) = borrow_validated(&value).unwrap().unwrap().target() else {
        panic!("item")
    };
    assert!(pin.matches_plex("server-1", "123"));
    assert!(!pin.matches_plex("server-2", "123"));
    assert!(!pin.matches_plex("server-1", "124"));
    let mut changed = item;
    changed.origin.subprofile = Some("Different".into());
    let changed = settings(Stored::item(changed, false).unwrap().encode().unwrap());
    let BorrowedTarget::Item(pin) = borrow_validated(&changed).unwrap().unwrap().target() else {
        panic!("item")
    };
    assert!(!pin.matches_plex("server-1", "123"));
}
#[test]
fn full_validation_precedes_borrowed_projection_for_every_present_value() {
    let mut bad_locator = policy(json!({"kind":"item","entry":entry()}));
    bad_locator["target"]["entry"]["locator"]["channelId"] = json!("channel:1");
    let mut bad_origin = policy(json!({"kind":"armed"}));
    bad_origin["origin"]["sourceAlias"] = json!("https://secret.invalid");
    for value in [
        Value::Null,
        json!({}),
        policy(json!({"kind":"armed","hidden":true})),
        bad_locator,
        bad_origin,
    ] {
        let image = settings(value);
        // This models the required Saved::validate admission gate. No invalid
        // image is ever projected as an unrestricted Runtime policy.
        assert!(read(&image).is_err());
    }
}

#[test]
fn strict_only_changes_preserve_target_but_never_hide_target_changes() {
    let first: Favorite = serde_json::from_value(entry()).unwrap();
    let pin = Stored::item(first.clone(), true).unwrap();
    assert!(pin.same_target(&pin.with_strict(false)));
    let mut other = first;
    other.origin.profile = "Different".into();
    assert!(!pin.same_target(&Stored::item(other, true).unwrap()));
    assert!(!pin.same_target(&Stored::armed(pin.origin().clone(), true).unwrap()));
}

fn attached_entry() -> Favorite {
    let mut value = entry();
    value["locator"] = json!({"kind":"vportal-media","idField":"id","id":"film-1"});
    serde_json::from_value(value).unwrap()
}
fn attachment_binding() -> KioskBinding {
    serde_json::from_value(json!({"parent_profile_id":"parent-1","attachment_id":"attachment-1",
        "source_fingerprint":"a".repeat(64)})).unwrap()
}

#[test]
fn attached_item_roundtrip_preserves_strictness_and_existing_pin_projection() {
    let entry = attached_entry();
    let binding = attachment_binding();
    for strict in [false, true] {
        let pin = Stored::attached_item(entry.clone(), strict, binding.clone()).unwrap();
        assert_eq!(pin.attachment_binding(), Some(&binding));
        assert_eq!(pin.pins(), std::slice::from_ref(&entry));
        let TargetRef::Item(target) = pin.target() else { panic!("item"); };
        assert_eq!(target, &entry);
        let image = settings(pin.encode().unwrap());
        assert_eq!(read(&image).unwrap(), Some(pin.clone()));
        let view = borrow_validated(&image).unwrap().unwrap();
        assert_eq!(view.state(), State::Locked { strict });
        let BorrowedTarget::Item(target) = view.target() else { panic!("borrowed item"); };
        assert!(target.equals(&entry));
        assert!(pin.same_target(&pin.with_strict(!strict)));
    }
}

#[test]
fn attached_item_target_identity_includes_the_durable_binding() {
    let entry = attached_entry();
    let original = Stored::attached_item(entry.clone(), true, attachment_binding()).unwrap();
    for field in ["parent_profile_id", "attachment_id", "source_fingerprint"] {
        let mut encoded = serde_json::to_value(attachment_binding()).unwrap();
        encoded[field] = if field == "source_fingerprint" { json!("b".repeat(64)) } else { json!("other-id") };
        let binding = serde_json::from_value(encoded).unwrap();
        let changed = Stored::attached_item(entry.clone(), true, binding).unwrap();
        assert!(!original.same_target(&changed), "{field}");
    }
    assert!(!original.same_target(&Stored::item(entry, true).unwrap()));
}

#[test]
fn attachment_metadata_is_only_valid_on_m3u_vportal_item_pins() {
    let portal = attached_entry();
    assert!(Stored::attached_item(serde_json::from_value(entry()).unwrap(), true, attachment_binding()).is_err());
    let mut standalone = portal.clone(); standalone.origin.provider = "vportal".into();
    assert!(Stored::attached_item(standalone, true, attachment_binding()).is_err());
    let binding = serde_json::to_value(attachment_binding()).unwrap();
    for target in [json!({"kind":"armed","attachment":binding}),
        json!({"kind":"queue","entries":[portal],"attachment":binding})] {
        assert!(read(&settings(policy(target))).is_err());
    }
    let good = Stored::attached_item(attached_entry(), true, attachment_binding()).unwrap().encode().unwrap();
    for invalid in [json!({}), json!(true), json!({"parent_profile_id":"parent-1",
        "attachment_id":"attachment-1","source_fingerprint":"a".repeat(64),"link":"private"}),
        json!({"parent_profile_id":"parent-1","attachment_id":"attachment-1","source_fingerprint":"A".repeat(64)})] {
        let mut changed = good.clone(); changed["target"]["attachment"] = invalid;
        let image = settings(changed);
        assert!(read(&image).is_err());
        assert!(borrow_validated(&image).is_err(), "must not downgrade malformed attachment to an ordinary pin");
    }
}

#[test]
fn old_item_documents_and_portable_favorite_encoding_remain_unchanged() {
    for item in [serde_json::from_value::<Favorite>(entry()).unwrap(), attached_entry()] {
        let portable = serde_json::to_value(&item).unwrap();
        let old = policy(json!({"kind":"item","entry":portable}));
        let pin = read(&settings(old.clone())).unwrap().unwrap();
        assert!(pin.attachment_binding().is_none());
        assert_eq!(pin.encode().unwrap(), old);
        assert_eq!(Stored::item(item.clone(), true).unwrap(), pin);
        assert_eq!(serde_json::to_value(&item).unwrap(), portable);
        let mut explicit_none = old; explicit_none["target"]["attachment"] = Value::Null;
        let image = settings(explicit_none);
        assert_eq!(read(&image).unwrap(), Some(pin));
        assert!(borrow_validated(&image).is_ok());
    }
}

use super::*;

#[test]
fn absent_settings_keep_defaults_and_every_item_roundtrips() {
    let base = Saved::default();
    assert!(Stored::read(&base).unwrap().visibility().is_default());
    for item in Item::ALL {
        let edited = candidate(&base, Change::Set { item, visible: false }).unwrap();
        edited.validate().unwrap();
        let encoded = serde_json::to_vec(&edited).unwrap();
        let restored: Saved = serde_json::from_slice(&encoded).unwrap();
        let value = Stored::read(&restored).unwrap();
        assert!(!value.visibility().visible(item));
        assert_eq!(value.revision, 1);
        for other in Item::ALL { assert_eq!(value.visibility().visible(other), other != item); }
        assert_eq!(snapshot(&restored, item)["persisted_value"], 0);
        assert_eq!(snapshot(&restored, item)["value"], 0);
    }
}

#[test]
fn malformed_present_state_is_rejected_without_defaulting_or_mutation() {
    for value in [Value::Null, json!({"version":2,"revision":0,"hidden":[]}),
        json!({"version":1,"revision":0,"hidden":["settings"]}),
        json!({"version":1,"revision":0,"hidden":["back"]}),
        json!({"version":1,"revision":0,"hidden":[],"extra":true}),
        json!({"version":1,"revision":-1,"hidden":[]})] {
        let mut saved = Saved::default();
        saved.settings.insert(KEY.into(), value.clone());
        assert!(saved.validate().is_err());
        assert!(candidate(&saved, Change::Reset).is_err());
        assert_eq!(snapshot(&saved, Item::Audio)["storage"], "invalid");
        assert_eq!(saved.settings[KEY], value);
    }
}

#[test]
fn edits_share_compound_revision_and_reset_does_not_rewind_it() {
    let base = Saved::default();
    let audio = candidate(&base, Change::Set { item: Item::Audio, visible: false }).unwrap();
    let both = candidate(&audio, Change::Set { item: Item::Subtitles, visible: false }).unwrap();
    let reset = candidate(&both, Change::Reset).unwrap();
    assert_eq!(Stored::read(&both).unwrap().revision, 2);
    assert_eq!(Stored::read(&reset).unwrap().revision, 3);
    assert!(Stored::read(&reset).unwrap().visibility().is_default());
    assert_ne!(reset.settings.get(KEY), base.settings.get(KEY));
    assert!(!Stored::read(&both).unwrap().visibility().visible(Item::Audio));
}

#[test]
fn revision_exhaustion_is_an_error_and_preserves_the_document() {
    let mut saved = Saved::default();
    saved.settings.insert(KEY.into(), json!({"version":1,"revision":u64::MAX,"hidden":["audio"]}));
    let before = saved.settings.clone();
    assert!(candidate(&saved, Change::Reset).is_err());
    assert_eq!(saved.settings, before);
}

#[test]
fn descriptors_advertise_v1_integer_choices_with_constraints() {
    for item in Item::ALL {
        let field = descriptor(item);
        assert_eq!(field["id"], item.setting());
        assert_eq!(field["type"], "integer");
        assert_eq!(field["constraints"], json!({"values":[0,1]}));
        assert_eq!(field["default_value"], 1);
        assert_eq!(field["access"]["write"], true);
        assert_eq!(field["choices"][0]["value"], 1);
        assert_eq!(field["choices"][1]["value"], 0);
    }
}

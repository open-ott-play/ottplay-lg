use super::*;
use serde_json::json;

fn bookmark(source: u8) -> ClassicBookmark {
    ClassicBookmark { source: format!("{source:064x}"), category: None, focus: None }
}
fn preferences() -> PublicPreferences {
    PublicPreferences { version: 1, device_id: Some("existing-device".into()),
        remote_address: Some("https://control.invalid:443/team".into()), interface_mode: Some(1), notifications_enabled: None,
        classic_bookmarks: Vec::new() }
}

#[test]
fn classic_bookmark_old_public_documents_keep_their_wire_shape() {
    for text in [r#"{"version":1}"#,
        r#"{"version":1,"device_id":"existing-device","remote_address":"https://control.invalid:443/team","interface_mode":1}"#]
    {
        let parsed: PublicPreferences = serde_json::from_str(text).unwrap();
        parsed.validate().unwrap();
        assert!(parsed.classic_bookmarks.is_empty());
        assert_eq!(serde_json::to_string(&parsed).unwrap(), text);
    }
    assert_eq!(serde_json::to_value(PublicPreferences::default()).unwrap(), json!({"version":1}));
}

#[test]
fn classic_bookmark_is_closed_and_debug_redacts_every_field() {
    let mut value = bookmark(17);
    value.category = Some("a".repeat(64)); value.focus = Some("12345".into());
    value.validate().unwrap();
    let encoded = serde_json::to_value(&value).unwrap();
    assert_eq!(serde_json::from_value::<ClassicBookmark>(encoded.clone()).unwrap(), value);
    assert_eq!(format!("{value:?}"), "ClassicBookmark([REDACTED])");
    let patch = PublicPatch::ClassicBookmark(value.clone());
    assert_eq!(format!("{patch:?}"), "PublicPatch([REDACTED])");
    let patched = preferences().apply(&patch).unwrap();
    assert_eq!(format!("{patched:?}"), "PublicPreferences([REDACTED])");
    for key in ["token", "url", "profile", "extra"] {
        let mut invalid = encoded.clone(); invalid[key] = json!("must-not-be-accepted");
        assert!(serde_json::from_value::<ClassicBookmark>(invalid).is_err());
    }
    let all: ClassicBookmark = serde_json::from_value(json!({"source": value.source})).unwrap();
    assert!(all.category.is_none() && all.focus.is_none());
    all.validate().unwrap();
}

#[test]
fn classic_bookmark_rejects_non_digest_sources_and_categories() {
    for invalid in [String::new(), "a".repeat(63), "a".repeat(65), "A".repeat(64),
        "g".repeat(64), "https://media.invalid:443/token".into(), "Bearer secret".into()]
    {
        let mut value = bookmark(1); value.source = invalid.clone();
        assert_eq!(value.validate(), Err(ErrorCode::Invalid));
        let mut value = bookmark(1); value.category = Some(invalid);
        assert_eq!(value.validate(), Err(ErrorCode::Invalid));
    }
    for valid in ["0".repeat(64), "f".repeat(64), "0123456789abcdef".repeat(4)] {
        let value = ClassicBookmark { source: valid.clone(), category: Some(valid), focus: None };
        value.validate().unwrap();
    }
}

#[test]
fn classic_bookmark_focus_matches_bounded_plex_rating_key_shape() {
    for invalid in [String::new(), "+1".into(), " 1".into(), "1 ".into(), "1.0".into(),
        "١".into(), "/library/metadata/42".into(), "folder/file".into(), "profile:42".into(),
        "https://media.invalid/42".into(), "token=42".into(), "%34%32".into(),
        "Bearer secret".into(), "key\n".into(), "a".repeat(257)]
    {
        let mut value = bookmark(1); value.focus = Some(invalid);
        assert_eq!(value.validate(), Err(ErrorCode::Invalid));
    }
    for valid in ["0".into(), "001".into(), "-1".into(), "_".into(), "ABCdef-012_".into(),
        "structural-key_42".into(), "a".repeat(256)] {
        let mut value = bookmark(1); value.focus = Some(valid); value.validate().unwrap();
    }
}

#[test]
fn classic_bookmark_collection_rejects_duplicates_overflow_and_wrong_shapes() {
    let mut values = preferences(); values.classic_bookmarks = vec![bookmark(1), bookmark(1)];
    assert_eq!(values.validate(), Err(ErrorCode::Invalid));
    values.classic_bookmarks = (0..17).map(bookmark).collect();
    assert_eq!(values.validate(), Err(ErrorCode::Invalid));
    values.classic_bookmarks.truncate(16); values.validate().unwrap();
    for invalid in [json!({"version":1,"classic_bookmarks":null}),
        json!({"version":1,"classic_bookmarks":{}}),
        json!({"version":1,"classic_bookmarks":[{"source":123}]}),
        json!({"version":1,"classic_bookmarks":[{"source":"a".repeat(64),"focus":42}]}),
        json!({"version":1,"classic_bookmarks":[],"unknown":true})]
    { assert!(serde_json::from_value::<PublicPreferences>(invalid).is_err()); }
}

#[test]
fn classic_bookmark_upsert_moves_only_its_source_and_preserves_other_fields() {
    let original = preferences();
    let first = original.apply(&PublicPatch::ClassicBookmark(bookmark(1))).unwrap();
    let second = first.apply(&PublicPatch::ClassicBookmark(bookmark(2))).unwrap();
    let mut replacement = bookmark(1);
    replacement.category = Some("b".repeat(64)); replacement.focus = Some("987".into());
    let updated = second.apply(&PublicPatch::ClassicBookmark(replacement.clone())).unwrap();
    assert_eq!(updated.classic_bookmarks, vec![bookmark(2), replacement]);
    assert_eq!(updated.version, original.version); assert_eq!(updated.device_id, original.device_id);
    assert_eq!(updated.remote_address, original.remote_address);
    assert_eq!(updated.interface_mode, original.interface_mode);
    assert_eq!(second.classic_bookmarks, vec![bookmark(1), bookmark(2)]);
    let all = updated.apply(&PublicPatch::ClassicBookmark(bookmark(1))).unwrap();
    assert_eq!(all.classic_bookmarks, vec![bookmark(2), bookmark(1)]);
    assert_eq!(all.apply(&PublicPatch::InterfaceMode(0)).unwrap().classic_bookmarks, all.classic_bookmarks);
}

#[test]
fn classic_bookmark_bounded_eviction_uses_last_upsert_order() {
    let mut values = preferences();
    for source in 0..16 { values = values.apply(&PublicPatch::ClassicBookmark(bookmark(source))).unwrap(); }
    values = values.apply(&PublicPatch::ClassicBookmark(bookmark(0))).unwrap();
    assert_eq!(values.classic_bookmarks.len(), 16);
    assert_eq!(values.classic_bookmarks.last(), Some(&bookmark(0)));
    let before = values.clone();
    values = values.apply(&PublicPatch::ClassicBookmark(bookmark(16))).unwrap();
    assert_eq!(values.classic_bookmarks.len(), 16);
    assert_eq!(values.classic_bookmarks.first(), Some(&bookmark(2)));
    assert_eq!(values.classic_bookmarks.last(), Some(&bookmark(16)));
    assert!(!values.classic_bookmarks.iter().any(|value| value.source == bookmark(1).source));
    assert_eq!(before.classic_bookmarks.first(), Some(&bookmark(1)));
    values.validate().unwrap();
}

#[test]
fn classic_bookmark_invalid_patch_never_repairs_or_changes_existing_preferences() {
    let original = preferences(); let before = serde_json::to_value(&original).unwrap();
    let mut invalid = bookmark(1); invalid.focus = Some("invalid/key".into());
    assert_eq!(original.apply(&PublicPatch::ClassicBookmark(invalid)), Err(ErrorCode::Invalid));
    assert_eq!(serde_json::to_value(&original).unwrap(), before);
    let mut invalid = original; invalid.classic_bookmarks = vec![bookmark(1), bookmark(1)];
    assert_eq!(invalid.apply(&PublicPatch::ClassicBookmark(bookmark(1))), Err(ErrorCode::Invalid));
    let patch = PublicPatch::ClassicBookmark(bookmark(1));
    let encoded = serde_json::to_value(&patch).unwrap();
    assert_eq!(encoded["field"], "classic_bookmark");
    assert_eq!(serde_json::from_value::<PublicPatch>(encoded).unwrap(), patch);
}

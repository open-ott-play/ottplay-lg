//! Source-only runtime wiring regressions; no worker or native player is started.
use super::*;
use plx_data::ott::reminders::{Book, CommitOutcome, Record};

fn saved_reminder() -> Saved {
    let mut saved = Saved::default();
    let mut book = Book::new(Default::default(), 100, 1).unwrap();
    let item: Record = serde_json::from_value(json!({
        "key":{"source_id":"source","channel_id":"channel","programme_id":"programme","start_s":200},
        "end_s":800,"title":"Programme"
    })).unwrap();
    let candidate = book.upsert(item, 100).unwrap();
    book.commit(candidate.token, CommitOutcome::Committed)
        .unwrap();
    saved.reminders = book.document().clone();
    saved
}

#[test]
fn legacy_saved_defaults_reminders_without_erasing_other_preferences() {
    let saved: Saved = serde_json::from_value(json!({"version":1,"category":"Favorites"})).unwrap();
    assert!(saved.reminders.records().is_empty());
    assert_eq!(saved.category, "Favorites");
}

#[test]
fn reminder_document_roundtrips_in_the_existing_saved_image() {
    let saved = saved_reminder();
    let decoded: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    assert_eq!(decoded.reminders, saved.reminders);
}

#[test]
fn source_fingerprint_changes_when_a_profile_id_is_reused_for_another_source() {
    let first = ProviderProfile {
        id: "same".into(),
        name: "Source".into(),
        kind: ProviderKind::M3u,
        source: "https://first.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let mut second = first.clone();
    second.source = "https://second.invalid/list.m3u".into();
    assert_ne!(profile_fingerprint(&first), profile_fingerprint(&second));
}

#[test]
fn malformed_reminders_are_not_interpreted_as_an_empty_legacy_document() {
    let invalid = json!({"version":1,"reminders":{"version":1,"records":[{
        "key":{"source_id":"source","channel_id":"channel","programme_id":"programme","start_s":200},
        "end_s":100,"title":"Invalid"
    }]}});
    let saved: Saved = serde_json::from_value(invalid).unwrap();
    assert!(saved.validate().is_err());
}

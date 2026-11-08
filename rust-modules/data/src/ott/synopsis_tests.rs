use super::*;

#[test]
fn synopsis_requires_exact_source_text_and_one_projection_validation() {
    let description = "2026\n90 мин\nНачало. Полное описание.";
    let start = "2026\n90 мин\n".len();
    let source = SynopsisSpan::new(description, start).unwrap();
    assert_eq!(
        source.get(description),
        None,
        "raw or deserialized annotations are not UI authority"
    );
    let projected = source.validated_for(description).unwrap();
    assert_eq!(projected.get(description), Some("Начало. Полное описание."));
    assert!(
        source
            .validated_for(&description.replace("Начало", "Финалы"))
            .is_none(),
        "equal-length replacement is still different metadata"
    );
    assert!(source.validated_for("shorter").is_none());
    assert!(
        SynopsisSpan::new("я", 1).is_none(),
        "never slice inside UTF-8"
    );
    assert!(SynopsisSpan::new("text", 5).is_none());
    assert!(SynopsisSpan::new(&"a".repeat(MAX_METADATA_BYTES + 1), 0).is_none());
    assert_eq!(
        SynopsisSpan::new("facts", 5)
            .unwrap()
            .validated_for("facts")
            .unwrap()
            .get("facts"),
        Some("")
    );
}

#[test]
fn synopsis_serialization_preserves_source_annotation_but_not_validation() {
    let source = SynopsisSpan::new("2026\nStory", 5).unwrap();
    let json = serde_json::to_value(&source).unwrap();
    let restored: SynopsisSpan = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(source, restored);
    let validated = source.validated_for("2026\nStory").unwrap();
    assert_eq!(serde_json::to_value(validated).unwrap(), json);
    assert_eq!(restored.get("2026\nStory"), None);
    let mut injected = json;
    injected["validated"] = true.into();
    assert!(serde_json::from_value::<SynopsisSpan>(injected).is_err());
}

#[test]
fn older_catalogue_entries_without_synopsis_still_deserialize() {
    let json = serde_json::json!({
        "id":"legacy", "title":"Title", "category":"Videos", "description":"Unchanged prose",
        "poster":"", "media":{"kind":"http","url":"https://example.invalid/video"},
        "duration_ms":null, "provider_id":null
    });
    let entry: CatalogEntry = serde_json::from_value(json).unwrap();
    assert!(entry.synopsis.is_none());
    let encoded = serde_json::to_value(&entry).unwrap();
    assert!(encoded.get("synopsis").is_none());
    assert_eq!(
        serde_json::from_value::<CatalogEntry>(encoded).unwrap(),
        entry
    );
}

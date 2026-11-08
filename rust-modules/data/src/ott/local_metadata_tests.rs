use super::*;
use crate::ott::{catalog_from_local_files, LocalFile, ProviderKind, ProviderProfile};
use serde_json::{json, Value};

fn profile(root: &str) -> ProviderProfile {
    ProviderProfile {
        id: "local-home".into(),
        name: "Local videos".into(),
        kind: ProviderKind::Local,
        source: root.into(),
        source_alias: "Family".into(),
        enabled: true,
    }
}

fn entry(path: &str, root: &str) -> CatalogEntry {
    catalog_from_local_files(
        &[LocalFile {
            relative_path: path.into(),
            size: 1234,
            duration_ms: Some(1000),
        }],
        &profile(root),
    )
    .unwrap()
    .entries
    .remove(0)
}

fn parse(items: Value) -> LocalMetadataMap {
    LocalMetadataMap::parse(&serde_json::to_vec(&json!({"version":1,"items":items})).unwrap())
        .unwrap()
}

fn invalid(value: Value) {
    assert!(LocalMetadataMap::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn exact_path_distinguishes_equal_basenames_and_keeps_catalog_order() {
    let map = parse(json!([
        {"relative_path":"One/Film.mkv","title":"First film"},
        {"relative_path":"Two/Film.mkv","title":"Second film"}
    ]));
    let mut rows = vec![
        entry("Two/Film.mkv", "/media/ssd"),
        entry("One/Film.mkv", "/media/ssd"),
        entry("Three/Film.mkv", "/media/ssd"),
    ];
    let identities: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
    for row in &mut rows {
        map.apply_to(row);
    }
    assert_eq!(rows[0].title, "Second film");
    assert_eq!(rows[1].title, "First film");
    assert_eq!(rows[2].title, "Film.mkv");
    assert_eq!(
        identities,
        rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
    );
    assert!(map.lookup("Film.mkv").is_none());
}

#[test]
fn exact_unicode_and_case_are_never_normalized_or_guessed() {
    let map = parse(json!([
        {"relative_path":"Семья/Épisode.MKV","title":"Exact title"}
    ]));
    assert!(map.lookup("Семья/Épisode.MKV").is_some());
    for path in [
        "семья/Épisode.MKV",
        "Семья/épisode.MKV",
        "Семья/E\u{301}pisode.MKV",
    ] {
        assert!(map.lookup(path).is_none(), "{path}");
    }
}

#[test]
fn display_overlay_preserves_all_identity_provider_and_locator_fields() {
    let map = parse(json!([{
        "relative_path":"Family/Film.mkv", "title":"Film title",
        "description":"First sentence. Second sentence.\nA full paragraph.",
        "duration_ms":120000, "poster_relative_path":".ottplay/posters/film.jpg"
    }]));
    let mut row = entry("Family/Film.mkv", "/media/ssd");
    row.provider_id = Some("existing-id".into());
    row.provider_id_field = Some("existing-field".into());
    row.poster = "retained-artwork".into();
    let original = row.clone();
    let metadata = map.apply_to(&mut row).unwrap();
    assert_eq!(row.title, "Film title");
    assert_eq!(
        row.description,
        "First sentence. Second sentence.\nA full paragraph."
    );
    assert_eq!(row.duration_ms, Some(120000));
    assert_eq!(
        metadata.poster_relative_path.as_deref(),
        Some(".ottplay/posters/film.jpg")
    );
    assert_eq!(row.id, original.id);
    assert_eq!(row.media, original.media);
    assert_eq!(row.category, original.category);
    assert_eq!(row.provider_id, original.provider_id);
    assert_eq!(row.provider_id_field, original.provider_id_field);
    assert_eq!(row.poster, original.poster);
}

#[test]
fn mount_relocation_does_not_change_metadata_or_local_identity() {
    let map = parse(json!([{"relative_path":"Film.mp4","title":"A film"}]));
    let mut first = entry("Film.mp4", "/media/usb-one");
    let mut second = entry("Film.mp4", "/media/usb-two");
    map.apply_to(&mut first);
    map.apply_to(&mut second);
    assert_eq!(first, second);
}

#[test]
fn missing_paths_and_nonlocal_locators_are_unchanged() {
    let map = parse(json!([{"relative_path":"Film.mp4","title":"A film"}]));
    let mut absent = entry("Missing.mp4", "/media/ssd");
    let original = absent.clone();
    assert!(map.apply_to(&mut absent).is_none());
    assert_eq!(absent, original);
    for media in [
        MediaLocator::Plex {
            server_id: "server".into(),
            rating_key: "1".into(),
        },
        MediaLocator::Http {
            url: "https://example.test/Film.mp4".into(),
        },
        MediaLocator::ServerLibrary {
            source_id: "root".into(),
            item_id: "1".into(),
            relative_path: Some("Film.mp4".into()),
            root_alias: Some("Family".into()),
        },
    ] {
        let mut row = entry("Film.mp4", "/media/ssd");
        row.media = media;
        let original = row.clone();
        assert!(map.apply_to(&mut row).is_none());
        assert_eq!(row, original);
    }
}

#[test]
fn omitted_fields_stay_unchanged_and_explicit_empty_description_clears() {
    let map = parse(json!([{"relative_path":"Film.mp4","description":""}]));
    let mut row = entry("Film.mp4", "/media/ssd");
    row.description = "Old description".into();
    let before = row.clone();
    map.apply_to(&mut row).unwrap();
    assert!(row.description.is_empty());
    assert_eq!(row.title, before.title);
    assert_eq!(row.duration_ms, before.duration_ms);
}

#[test]
fn poster_only_is_retained_metadata_and_never_a_renderable_file_url() {
    let map = parse(json!([{
        "relative_path":"Film.mp4","poster_relative_path":"Art/Film.PNG"
    }]));
    let mut row = entry("Film.mp4", "/media/ssd");
    let original = row.clone();
    assert_eq!(
        map.apply_to(&mut row)
            .unwrap()
            .poster_relative_path
            .as_deref(),
        Some("Art/Film.PNG")
    );
    assert_eq!(row, original);
}

#[test]
fn duplicate_paths_reject_even_identical_rows_before_any_overlay() {
    let bytes = br#"{"version":1,"items":[{"relative_path":"Film.mp4","title":"A"},{"relative_path":"Film.mp4","title":"A"}]}"#;
    assert!(matches!(
        LocalMetadataMap::parse(bytes),
        Err(Error::DuplicatePath)
    ));
    let before = entry("Film.mp4", "/media/ssd");
    let mut after = before.clone();
    if let Ok(map) = LocalMetadataMap::parse(bytes) {
        map.apply_to(&mut after);
    }
    assert_eq!(before, after);
}

#[test]
fn raw_duplicate_required_fields_are_rejected_at_each_object() {
    for bytes in [
        r#"{"version":1,"version":1,"items":[]}"#,
        r#"{"version":null,"version":1,"items":[]}"#,
        r#"{"version":1,"items":[],"items":[]}"#,
        r#"{"version":1,"items":null,"items":[]}"#,
        r#"{"version":1,"items":[{"relative_path":"Film.mp4","relative_path":"Film.mp4","title":"A"}]}"#,
        r#"{"version":1,"items":[{"relative_path":null,"relative_path":"Film.mp4","title":"A"}]}"#,
    ] {
        assert!(
            LocalMetadataMap::parse(bytes.as_bytes()).is_err(),
            "{bytes}"
        );
    }
}

#[test]
fn raw_optional_duplicates_including_null_first_are_rejected() {
    for (field, value) in [
        ("title", r#""Film""#),
        ("description", r#""Description""#),
        ("duration_ms", "1234"),
        ("poster_relative_path", r#""Art/Film.jpg""#),
    ] {
        for first in [value, "null"] {
            let bytes = format!(
                r#"{{"version":1,"items":[{{"relative_path":"Film.mp4","{field}":{first},"{field}":{value}}}]}}"#
            );
            assert!(
                LocalMetadataMap::parse(bytes.as_bytes()).is_err(),
                "{field}"
            );
        }
        let bytes =
            format!(r#"{{"version":1,"items":[{{"relative_path":"Film.mp4","{field}":null}}]}}"#);
        assert!(
            LocalMetadataMap::parse(bytes.as_bytes()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn unknown_fields_cannot_grant_sources_paths_or_plex_authority() {
    for field in [
        "root",
        "source_id",
        "authority_ref",
        "generation",
        "ratingKey",
        "Part",
        "file",
    ] {
        invalid(json!({"version":1,"items":[],(field):"unexpected"}));
        invalid(
            json!({"version":1,"items":[{"relative_path":"Film.mp4","title":"A",(field):"unexpected"}]}),
        );
    }
}

#[test]
fn media_and_poster_paths_refuse_traversal_absolute_urls_and_controls() {
    for path in [
        "",
        "../Film.mp4",
        "/Film.mp4",
        "a/../Film.mp4",
        "a/./Film.mp4",
        "a//Film.mp4",
        "C:/Film.mp4",
        "https://example.test/Film.mp4",
        "file:Film.mp4",
        "a\\Film.mp4",
        "a\u{0}Film.mp4",
        "a\nFilm.mp4",
        "notes.txt",
    ] {
        invalid(json!({"version":1,"items":[{"relative_path":path,"title":"A"}]}));
    }
    for path in [
        "",
        "../Film.jpg",
        "/Film.jpg",
        "a/../Film.jpg",
        "a/./Film.jpg",
        "a//Film.jpg",
        "C:/Film.jpg",
        "https://example.test/Film.jpg",
        "file:Film.jpg",
        "a\\Film.jpg",
        "a\u{0}Film.jpg",
        "a\nFilm.jpg",
        "Film.svg",
        "Film.mp4",
    ] {
        invalid(
            json!({"version":1,"items":[{"relative_path":"Film.mp4","poster_relative_path":path}]}),
        );
    }
}

#[test]
fn duration_must_be_a_positive_bounded_integer() {
    for duration in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("1234"),
        json!(null),
        json!(u64::MAX),
    ] {
        invalid(json!({"version":1,"items":[{"relative_path":"Film.mp4","duration_ms":duration}]}));
    }
    let map = parse(json!([{"relative_path":"Film.mp4","duration_ms":i64::MAX}]));
    assert_eq!(
        map.lookup("Film.mp4").unwrap().duration_ms,
        Some(i64::MAX as u64)
    );
}

#[test]
fn field_byte_limits_keep_complete_utf8_and_description_paragraphs() {
    let title = "é".repeat(512);
    let description = "я".repeat(MAX_METADATA_BYTES / 2);
    let map = parse(json!([{"relative_path":"Film.mp4","title":title,"description":description}]));
    assert_eq!(
        map.lookup("Film.mp4")
            .unwrap()
            .title
            .as_ref()
            .unwrap()
            .len(),
        1024
    );
    invalid(json!({"version":1,"items":[{"relative_path":"Film.mp4","title":"é".repeat(513)}]}));
    invalid(
        json!({"version":1,"items":[{"relative_path":"Film.mp4","description":"я".repeat(MAX_METADATA_BYTES/2+1)}]}),
    );
    for title in ["", " ", "A\nB", "A\tB", "A\u{7f}B"] {
        invalid(json!({"version":1,"items":[{"relative_path":"Film.mp4","title":title}]}));
    }
    invalid(json!({"version":1,"items":[{"relative_path":"Film.mp4","description":"A\u{1}B"}]}));
}

#[test]
fn path_byte_limit_is_the_shared_local_limit() {
    let path = format!("{}.mp4", "a".repeat(8192 - 4));
    let map = parse(json!([{"relative_path":path,"title":"A"}]));
    assert!(map.lookup(&path).is_some());
    let oversized = format!("a{path}");
    invalid(json!({"version":1,"items":[{"relative_path":oversized,"title":"A"}]}));
    let poster = format!("{}.jpg", "a".repeat(8192 - 4));
    parse(json!([{"relative_path":"Film.mp4","poster_relative_path":poster}]));
    invalid(
        json!({"version":1,"items":[{"relative_path":"Film.mp4","poster_relative_path":format!("a{poster}")}]}),
    );
}

#[test]
fn malformed_and_unsupported_envelopes_never_produce_a_map() {
    for value in [
        json!({}),
        json!([]),
        json!(null),
        json!({"version":1}),
        json!({"items":[]}),
        json!({"version":1,"items":{}}),
        json!({"version":1,"items":[{"relative_path":"Film.mp4"}]}),
        json!({"version":1,"items":[{"relative_path":"Film.mp4","title":{"nested":"A"}}]}),
    ] {
        invalid(value);
    }
    assert!(matches!(
        LocalMetadataMap::parse(br#"{"version":2,"items":[]}"#),
        Err(Error::UnsupportedVersion)
    ));
    assert!(LocalMetadataMap::parse(br#"{"version":1,"items":[]} trailing"#).is_err());
    assert!(LocalMetadataMap::parse(b"\xff").is_err());
}

#[test]
fn document_size_and_item_count_are_bounded_before_use() {
    assert!(matches!(
        LocalMetadataMap::parse(&vec![b' '; MAX_CATALOG_BYTES + 1]),
        Err(Error::LimitExceeded)
    ));
    let items: Vec<_> = (0..MAX_CATALOG_ENTRIES)
        .map(|n| json!({"relative_path":format!("{n}.mp4"),"title":"A"}))
        .collect();
    let mut document = json!({"version":1,"items":items});
    let map = LocalMetadataMap::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(map
        .lookup(&format!("{}.mp4", MAX_CATALOG_ENTRIES - 1))
        .is_some());
    document["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"relative_path":"overflow.mp4","title":"A"}));
    invalid(document);
}

#[test]
fn valid_empty_document_is_an_explicit_empty_overlay() {
    let map = parse(json!([]));
    let mut row = entry("Film.mp4", "/media/ssd");
    let before = row.clone();
    assert!(map.apply_to(&mut row).is_none());
    assert_eq!(row, before);
}

use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("media-tracks-v1.json")).unwrap()
}
fn case(name: &str) -> Value {
    fixture()["valid"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap()
        .clone()
}
fn request(params: Value) -> Request {
    Request::decode(&serde_json::to_vec(&json!({"action":"media_tracks","params":params})).unwrap())
        .unwrap()
}
fn from_case(name: &str) -> Request {
    request(case(name)["params"].clone())
}
fn context(owner: u64) -> Context<u64> {
    Context::new(
        owner,
        "native-example".into(),
        "p-example".into(),
        "s-example".into(),
    )
    .unwrap()
}
fn tracks(data: &Value) -> Inventory<usize> {
    Inventory::Complete {
        tracks: data["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, row)| Track {
                id: row["id"].as_str().unwrap().into(),
                kind: if row["kind"] == "audio" {
                    Kind::Audio
                } else {
                    Kind::Subtitle
                },
                label: row["label"].as_str().unwrap().into(),
                language: row["language"].as_str().map(str::to_owned),
                codec: row["codec"].as_str().map(str::to_owned),
                choice: row["selectable"].as_bool().unwrap().then_some(i),
            })
            .collect(),
        subtitle_off: data["subtitle_off_supported"]
            .as_bool()
            .unwrap()
            .then_some(1000),
    }
}
fn ready() -> (Catalog<u64, usize>, Context<u64>) {
    let mut catalog = Catalog::default();
    let live = context(1);
    catalog
        .install(
            &live,
            live.clone(),
            "c-example".into(),
            tracks(&case("list-unobserved")["data"]),
        )
        .unwrap();
    catalog
        .list(&from_case("list-unobserved"), Some(&live), 100)
        .unwrap();
    (catalog, live)
}
fn error(code: &str) -> Error {
    [
        Error::InvalidRequest,
        Error::RuntimeMismatch,
        Error::NoPlayback,
        Error::PlaybackChanged,
        Error::CatalogChanged,
        Error::CatalogIncomplete,
        Error::TrackNotFound,
        Error::NotSelectable,
        Error::Restricted,
        Error::Unsupported,
        Error::Busy,
        Error::Unavailable,
    ]
    .into_iter()
    .find(|e| e.code() == code)
    .unwrap()
}

#[test]
fn shared_fixture_request_shapes_are_closed() {
    let f = fixture();
    for valid in f["valid"].as_array().unwrap() {
        let bytes =
            serde_json::to_vec(&json!({"action":"media_tracks","params":valid["params"]})).unwrap();
        assert!(Request::decode(&bytes).is_ok(), "{}", valid["name"]);
    }
    for invalid in f["invalid_requests"].as_array().unwrap() {
        let bytes =
            serde_json::to_vec(&json!({"action":"media_tracks","params":invalid["params"]}))
                .unwrap();
        assert!(
            matches!(Request::decode(&bytes), Err(Error::InvalidRequest)),
            "{}",
            invalid["name"]
        );
    }
}

#[test]
fn duplicate_keys_and_full_request_bytes_are_rejected_before_dispatch() {
    for bytes in [
        r#"{"action":"media_tracks","action":"media_tracks","params":{"op":"list","runtime":"native-example"}}"#,
        r#"{"action":"media_tracks","params":{"op":"list","op":"list","runtime":"native-example"}}"#,
        r#"{"action":"media_tracks","params":{"op":"list","runtime":"native-example","runtime":"native-example"}}"#,
        r#"{"action":"media_tracks","params":{"op":"select","runtime":"native-example","playback":"p","source_generation":"s","catalog":"c","target":{"kind":"subtitle","kind":"audio","mode":"off"}}}"#,
        r#"{"action":"media_tracks","params":{"op":"select","runtime":"native-example","playback":"p","source_generation":"s","catalog":"c","target":{"kind":"subtitle","mode":"off","mode":"off"}}}"#,
        r#"{"action":"media_tracks","params":{"op":"list","runtime":"native-example"},"extra":null}"#,
        r#"{"action":"other","params":{"op":"list","runtime":"native-example"}}"#,
    ] {
        assert!(
            matches!(
                Request::decode(bytes.as_bytes()),
                Err(Error::InvalidRequest)
            ),
            "{bytes}"
        );
    }
    let mut bytes = serde_json::to_vec(&fixture()["requests"]["list"]).unwrap();
    bytes.resize(MAX_REQUEST_BYTES, b' ');
    assert!(Request::decode(&bytes).is_ok());
    bytes.push(b' ');
    assert!(matches!(
        Request::decode(&bytes),
        Err(Error::InvalidRequest)
    ));
}

#[test]
fn passive_unobserved_and_empty_lists_match_shared_fixture_exactly() {
    for name in ["list-unobserved", "list-empty"] {
        let expected = case(name);
        let live = context(1);
        let mut catalog = Catalog::default();
        catalog
            .install(
                &live,
                live.clone(),
                "c-example".into(),
                tracks(&expected["data"]),
            )
            .unwrap();
        let result = catalog
            .list(&from_case(name), Some(&live), 0)
            .unwrap()
            .into_envelope();
        assert_eq!(
            result,
            json!({"status":expected["status"],"data":expected["data"]})
        );
        assert!(serde_json::to_vec(&result["data"]).unwrap().len() <= MAX_RESULT_BYTES);
    }
}

#[test]
fn resolved_native_choices_only_emit_requested_responses() {
    let (catalog, live) = ready();
    for (name, native) in [
        ("select-audio-requested", 1),
        ("select-subtitle-requested", 2),
        ("select-subtitle-off-requested", 1000),
    ] {
        let expected = case(name);
        let selection = catalog
            .select(&from_case(name), Some(&live), 110, 200)
            .unwrap();
        assert_eq!(*selection.choice(), native);
        catalog.recheck(&selection, Some(&live), 111).unwrap();
        let response = selection.requested_response().into_envelope();
        assert_eq!(
            response,
            json!({"status":expected["status"],"data":expected["data"]})
        );
        assert!(response["data"]["observed"].is_null());
        assert_eq!(response["data"]["outcome"], "requested");
    }
}

#[test]
fn error_projection_matches_every_shared_error_fixture_without_details() {
    for expected in fixture()["valid"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["name"].as_str().unwrap().contains("-error-"))
    {
        let req = request(expected["params"].clone());
        let result = req
            .error_response(error(expected["data"]["error"].as_str().unwrap()))
            .into_envelope();
        assert_eq!(
            result,
            json!({"status":expected["status"],"data":expected["data"]}),
            "{}",
            expected["name"]
        );
    }
}

#[test]
fn list_renewal_cannot_extend_a_captured_selection_deadline() {
    let (mut catalog, live) = ready();
    let req = from_case("select-audio-requested");
    let old = catalog.select(&req, Some(&live), 200, 500).unwrap();
    assert_eq!(old.deadline_ms(), 500);
    catalog
        .list(&from_case("list-unobserved"), Some(&live), 450)
        .unwrap();
    assert!(catalog.recheck(&old, Some(&live), 499).is_ok());
    assert_eq!(
        catalog.recheck(&old, Some(&live), 500),
        Err(Error::Unavailable)
    );
    let lease_limited = catalog.select(&req, Some(&live), 501, 100_000).unwrap();
    assert_eq!(lease_limited.deadline_ms(), 60_450);
    assert!(matches!(
        catalog.select(&req, Some(&live), 60_450, 100_000),
        Err(Error::CatalogChanged)
    ));
    catalog
        .list(&from_case("list-unobserved"), Some(&live), 60_450)
        .unwrap();
    assert_eq!(
        catalog.recheck(&lease_limited, Some(&live), 60_450),
        Err(Error::Unavailable)
    );
    assert!(matches!(
        catalog.select(&req, Some(&live), 60_451, 60_451),
        Err(Error::Unavailable)
    ));
}

#[test]
fn unlisted_inventory_has_no_receipt_and_failed_list_cannot_renew() {
    let live = context(1);
    let mut catalog = Catalog::default();
    catalog
        .install(
            &live,
            live.clone(),
            "c-example".into(),
            tracks(&case("list-unobserved")["data"]),
        )
        .unwrap();
    let req = from_case("select-audio-requested");
    assert!(matches!(
        catalog.select(&req, Some(&live), 0, 1000),
        Err(Error::CatalogChanged)
    ));
    catalog
        .list(&from_case("list-unobserved"), Some(&live), 100)
        .unwrap();
    let other = Context::new(1, "other".into(), "p-example".into(), "s-example".into()).unwrap();
    assert!(matches!(
        catalog.list(&from_case("list-unobserved"), Some(&other), 60_099),
        Err(Error::RuntimeMismatch)
    ));
    assert!(matches!(
        catalog.select(&req, Some(&live), 60_100, 90_000),
        Err(Error::CatalogChanged)
    ));
    assert!(matches!(
        catalog.list(&from_case("list-unobserved"), Some(&live), u64::MAX),
        Err(Error::Unavailable)
    ));
}

#[test]
fn hidden_owner_changes_and_replaced_snapshots_reject_old_handles() {
    let (mut catalog, live) = ready();
    let req = from_case("select-audio-requested");
    let selected = catalog.select(&req, Some(&live), 110, 1000).unwrap();
    let same_wire_new_owner = context(2);
    assert!(matches!(
        catalog.select(&req, Some(&same_wire_new_owner), 111, 1000),
        Err(Error::CatalogChanged)
    ));
    assert_eq!(
        catalog.recheck(&selected, Some(&same_wire_new_owner), 111),
        Err(Error::CatalogChanged)
    );
    catalog
        .install(
            &same_wire_new_owner,
            same_wire_new_owner.clone(),
            "c-new".into(),
            tracks(&case("list-unobserved")["data"]),
        )
        .unwrap();
    assert_eq!(
        catalog.recheck(&selected, Some(&live), 111),
        Err(Error::CatalogChanged)
    );
    assert!(matches!(
        catalog.select(&req, Some(&same_wire_new_owner), 111, 1000),
        Err(Error::CatalogChanged)
    ));
    let fresh = Context::new(3, "native-example".into(), "p-new".into(), "s-new".into()).unwrap();
    assert_eq!(
        catalog.recheck(&selected, Some(&fresh), 111),
        Err(Error::PlaybackChanged)
    );
    assert_eq!(
        catalog.recheck(&selected, None, 111),
        Err(Error::NoPlayback)
    );
    catalog.invalidate();
    assert_eq!(
        catalog.recheck(&selected, Some(&live), 111),
        Err(Error::CatalogChanged)
    );
}

#[test]
fn unknown_oversized_or_duplicate_inventory_fails_as_a_whole() {
    let (mut catalog, live) = ready();
    assert_eq!(
        catalog.install(&live, live.clone(), "c-next".into(), Inventory::Incomplete),
        Err(Error::CatalogIncomplete)
    );
    for (total, audio) in [(129, 64), (65, 65)] {
        let rows = (0..total)
            .map(|n| Track {
                id: format!("row-{n}"),
                kind: if n < audio {
                    Kind::Audio
                } else {
                    Kind::Subtitle
                },
                label: format!("Track {n}"),
                language: None,
                codec: None,
                choice: Some(n),
            })
            .collect();
        assert_eq!(
            catalog.install(
                &live,
                live.clone(),
                "c-next".into(),
                Inventory::Complete {
                    tracks: rows,
                    subtitle_off: None
                }
            ),
            Err(Error::CatalogIncomplete)
        );
    }
    let mut bad = case("list-unobserved")["data"].clone();
    bad["tracks"][2]["id"] = bad["tracks"][1]["id"].clone();
    assert_eq!(
        catalog.install(&live, live.clone(), "c-next".into(), tracks(&bad)),
        Err(Error::CatalogIncomplete)
    );
    bad["tracks"][2]["id"] = json!("https://private.invalid/track");
    assert_eq!(
        catalog.install(&live, live.clone(), "c-next".into(), tracks(&bad)),
        Err(Error::CatalogIncomplete)
    );
    assert!(matches!(catalog.select(&from_case("select-audio-requested"), Some(&live), 110, 1000), Err(Error::CatalogIncomplete)),
        "failed owner publications retire the previous receipt rather than expose stale completeness");
    catalog
        .install(
            &live,
            live.clone(),
            "c-restored".into(),
            tracks(&case("list-empty")["data"]),
        )
        .unwrap();
    assert_eq!(
        catalog.install(
            &live,
            live.clone(),
            "c-restored".into(),
            tracks(&case("list-empty")["data"])
        ),
        Err(Error::CatalogChanged)
    );
}

#[test]
fn wrong_kind_unavailable_track_and_unsupported_off_never_map_to_another_choice() {
    let (mut catalog, live) = ready();
    for (id, kind, expected) in [
        ("a-example", "audio", Error::NotSelectable),
        ("a-other", "subtitle", Error::TrackNotFound),
        ("A-other", "audio", Error::TrackNotFound),
    ] {
        let mut params = case("select-audio-requested")["params"].clone();
        params["target"] = json!({"kind":kind,"mode":"track","id":id});
        assert!(
            matches!(catalog.select(&request(params), Some(&live), 110, 1000), Err(e) if e == expected)
        );
    }
    let mut empty = case("list-empty")["data"].clone();
    empty["catalog"] = json!("c-empty");
    catalog
        .install(&live, live.clone(), "c-empty".into(), tracks(&empty))
        .unwrap();
    catalog
        .list(&from_case("list-empty"), Some(&live), 120)
        .unwrap();
    let mut off = case("select-subtitle-off-requested")["params"].clone();
    off["catalog"] = json!("c-empty");
    assert!(matches!(
        catalog.select(&request(off), Some(&live), 121, 1000),
        Err(Error::NotSelectable)
    ));
}

#[test]
fn safe_labels_use_shared_redaction_and_do_not_serialize_native_objects() {
    for invalid in fixture()["invalid_results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| {
            v["name"]
                .as_str()
                .unwrap()
                .starts_with("list-label-private")
        })
    {
        let raw = invalid["data"]["tracks"][0]["label"].as_str().unwrap();
        assert_eq!(label(raw, 7), "Track 7", "{}", invalid["name"]);
    }
    assert_eq!(label("AC/DC", 1), "AC/DC");
    assert_eq!(label(&"é".repeat(64), 1), "é".repeat(64));
    assert_eq!(label(&"é".repeat(65), 1), "Track 1");
    assert_eq!(metadata(Some("../../secret".into()), true), None);
    assert_eq!(metadata(Some("日本語".into()), false), None);
    struct NativeOnly(&'static str); // Deliberately not Serialize.
    let live = context(1);
    let mut catalog = Catalog::default();
    catalog
        .install(
            &live,
            live.clone(),
            "c-example".into(),
            Inventory::Complete {
                tracks: vec![Track {
                    id: "a-other".into(),
                    kind: Kind::Audio,
                    label: "token=PRIVATE".into(),
                    language: None,
                    codec: None,
                    choice: Some(NativeOnly("PRIVATE-native-handle")),
                }],
                subtitle_off: None,
            },
        )
        .unwrap();
    let response = catalog
        .list(&from_case("list-empty"), Some(&live), 0)
        .unwrap()
        .into_envelope();
    assert_eq!(response["data"]["tracks"][0]["label"], "Track 1");
    assert!(!response.to_string().contains("PRIVATE"));
    let selected = catalog
        .select(&from_case("select-audio-requested"), Some(&live), 1, 100)
        .unwrap();
    assert_eq!(selected.choice().0, "PRIVATE-native-handle");
}

#[test]
fn stale_owner_completion_cannot_retire_a_newer_valid_catalog() {
    let (mut catalog, old) = ready();
    let live = context(2);
    catalog
        .install(
            &live,
            live.clone(),
            "c-new".into(),
            tracks(&case("list-unobserved")["data"]),
        )
        .unwrap();
    catalog
        .list(&from_case("list-unobserved"), Some(&live), 200)
        .unwrap();
    assert_eq!(
        catalog.install(&live, old, "c-stale".into(), Inventory::Incomplete),
        Err(Error::CatalogChanged)
    );
    let mut params = case("select-audio-requested")["params"].clone();
    params["catalog"] = json!("c-new");
    let selected = catalog
        .select(&request(params), Some(&live), 201, 1000)
        .unwrap();
    assert_eq!(*selected.choice(), 1);
    assert!(catalog.recheck(&selected, Some(&live), 202).is_ok());
}

#[test]
fn maximum_complete_inventory_is_delivered_without_truncation() {
    let live = context(1);
    let mut catalog = Catalog::default();
    let rows = (0..MAX_TRACKS)
        .map(|n| Track {
            id: format!("track-{n}"),
            kind: if n < MAX_TRACKS_PER_KIND {
                Kind::Audio
            } else {
                Kind::Subtitle
            },
            label: "é".repeat(64),
            language: Some("a".repeat(32)),
            codec: Some("c".repeat(32)),
            choice: Some(n),
        })
        .collect();
    catalog
        .install(
            &live,
            live.clone(),
            "c-full".into(),
            Inventory::Complete {
                tracks: rows,
                subtitle_off: Some(1000),
            },
        )
        .unwrap();
    let response = catalog
        .list(&from_case("list-empty"), Some(&live), 0)
        .unwrap();
    assert_eq!(
        response.data()["tracks"].as_array().unwrap().len(),
        MAX_TRACKS
    );
    assert!(serde_json::to_vec(response.data()).unwrap().len() <= MAX_RESULT_BYTES);
    assert!(matches!(
        bounded(json!({"large":"a".repeat(MAX_RESULT_BYTES)})),
        Err(Error::CatalogIncomplete)
    ));
}

// Expectations below are derived from the unchanged CLI media_tracks.label_valid
// contract, not a second implementation of its regular expression.
#[test]
fn wire_label_soft_hyphen_and_embedded_path_regressions() {
    assert_eq!(label("English\u{00ad}", 1), "Track 1");
    assert_eq!(label("Commentary /private/PRIVATE", 2), "Track 2");
}

#[test]
fn wire_label_unicode_categories_are_rejected_before_projection() {
    // UCD 17.0 DerivedGeneralCategory.txt: all Cc and Cf ranges. U+2065
    // is unassigned but was already conservatively refused by this producer.
    // Rust &str cannot contain the CLI's third excluded category (Cs).
    for (start, end) in [
        (0x0000, 0x001f),
        (0x007f, 0x009f),
        (0x00ad, 0x00ad),
        (0x0600, 0x0605),
        (0x061c, 0x061c),
        (0x06dd, 0x06dd),
        (0x070f, 0x070f),
        (0x0890, 0x0891),
        (0x08e2, 0x08e2),
        (0x180e, 0x180e),
        (0x200b, 0x200f),
        (0x202a, 0x202e),
        (0x2060, 0x206f),
        (0xfeff, 0xfeff),
        (0xfff9, 0xfffb),
        (0x110bd, 0x110bd),
        (0x110cd, 0x110cd),
        (0x13430, 0x1343f),
        (0x1bca0, 0x1bca3),
        (0x1d173, 0x1d17a),
        (0xe0001, 0xe0001),
        (0xe0020, 0xe007f),
    ] {
        for code in start..=end {
            let c = char::from_u32(code).unwrap();
            for raw in [
                format!("{c}English"),
                format!("Eng{c}lish"),
                format!("English{c}"),
            ] {
                assert_eq!(label(&raw, 7), "Track 7", "U+{code:04X}");
            }
        }
    }
}

#[test]
fn wire_label_private_syntax_corpus_preserves_no_secret_fragment() {
    for raw in [
        "English\u{00ad}",
        "Commentary /private/PRIVATE",
        "Commentary (/private/PRIVATE)",
        "Commentary \"/private/PRIVATE\"",
        "Commentary '/private/PRIVATE'",
        "Commentary ~/PRIVATE",
        "Commentary C:\\PRIVATE\\track",
        "Commentary (d:/PRIVATE/track)",
        "Commentary \\\\PRIVATE\\track",
        "Listen http:PRIVATE",
        "Listen HTTPS:PRIVATE",
        "Listen MAGNET:PRIVATE",
        "Listen data:PRIVATE",
        "Listen FILE:PRIVATE",
        "éhttp:PRIVATE",
        "日本語magnet:PRIVATE",
        "wss://PRIVATE.invalid/track",
        "token=PRIVATE",
        "token : PRIVATE",
        "api_key = PRIVATE",
        "api-key:PRIVATE",
        "apikey=PRIVATE",
        "Authorization=PRIVATE",
        "Commentary authorization = PRIVATE",
        "password:PRIVATE",
        "secret = PRIVATE",
        "étoken=PRIVATE",
        "Commentary (token=PRIVATE)",
        "Bearer PRIVATE",
        "API key:PRIVATE",
        "\u{e000}/private/PRIVATE",
        // The shared ASCII classification is " / ", then "/": keep its refusal.
        "日本語 / 音声",
    ] {
        assert_eq!(label(raw, 4), "Track 4", "raw label: {raw:?}");
    }
}

#[test]
fn wire_label_multilingual_and_nonprivate_syntax_are_preserved() {
    for raw in [
        "English",
        "Español — Comentarios",
        "Français",
        "日本語 音声",
        "한국어",
        "العربية",
        "עברית",
        "हिन्दी",
        "é",
        "🎵 Audio",
        "AC/DC",
        "Commentary / Music",
        "Part 1: The Beginning",
        "C: Original mix",
        "Myhttp: mix",
        "Token",
        "Secret Window",
        "Password",
        "API key documentary",
    ] {
        assert_eq!(label(raw, 3), raw, "raw label: {raw:?}");
    }
}

#[test]
fn wire_label_raw_bytes_are_checked_before_trim_or_truncation() {
    for raw in ["a".repeat(128), "é".repeat(64)] {
        assert_eq!(label(&raw, 1), raw);
    }
    for raw in [
        "a".repeat(129),
        "é".repeat(65),
        format!("{}English", " ".repeat(128)),
        format!("{} token=PRIVATE", "a".repeat(128)),
        format!("{}\u{00ad}", "a".repeat(127)),
    ] {
        assert_eq!(label(&raw, 1), "Track 1");
    }
    for raw in ["", "  ", "\u{00a0}\u{3000}", "\tEnglish", "English\n"] {
        assert_eq!(label(raw, 1), "Track 1");
    }
    assert_eq!(label("  日本語  ", 1), "日本語");
}

#[test]
fn wire_catalog_safe_labels_leave_native_choice_and_identity_unchanged() {
    let live = context(1);
    let mut catalog = Catalog::default();
    let raw = ["English\u{00ad}", "Commentary /private/PRIVATE", "日本語"];
    catalog
        .install(
            &live,
            live.clone(),
            "c-labels".into(),
            Inventory::Complete {
                tracks: raw
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| Track {
                        id: format!("audio-{index}"),
                        kind: Kind::Audio,
                        label: label.into(),
                        language: Some("en".into()),
                        codec: Some("aac".into()),
                        choice: Some(("PRIVATE-native-choice", index)),
                    })
                    .collect(),
                subtitle_off: None,
            },
        )
        .unwrap();
    let response = catalog
        .list(&from_case("list-empty"), Some(&live), 0)
        .unwrap()
        .into_envelope();
    let rows = response["data"]["tracks"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(response["data"]["catalog"], "c-labels");
    assert!(!response.to_string().contains("PRIVATE"));
    for (index, expected_label) in ["Track 1", "Track 2", "日本語"].into_iter().enumerate() {
        let id = format!("audio-{index}");
        assert_eq!(rows[index]["id"], id);
        assert_eq!(rows[index]["label"], expected_label);
        assert_eq!(rows[index]["selectable"], true);
        let request = request(
            json!({"op":"select", "runtime":"native-example", "playback":"p-example",
            "source_generation":"s-example", "catalog":"c-labels", "target":{"mode":"track","kind":"audio","id":id}}),
        );
        let selected = catalog.select(&request, Some(&live), 1, 100).unwrap();
        assert_eq!(*selected.choice(), ("PRIVATE-native-choice", index));
        assert_eq!(selected.requested_response().data()["target"]["id"], id);
    }
}

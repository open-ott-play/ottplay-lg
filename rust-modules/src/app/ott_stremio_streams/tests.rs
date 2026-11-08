use super::*;
use serde_json::json;

fn decode(rows: Value) -> Streams {
    parse(
        &serde_json::to_vec(&json!({"streams":rows})).unwrap(),
        "Open add-on",
    )
    .unwrap()
}
fn hash() -> String {
    "0123456789ABCDEF0123456789ABCDEF01234567".into()
}

#[test]
fn http_sources_are_untrusted_candidates_and_signed_urls_are_preserved() {
    let url = "https://media.example:443/a/../film.mp4?signature=AbC%2B123&x=%C3%A9";
    let decoded = decode(json!([{"url":url,"name":"HD","description":"First line\nSecond line"}]));
    assert_eq!(decoded.candidates.len(), 1);
    assert_eq!(decoded.candidates[0].title, "HD");
    assert_eq!(decoded.candidates[0].description, "First line\nSecond line");
    assert!(
        matches!(&decoded.candidates[0].source,SourceProposal::Http{url:value} if value == url)
    );
    assert!(!format!("{:?}", decoded.candidates[0].source).contains("signature"));
}

#[test]
fn url_syntax_reuses_native_validation_and_rejects_ambiguous_metadata() {
    for url in [
        "file:///etc/passwd",
        "https://user:pass@host/video",
        "https://host/video#part",
        "https://host/video|User-Agent=x",
        "https://host/a%0a",
        "https://host/a%5C",
        "https://host/a%FF",
        "https://host/a%",
        "https://host/a b",
        "//host/video",
        "https:///video",
    ] {
        let decoded = decode(json!([{"url":url}]));
        assert!(decoded.candidates.is_empty(), "{url}");
        assert_eq!(
            decoded.rejected[0].reason,
            Rejection::InvalidSource,
            "{url}"
        );
    }
}

#[test]
fn local_http_is_only_a_proposal_not_a_grant_to_a_control_endpoint() {
    let decoded = decode(json!([{"url":"http://127.0.0.1:8794/open.mp4"}]));
    assert_eq!(decoded.candidates.len(), 1);
    assert!(matches!(
        decoded.candidates[0].source,
        SourceProposal::Http { .. }
    ));
    // The existing runtime must still apply its reserved/owned endpoint policy.
}

#[test]
fn headers_drm_archives_and_external_targets_are_explicitly_unsupported() {
    for (field, feature) in [
        ("headers", Feature::Headers),
        ("proxyHeaders", Feature::Headers),
        ("requestHeaders", Feature::Headers),
        ("responseHeaders", Feature::Headers),
        ("drm", Feature::Drm),
        ("drmInfo", Feature::Drm),
        ("licenseUrl", Feature::Drm),
        ("licenseKey", Feature::Drm),
        ("licenseType", Feature::Drm),
        ("zipUrls", Feature::Archive),
        ("nzbUrl", Feature::Archive),
        ("rarUrls", Feature::Archive),
        ("7zipUrls", Feature::Archive),
        ("tgzUrls", Feature::Archive),
        ("tarUrls", Feature::Archive),
        ("externalUrl", Feature::ExternalPlayer),
        ("ytId", Feature::ExternalPlayer),
    ] {
        let mut row = json!({"url":"https://media.example/film"});
        row[field] = Value::Null;
        let decoded = decode(json!([row]));
        assert_eq!(
            decoded.rejected[0].reason,
            Rejection::Unsupported(feature),
            "{field}"
        );
        let mut row = json!({"url":"https://media.example/film","behaviorHints":{}});
        row["behaviorHints"][field] = json!("unsupported");
        assert_eq!(
            decode(json!([row])).rejected[0].reason,
            Rejection::Unsupported(feature)
        );
    }
}

#[test]
fn malformed_shape_and_behavior_hints_are_not_silently_ignored() {
    for row in [
        Value::Null,
        json!(true),
        json!([]),
        json!({"url":"https://host/a","behaviorHints":null}),
        json!({"url":"https://host/a","behaviorHints":[]}),
    ] {
        assert_eq!(
            decode(json!([row])).rejected[0].reason,
            Rejection::InvalidShape
        );
    }
}

#[test]
fn url_cannot_be_combined_with_torrent_specific_fields() {
    for field in ["infoHash", "fileIdx", "sources"] {
        let mut row = json!({"url":"https://media.example/film"});
        row[field] = Value::Null;
        assert_eq!(
            decode(json!([row])).rejected[0].reason,
            Rejection::InvalidSource
        );
    }
}

#[test]
fn torrent_hash_and_missing_file_selection_match_classic_contract() {
    let decoded = decode(json!([{"infoHash":hash()}]));
    let SourceProposal::Torrent(torrent) = &decoded.candidates[0].source else {
        panic!("wrong candidate")
    };
    assert_eq!(torrent.info_hash(), hash().to_ascii_lowercase());
    assert_eq!(torrent.file, FileChoice::Largest);
    assert!(torrent.trackers().is_empty());
    assert_eq!(
        torrent.magnet_candidate(),
        format!("magnet:?xt=urn:btih:{}", hash().to_ascii_lowercase())
    );
    for bad in [
        "",
        "abcdef",
        "urn:btih:0123",
        "012345678901234567890123456789012345678g",
    ] {
        assert_eq!(
            decode(json!([{"infoHash":bad}])).rejected[0].reason,
            Rejection::InvalidSource
        );
    }
}

#[test]
fn file_index_is_exact_bounded_and_never_clamped_or_silently_changed() {
    for value in [json!(0), json!(1.0), json!(2147483647_u64)] {
        let decoded = decode(json!([{"infoHash":hash(),"fileIdx":value}]));
        let SourceProposal::Torrent(torrent) = &decoded.candidates[0].source else {
            panic!("wrong candidate")
        };
        assert_eq!(
            torrent.file,
            FileChoice::Index(value.as_f64().unwrap() as u32)
        );
    }
    for value in [
        json!(-1),
        json!(0.5),
        json!(2147483648_u64),
        Value::Null,
        json!("1"),
    ] {
        assert_eq!(
            decode(json!([{"infoHash":hash(),"fileIdx":value}])).rejected[0].reason,
            Rejection::InvalidFileIndex
        );
    }
}

#[test]
fn trackers_are_validated_deduped_sorted_and_preserved_in_magnet() {
    let decoded = decode(
        json!([{"infoHash":hash(),"sources":["tracker:udp://b.example:80/announce","tracker:https://a.example/a?token=x",format!("dht:{}",hash()),"tracker:udp://b.example:80/announce"]}]),
    );
    let SourceProposal::Torrent(torrent) = &decoded.candidates[0].source else {
        panic!("wrong candidate")
    };
    assert_eq!(
        torrent.trackers(),
        ["https://a.example/a?token=x", "udp://b.example:80/announce"]
    );
    assert!(torrent
        .magnet_candidate()
        .contains("&tr=https%3A%2F%2Fa.example%2Fa%3Ftoken%3Dx"));
}

#[test]
fn unsupported_or_malformed_trackers_reject_the_whole_candidate() {
    for source in [
        "https://a.example/announce",
        "tracker:file:///a",
        "tracker:udp://user:pass@host/announce",
        "dht:abcd",
        "tracker:https://a/a%0D",
    ] {
        assert_eq!(
            decode(json!([{"infoHash":hash(),"sources":[source]}])).rejected[0].reason,
            Rejection::InvalidTrackers
        );
    }
    assert_eq!(
        decode(json!([{"infoHash":hash(),"sources":[]}]))
            .candidates
            .len(),
        1
    );
    assert_eq!(
        decode(json!([{"infoHash":hash(),"sources":vec!["tracker:https://a.example/a";33]}]))
            .rejected[0]
            .reason,
        Rejection::InvalidTrackers
    );
}

#[test]
fn generated_magnet_size_is_bounded() {
    let sources = (0..32)
        .map(|index| {
            format!(
                "tracker:https://tracker{index}.example/{}",
                "x".repeat(1900)
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        decode(json!([{"infoHash":hash(),"sources":sources}])).rejected[0].reason,
        Rejection::SourceLimit
    );
}

#[test]
fn source_dedup_keeps_first_metadata_and_distinct_file_choices() {
    let decoded = decode(json!([
        {"url":"https://media.example/a","name":"First"},
        {"url":"https://media.example/a","name":"Second"},
        {"infoHash":hash(),"fileIdx":0},
        {"infoHash":hash().to_lowercase(),"fileIdx":0},
        {"infoHash":hash(),"fileIdx":1}
    ]));
    assert_eq!(decoded.candidates.len(), 3);
    assert_eq!(decoded.duplicate_count, 2);
    assert_eq!(decoded.candidates[0].title, "First");
}

#[test]
fn empty_unsupported_and_invalid_response_are_distinct() {
    let empty = decode(json!([]));
    assert!(empty.candidates.is_empty() && empty.rejected.is_empty());
    let unsupported = decode(json!([{"externalUrl":"https://example/a"}]));
    assert!(unsupported.candidates.is_empty());
    assert_eq!(unsupported.rejected.len(), 1);
    assert_eq!(parse(b"{}", "Test").err(), Some(Error::InvalidResponse));
    assert_eq!(
        parse(b"not JSON", "Test").err(),
        Some(Error::InvalidResponse)
    );
}

#[test]
fn response_limits_and_rejected_row_positions_are_preserved() {
    assert_eq!(
        parse(&vec![b' '; 524289], "Test").err(),
        Some(Error::ResponseLimit)
    );
    assert_eq!(
        parse(
            &serde_json::to_vec(&json!({"streams":vec![json!({});257]})).unwrap(),
            "Test"
        )
        .err(),
        Some(Error::ResponseLimit)
    );
    let decoded = decode(json!([{"url":"https://media.example/a"},null,{}, {"infoHash":hash()}]));
    assert_eq!(
        decoded
            .rejected
            .iter()
            .map(|row| row.position)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn blank_titles_use_addon_fallback_and_unknown_optional_hints_do_not_break_playable_shapes() {
    let decoded = decode(
        json!([{"url":"https://media.example/a","name":"","title":"","behaviorHints":{"bingeGroup":"group"}}]),
    );
    assert_eq!(decoded.candidates[0].title, "Open add-on");
}

#[test]
fn diagnostics_never_echo_arbitrary_addon_titles_or_descriptions() {
    let decoded = decode(
        json!([{"url":"https://media.example/film?token=secret","name":"private-title","description":"private-description"}]),
    );
    let debug = format!("{decoded:?}");
    assert!(!debug.contains("secret"));
    assert!(!debug.contains("private-title"));
    assert!(!debug.contains("private-description"));
}

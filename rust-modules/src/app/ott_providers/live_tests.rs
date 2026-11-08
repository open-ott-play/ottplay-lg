use super::super::Response;
use super::*;
use std::collections::VecDeque;
struct Http {
    responses: VecDeque<Result<Response, &'static str>>,
    requests: usize,
    cancel_at: Option<usize>,
}
impl ProviderHttp for Http {
    fn request(&mut self, _: &Request) -> Result<Response, &'static str> {
        self.requests += 1;
        self.responses.pop_front().expect("unexpected request")
    }
    fn cancelled(&self) -> bool {
        self.cancel_at == Some(self.requests)
    }
}
fn response(s: &str) -> Result<Response, &'static str> {
    Ok(Response {
        status: 200,
        body: s.as_bytes().to_vec(),
    })
}
fn fixture() -> (ProviderProfile, CatalogEntry, GuideRequest) {
    let p = ProviderProfile {
        id: "home".into(),
        name: "Home".into(),
        kind: ott::ProviderKind::M3u,
        source: "https://guide.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let mut catalog = ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-name=\"World News\",World News\nhttps://video.invalid/live",
        &p,
    )
    .unwrap();
    let e = catalog.entries.remove(0);
    let request = GuideRequest {
        context: ott::guide::Context {
            generation: 7,
            profile_id: p.id.clone(),
            entry_id: e.id.clone(),
        },
        query: ott::guide::Query::CurrentNext { now: 1767227400 },
    };
    (p, e, request)
}
const HEADER: &str =
    "#EXTM3U url-tvg=\"https://guide.invalid/first.xml,https://guide.invalid/second.xml\"\n";
const XML: &str = r#"<tv><channel id="world"><display-name>World News</display-name></channel><programme channel="world" start="20260101000000 +0000" stop="20260101010000 +0000"><title>First</title></programme></tv>"#;
#[test]
fn failed_first_source_falls_back_and_matches_name_without_tvg_id() {
    let (p, e, r) = fixture();
    let mut h = Http {
        responses: vec![response(HEADER), Err("fixture failure"), response(XML)].into(),
        requests: 0,
        cancel_at: None,
    };
    let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert_eq!(h.requests, 3);
    assert_eq!(result.programmes[0].title, "First");
    assert_eq!(result.context, r.context);
}
#[test]
fn empty_first_source_does_not_hide_second_source() {
    let (p, e, r) = fixture();
    let mut h = Http {
        responses: vec![response(HEADER), response("<tv/>"), response(XML)].into(),
        requests: 0,
        cancel_at: None,
    };
    assert_eq!(
        super::super::guide_with(&p, &e, &r, &mut h)
            .unwrap()
            .programmes
            .len(),
        1
    );
}
#[test]
fn cancelled_transport_and_changed_context_cannot_publish() {
    let (p, e, r) = fixture();
    let mut h = Http {
        responses: vec![response(HEADER), response(XML)].into(),
        requests: 0,
        cancel_at: Some(2),
    };
    assert_eq!(
        super::super::guide_with(&p, &e, &r, &mut h).unwrap_err(),
        "Provider request cancelled"
    );
    let mut changed = r.clone();
    changed.context.profile_id = "other".into();
    let mut h = Http {
        responses: VecDeque::new(),
        requests: 0,
        cancel_at: None,
    };
    assert!(super::super::guide_with(&p, &e, &changed, &mut h).is_err());
    assert_eq!(h.requests, 0);
}

#[test]
fn invalid_shifted_time_in_first_source_does_not_abort_second_source() {
    let (p, mut e, r) = fixture();
    e.live.as_mut().unwrap().guide_shift_seconds = -3600;
    let first = r#"<tv><channel id="world"><display-name>World News</display-name></channel><programme channel="world" start="19700101000000 +0000" stop="19700101010000 +0000"><title>Invalid after shift</title></programme></tv>"#;
    let second = r#"<tv><channel id="world"><display-name>World News</display-name></channel><programme channel="world" start="20260101010000 +0000" stop="20260101020000 +0000"><title>Second source</title></programme></tv>"#;
    let mut h = Http {
        responses: vec![response(HEADER), response(first), response(second)].into(),
        requests: 0,
        cancel_at: None,
    };
    let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert_eq!(h.requests, 3);
    assert_eq!(result.programmes[0].title, "Second source");
    assert_eq!(result.programmes[0].start, 1767225600);
}

#[test]
fn playlist_without_guide_is_valid_empty_metadata() {
    let (p, e, r) = fixture();
    let mut h = Http {
        responses: vec![response("#EXTM3U\n")].into(),
        requests: 0,
        cancel_at: None,
    };
    let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert!(result.programmes.is_empty());
    assert_eq!(h.requests, 1);
}

fn with_unrelated_bad_programme(xml: &str, stop: &str) -> String {
    xml.replace("</tv>", &format!("<programme channel='unrelated' start='20260101000000 +0000' {stop}><title>Unrelated</title></programme></tv>"))
}

#[test]
fn exact_id_ignores_unrelated_bad_times_without_channel_declaration() {
    let (p, mut e, r) = fixture();
    e.provider_id = Some("world".into());
    e.title = "Another display label".into();
    e.live.as_mut().unwrap().tvg_name = String::new();
    let programme_only = XML.replace(
        "<channel id=\"world\"><display-name>World News</display-name></channel>",
        "",
    );
    for stop in ["", "stop='invalid'"] {
        let xml = with_unrelated_bad_programme(&programme_only, stop);
        let mut h = Http {
            responses: vec![response(HEADER), response(&xml)].into(),
            requests: 0,
            cancel_at: None,
        };
        let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
        assert_eq!(result.programmes[0].title, "First");
        assert_eq!(
            h.requests, 2,
            "unrelated bad content must not force a fallback request"
        );
    }
}

#[test]
fn name_fallback_ignores_unrelated_bad_programme_times() {
    let (p, e, r) = fixture();
    assert!(e.provider_id.is_none());
    for stop in ["", "stop='invalid'"] {
        let xml = with_unrelated_bad_programme(XML, stop);
        let mut h = Http {
            responses: vec![response(HEADER), response(&xml)].into(),
            requests: 0,
            cancel_at: None,
        };
        let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
        assert_eq!(result.programmes[0].title, "First");
        assert_eq!(h.requests, 2);
    }
}

#[test]
fn malformed_selected_programme_remains_strict_and_uses_next_source() {
    let (p, e, r) = fixture();
    let broken = XML.replace("stop=\"20260101010000 +0000\"", "stop=\"invalid\"");
    let mut h = Http {
        responses: vec![response(HEADER), response(&broken), response(XML)].into(),
        requests: 0,
        cancel_at: None,
    };
    let result = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert_eq!(result.programmes[0].title, "First");
    assert_eq!(h.requests, 3);
    let mut h = Http {
        responses: vec![
            response("#EXTM3U url-tvg='https://guide.invalid/only.xml'\n"),
            response(&broken),
        ]
        .into(),
        requests: 0,
        cancel_at: None,
    };
    assert!(super::super::guide_with(&p, &e, &r, &mut h).is_err());
    assert_eq!(h.requests, 2);
}

fn records_fixture() -> (ProviderProfile, CatalogEntry, GuideRequest) {
    let (p, mut e, mut r) = fixture();
    e.live.as_mut().unwrap().catchup = Some(ott::archive::CatchupSource {
        mode: ott::archive::CatchupMode::Shift, source: String::new(),
        retention_seconds: 7 * 86400, correction_seconds: 0,
    });
    let now = ott::epg::timestamp("20260108000000 +0000").unwrap();
    r.query = ott::records::query(&p, &e, 0, now).unwrap();
    (p, e, r)
}

#[test]
fn records_guide_reads_full_window_and_all_distinct_feeds_once() {
    let (p, e, r) = records_fixture();
    let header = "#EXTM3U url-tvg='https://guide.invalid/recent.xml,https://guide.invalid/old.xml,https://guide.invalid/recent.xml'\n";
    let recent = XML.replace("20260101", "20260107").replace("First", "Recent");
    let old = XML.replace("20260101", "20260102").replace("First", "Older than one day");
    let mut h = Http { responses: vec![response(header), response(&recent), response(&old)].into(),
        requests: 0, cancel_at: None };
    let value = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert_eq!(h.requests, 3, "no day loop or repeated URL fetch");
    assert_eq!(value.context, r.context);
    assert_eq!(value.query, r.query);
    assert_eq!(value.programmes.len(), 2, "a nonempty recent feed must not hide historical records");
    assert_eq!(value.programmes[1].title, "Older than one day");
    assert_eq!(ott::records::indices(&value).unwrap(), vec![1, 0]);
}

#[test]
fn records_guide_mirrored_feeds_keep_raw_identity_and_one_visible_title() {
    let (p, e, r) = records_fixture();
    let valid = XML.replace("20260101", "20260102");
    let mut h = Http { responses: vec![response(HEADER), response(&valid), response(&valid)].into(),
        requests: 0, cancel_at: None };
    let value = super::super::guide_with(&p, &e, &r, &mut h).unwrap();
    assert_eq!(value.programmes.len(), 2);
    assert_eq!(value.programmes[0], value.programmes[1]);
    assert_eq!(ott::records::indices(&value).unwrap(), vec![1]);
    assert_eq!(h.requests, 3);
}

#[test]
fn records_guide_distinguishes_unavailable_from_matched_empty() {
    let (p, e, r) = records_fixture();
    let header = "#EXTM3U url-tvg='https://guide.invalid/only.xml'\n";
    let empty = "<tv><channel id='world'><display-name>World News</display-name></channel></tv>";
    let mut h = Http { responses: vec![response(header), response(empty)].into(), requests: 0, cancel_at: None };
    assert!(super::super::guide_with(&p, &e, &r, &mut h).unwrap().programmes.is_empty());
    for replies in [vec![response("#EXTM3U\n")], vec![response(header), response("<tv/>")]] {
        let mut h = Http { responses: replies.into(), requests: 0, cancel_at: None };
        assert!(super::super::guide_with(&p, &e, &r, &mut h).is_err());
    }
}

#[test]
fn records_guide_failure_never_publishes_partial_results() {
    let (p, e, r) = records_fixture();
    let valid = XML.replace("20260101", "20260102");
    for failed in [Err("fixture timeout"), response("<tv><channel id='world'>"),
        Ok(Response { status: 503, body: Vec::new() })] {
        let mut h = Http { responses: vec![response(HEADER), response(&valid), failed].into(), requests: 0, cancel_at: None };
        assert!(super::super::guide_with(&p, &e, &r, &mut h).is_err());
        assert_eq!(h.requests, 3);
    }
}

#[test]
fn records_guide_rejects_unsupported_before_io() {
    let (p, mut e, r) = records_fixture();
    let mut h = Http { responses: VecDeque::new(), requests: 0, cancel_at: None };
    let mut other = p.clone();
    other.kind = ott::ProviderKind::Xtream;
    other.source = r#"{"server":"https://portal.invalid","username":"test","password":"test"}"#.into();
    let mut other_entry = e.clone();
    other_entry.media = ott::MediaLocator::Xtream { stream_id: "42".into(), stream_kind: "live".into(),
        extension: "ts".into(), series_id: None, has_direct_source: false };
    other.validate().unwrap();
    assert!(super::super::guide_with(&other, &other_entry, &r, &mut h).is_err());
    e.live.as_mut().unwrap().catchup = None;
    assert!(super::super::guide_with(&p, &e, &r, &mut h).is_err());
    assert_eq!(h.requests, 0);
}

#[test]
fn records_guide_bounds_and_cancellation() {
    let (p, e, r) = records_fixture();
    let urls = (0..=MAX_GUIDE_SOURCES).map(|n| format!("https://guide.invalid/{n}.xml"))
        .collect::<Vec<_>>().join(",");
    let header = format!("#EXTM3U url-tvg='{urls}'\n");
    let mut h = Http { responses: vec![response(&header)].into(), requests: 0, cancel_at: None };
    assert!(super::super::guide_with(&p, &e, &r, &mut h).is_err());
    assert_eq!(h.requests, 1, "source count is refused before any XMLTV request");
    let valid = XML.replace("20260101", "20260102");
    let mut h = Http { responses: vec![response(HEADER), response(&valid)].into(), requests: 0, cancel_at: Some(2) };
    assert_eq!(super::super::guide_with(&p, &e, &r, &mut h).unwrap_err(), "Provider request cancelled");
    assert_eq!(h.requests, 2);
}

#[test]
fn records_guide_aggregate_bytes_fail_atomically() {
    let (p, e, r) = records_fixture();
    let valid = XML.replace("20260101", "20260102");
    let padded = valid.replace("</tv>", &format!("<!--{}--></tv>", "x".repeat(ott::MAX_CATALOG_BYTES / 2)));
    let mut h = Http { responses: vec![response(HEADER), response(&padded), response(&padded)].into(), requests: 0, cancel_at: None };
    assert_eq!(super::super::guide_with(&p, &e, &r, &mut h).unwrap_err(),
        "Records XMLTV sources exceed the aggregate byte limit");
    assert_eq!(h.requests, 3);
}

#[path = "live_rows_tests.rs"]
mod row_tests;

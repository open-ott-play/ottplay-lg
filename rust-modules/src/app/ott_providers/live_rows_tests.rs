use super::*;
use super::super::rows::m3u_guide_rows;
use plx_data::ott::guide::rows::{Selector, CoverageReason, CoverageState};

fn selectors() -> Vec<Selector> {
    ["world", "second", "third", "fourth"].iter().map(|id| Selector {
        entry_id: (*id).into(), provider_id: Some((*id).into()),
        tvg_name: String::new(), title: (*id).into(), shift_seconds: 0,
    }).collect()
}
fn programme(id: &str, title: &str, start: &str, end: &str) -> String {
    format!("<programme channel='{id}' start='20260101{start} +0000' stop='20260101{end} +0000'><title>{title}</title></programme>")
}
#[test]
fn rows_share_sources_and_preserve_next_only_precedence() {
    let (profile, entry, request) = fixture();
    let first = format!("<tv><channel id='world'><display-name>World News</display-name></channel>{}{}{}</tv>",
        programme("world", "First", "000000", "010000"),
        programme("second", "Later", "020000", "030000"),
        programme("third", "Early", "000000", "004500"));
    let second = format!("<tv>{}</tv>", programme("second", "Must not override next-only", "000000", "010000"));
    let mut http = Http { responses: vec![response(HEADER), response(&first), response(&second)].into(), requests: 0, cancel_at: None };
    let (selected, snapshot) = m3u_guide_rows(&profile, &entry, &request, &selectors(), &mut http);
    assert_eq!(selected.unwrap().programmes[0].title, "First");
    assert_eq!(http.requests, 3, "one header and each source once, not per row");
    let snapshot = snapshot.snapshot.unwrap();
    assert_eq!(snapshot.programmes["world"].title, "First");
    assert_eq!(snapshot.programmes["third"].title, "Early");
    assert!(!snapshot.programmes.contains_key("second"));
    assert_eq!(snapshot.next_transition_s, ott::epg::timestamp("20260101004500 +0000"));
}
#[test]
fn rows_skip_unrelated_bad_times_and_use_later_source_for_missing_channel() {
    let (profile, entry, request) = fixture();
    let first = XML.replace("</tv>", "<programme channel='second' start='bad' stop='bad'/></tv>");
    let second = format!("<tv>{}</tv>", programme("second", "Fallback", "000000", "010000"));
    let mut http = Http { responses: vec![response(HEADER), response(&first), response(&second)].into(), requests: 0, cancel_at: None };
    let (selected, snapshot) = m3u_guide_rows(&profile, &entry, &request, &selectors(), &mut http);
    assert!(selected.is_ok());
    assert_eq!(snapshot.snapshot.unwrap().programmes["second"].title, "Fallback");
}
#[test]
fn rows_cancel_after_response_publishes_neither_result() {
    let (profile, entry, request) = fixture();
    let mut http = Http { responses: vec![response(HEADER), response(XML)].into(), requests: 0, cancel_at: Some(2) };
    let (selected, snapshot) = m3u_guide_rows(&profile, &entry, &request, &selectors(), &mut http);
    assert!(selected.is_err());
    assert!(snapshot.snapshot.is_none());
    assert_eq!(http.requests, 2);
}
#[test]
fn rows_accept_two_valid_documents_larger_than_previous_aggregate_limit() {
    let (profile, entry, request) = fixture();
    let padding = "x".repeat(ott::MAX_CATALOG_BYTES / 2 + 64);
    let first = XML.replace("</tv>", &format!("<!--{padding}--></tv>"));
    let second = format!("<tv>{}<!--{padding}--></tv>", programme("second", "Second", "000000", "010000"));
    assert!(first.len() + second.len() > ott::MAX_CATALOG_BYTES);
    assert!(first.len() < ott::MAX_CATALOG_BYTES && second.len() < ott::MAX_CATALOG_BYTES);
    let mut http = Http { responses: vec![response(HEADER), response(&first), response(&second)].into(), requests: 0, cancel_at: None };
    let (selected, outcome) = m3u_guide_rows(&profile, &entry, &request, &selectors()[..2], &mut http);
    assert!(selected.is_ok());
    assert_eq!(outcome.report.state, CoverageState::Complete);
    let snapshot = outcome.snapshot.unwrap();
    assert_eq!(snapshot.programmes["world"].title, "First");
    assert_eq!(snapshot.programmes["second"].title, "Second");
}
#[test]
fn rows_source_limit_reports_partial_prefix_and_keeps_selected() {
    let (profile, entry, request) = fixture();
    let urls = (0..=MAX_GUIDE_SOURCES).map(|i| format!("https://guide.invalid/{i}.xml")).collect::<Vec<_>>().join(",");
    let header = format!("#EXTM3U url-tvg=\"{urls}\"\n");
    let responses = std::iter::once(response(&header)).chain((0..MAX_GUIDE_SOURCES).map(|_| response(XML))).collect();
    let mut http = Http { responses, requests: 0, cancel_at: None };
    let (selected, outcome) = m3u_guide_rows(&profile, &entry, &request, &selectors(), &mut http);
    assert!(selected.is_ok());
    assert_eq!(outcome.report.state, CoverageState::Partial);
    assert!(outcome.report.reasons.contains(&CoverageReason::SourceLimit));
    assert_eq!(outcome.report.attempted_sources, MAX_GUIDE_SOURCES);
    assert_eq!(outcome.snapshot.unwrap().programmes["world"].title, "First");
    assert_eq!(http.requests, MAX_GUIDE_SOURCES + 1);
}

#[test]
fn rows_stop_when_all_selectors_and_selected_guide_are_satisfied() {
    let (profile, entry, request) = fixture();
    let mut http = Http { responses: vec![response(HEADER), response(XML)].into(), requests: 0, cancel_at: None };
    let (selected, snapshot) = m3u_guide_rows(&profile, &entry, &request, &selectors()[..1], &mut http);
    assert!(selected.is_ok());
    assert_eq!(snapshot.snapshot.unwrap().programmes["world"].title, "First");
    assert_eq!(http.requests, 2, "a later source cannot override already useful facts");
}

#[test]
fn rows_output_budget_keeps_committed_prefix_and_exposes_reason() {
    let (profile, entry, request) = fixture();
    let title = "T".repeat(ott::MAX_METADATA_BYTES / 2);
    let xml = XML.replace("First", &title);
    let all: Vec<_> = (0..600).map(|i| Selector { entry_id: format!("row-{i}"), ..selectors()[0].clone() }).collect();
    let mut http = Http { responses: vec![response(HEADER), response(&xml)].into(), requests: 0, cancel_at: None };
    let (selected, outcome) = m3u_guide_rows(&profile, &entry, &request, &all, &mut http);
    assert!(selected.is_ok());
    assert!(outcome.report.reasons.contains(&CoverageReason::OutputLimit));
    assert_eq!(outcome.report.state, CoverageState::Partial);
    assert!(outcome.report.matched > 0 && outcome.report.matched < all.len());
    let snapshot = outcome.snapshot.unwrap();
    assert_eq!(outcome.report.current, snapshot.programmes.len());
    assert_eq!(outcome.report.matched, snapshot.programmes.len());
    assert!(snapshot.next_transition_s.is_some());
    assert_eq!(http.requests, 2);
}

struct DeadlineHttp { inner: Http, expire_at: usize, selected_ready: bool }
impl ProviderHttp for DeadlineHttp {
    fn request(&mut self, request: &Request) -> Result<super::super::super::Response, &'static str> { self.inner.request(request) }
    fn cancelled(&self) -> bool { self.inner.cancelled() }
    fn row_guide_expired(&self) -> bool { self.inner.requests >= self.expire_at }
    fn row_guide_selected(&mut self) { self.selected_ready = true; }
}
#[test]
fn rows_deadline_keeps_prefix_and_does_not_stop_selected_fallback() {
    let (profile, entry, request) = fixture();
    for selected_first in [false, true] {
        let first = if selected_first { XML.to_owned() } else { format!("<tv>{}</tv>", programme("second", "Other", "000000", "010000")) };
        let mut http = DeadlineHttp { inner: Http { responses: vec![response(HEADER), response(&first), response(XML)].into(), requests: 0, cancel_at: None }, expire_at: if selected_first { 3 } else { 2 }, selected_ready: false };
        let (selected, outcome) = m3u_guide_rows(&profile, &entry, &request, &selectors(), &mut http);
        assert_eq!(selected.unwrap().programmes[0].title, "First");
        assert!(http.selected_ready);
        assert!(outcome.report.reasons.contains(&CoverageReason::Deadline));
        assert_eq!(outcome.report.state, if selected_first { CoverageState::Partial } else { CoverageState::Unavailable });
        assert_eq!(outcome.snapshot.is_some(), selected_first);
    }
}

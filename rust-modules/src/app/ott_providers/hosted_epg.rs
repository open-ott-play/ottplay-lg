//! Hosted guide JSON v1. The metadata worker owns source/config/generation admission.
use super::{ott, CatalogEntry, ProviderHttp, ProviderProfile, Request};
use ott::{
    guide::{self, rows},
    hosted_epg::{self as wire, Config, Envelope, Mapping},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
const RESPONSE: usize = 8 * 1024 * 1024;
fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
fn selector(entry: &CatalogEntry) -> rows::Selector {
    rows::Selector {
        entry_id: entry.id.clone(),
        // Native M3U parsing attests tvg-id here; its legacy field marker is None.
        provider_id: entry.provider_id.clone(),
        tvg_name: entry
            .live
            .as_ref()
            .map_or(String::new(), |l| l.tvg_name.clone()),
        title: entry.title.clone(),
        shift_seconds: entry.live.as_ref().map_or(0, |l| l.guide_shift_seconds),
    }
}
fn channel(row: &rows::Selector, current: bool) -> Result<Value, &'static str> {
    for field in [
        &row.entry_id,
        row.provider_id.as_deref().unwrap_or(""),
        &row.tvg_name,
        &row.title,
    ] {
        if field.encode_utf16().count() > 512 || field.chars().any(char::is_control) {
            return Err("Invalid hosted guide channel");
        }
    }
    if row.entry_id.is_empty() || !(-86400..=86400).contains(&row.shift_seconds) {
        return Err("Invalid hosted guide channel");
    }
    let mut value = json!({"id":row.entry_id,"tvgId":row.provider_id.as_deref().unwrap_or(""),"tvgName":row.tvg_name,"name":row.title});
    if current {
        value["shift"] = json!(row.shift_seconds);
    }
    Ok(value)
}
fn fetch(
    config: &Config,
    path: &str,
    body: Option<Value>,
    http: &mut dyn ProviderHttp,
) -> Result<Value, &'static str> {
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let body = body
        .map(|v| serde_json::to_vec(&v).map_err(|_| "Invalid hosted guide request"))
        .transpose()?;
    if body.as_ref().is_some_and(|b| b.len() > 512 * 1024) {
        return Err("Hosted guide request exceeds limit");
    }
    let response = http.request(&Request {
        url: format!("{}{path}", config.url),
        headers: if body.is_some() {
            vec!["Content-Type: application/json".into()]
        } else {
            Vec::new()
        },
        body,
    })?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let limit = if path == "/current" {
        2 * 1024 * 1024
    } else {
        RESPONSE
    };
    if !(200..300).contains(&response.status) || response.body.len() > limit {
        return Err("Hosted guide request failed or exceeds limit");
    }
    serde_json::from_slice(&response.body).map_err(|_| "Invalid hosted guide response")
}
fn validate(
    config: &Config,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &guide::Request,
) -> Result<(), &'static str> {
    if Config::parse(
        &serde_json::to_value(config).map_err(|_| "Invalid hosted guide configuration")?,
    )
    .map_err(|_| "Invalid hosted guide configuration")?
        != *config
    {
        return Err("Invalid hosted guide configuration");
    }
    if profile.kind != ott::ProviderKind::M3u
        || profile.id != request.context.profile_id
        || entry.id != request.context.entry_id
        || entry.live.is_none()
        || entry.duration_ms.is_some()
        || !matches!(entry.media, ott::MediaLocator::Http { .. })
    {
        return Err("Hosted guide source changed");
    }
    request
        .query
        .validate()
        .map_err(|_| "Invalid hosted guide query")?;
    Ok(())
}
fn matched(
    config: &Config,
    selectors: &[rows::Selector],
    http: &mut dyn ProviderHttp,
) -> Result<(Envelope, BTreeMap<String, Mapping>), &'static str> {
    if selectors.is_empty() || selectors.len() > 2048 {
        return Err("Hosted guide channel limit");
    }
    let ids: BTreeSet<_> = selectors.iter().map(|r| r.entry_id.clone()).collect();
    if ids.len() != selectors.len() {
        return Err("Invalid hosted guide channel identities");
    }
    let channels = selectors
        .iter()
        .map(|r| channel(r, false))
        .collect::<Result<Vec<_>, _>>()?;
    let data = fetch(
        config,
        "/match",
        Some(json!({"version":1,"source":config.source,"channels":channels})),
        http,
    )?;
    wire::mappings(&data, &ids).map_err(|_| "Invalid or stale hosted guide mapping")
}
fn selected(
    config: &Config,
    entry: &CatalogEntry,
    request: &guide::Request,
    meta: &Envelope,
    map: &BTreeMap<String, Mapping>,
    http: &mut dyn ProviderHttp,
) -> Result<guide::ResultSet, &'static str> {
    let Some(mapping) = map.get(&entry.id) else {
        return Ok(guide::ResultSet {
            context: request.context.clone(),
            query: request.query,
            coverage: guide::Coverage::ProviderShortGuide,
            programmes: Vec::new(),
        });
    };
    let adjustment = entry.live.as_ref().map_or(0, |l| l.guide_shift_seconds);
    let newest = meta.fetched_ms / 1000 + meta.refresh_ms / 1000 + 1;
    let start = match request.query {
        guide::Query::CurrentNext { now } => now,
        guide::Query::Day { start, .. } | guide::Query::Records { start, .. } => start,
    };
    let first = start
        .checked_sub(adjustment)
        .ok_or("Invalid hosted guide interval")?;
    let hours = ((newest.saturating_sub(first).max(0) + 3599) / 3600)
        .max(48)
        .min(8784);
    let covered = first >= newest - hours * 3600
        && match request.query {
            guide::Query::CurrentNext { .. } => false,
            guide::Query::Day { end, .. } | guide::Query::Records { end, .. } => end
                .checked_sub(adjustment)
                .is_some_and(|end| end <= meta.fetched_ms / 1000 + 48 * 3600),
        };
    http.row_guide_selected();
    let data = fetch(
        config,
        &format!(
            "/programmes?channelId={}&shift={}&hours={hours}&generation={}",
            encode(&mapping.channel),
            mapping.shift,
            encode(&meta.generation)
        ),
        None,
        http,
    )?;
    let programmes = wire::programmes(&data, meta, &mapping.channel)
        .map_err(|_| "Invalid or stale hosted guide programmes")?;
    let programmes = guide::select(&programmes, &mapping.channel, request.query, adjustment)
        .map_err(|_| "Invalid hosted guide interval")?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    Ok(guide::ResultSet {
        context: request.context.clone(),
        query: request.query,
        coverage: if covered {
            guide::Coverage::RequestedWindow
        } else {
            guide::Coverage::ProviderShortGuide
        },
        programmes,
    })
}
pub(super) fn guide(
    config: &Config,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<guide::ResultSet, &'static str> {
    validate(config, profile, entry, request)?;
    let (meta, map) = matched(config, &[selector(entry)], http)?;
    selected(config, entry, request, &meta, &map, http)
}
pub(super) fn rows(
    config: &Config,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &guide::Request,
    selectors: &[rows::Selector],
    http: &mut dyn ProviderHttp,
) -> Result<(guide::ResultSet, rows::Outcome), &'static str> {
    validate(config, profile, entry, request)?;
    let guide::Query::CurrentNext { now } = request.query else {
        return Err("Invalid hosted current query");
    };
    let actual = selector(entry);
    // Selected guide always has admission even when the catalogue exceeds the
    // service's channel/body caps. Unread rows are explicitly partial, not empty.
    let mut inputs = vec![actual.clone()];
    let mut bytes = serde_json::to_vec(&channel(&actual, true)?)
        .map_err(|_| "Invalid hosted guide channel")?
        .len()
        + 1024;
    let mut seen = BTreeSet::new();
    let mut limited = false;
    for row in selectors {
        if !seen.insert(&row.entry_id) {
            return Err("Invalid hosted guide channel identities");
        }
        if row.entry_id == entry.id {
            if row != &actual {
                return Err("Hosted guide source changed");
            }
            continue;
        }
        if inputs.len() >= 2048 {
            limited = true;
            continue;
        }
        let size = serde_json::to_vec(&channel(row, true)?)
            .map_err(|_| "Invalid hosted guide channel")?
            .len()
            + 1;
        if bytes.saturating_add(size) > 512 * 1024 {
            limited = true;
            continue;
        }
        bytes += size;
        inputs.push(row.clone());
    }
    let (meta, map) = matched(config, &inputs, http)?;
    // Selected guide owns the mandatory budget. Optional catalogue facts must
    // never erase a usable selected result after a deadline or source refresh.
    let result = selected(config, entry, request, &meta, &map, http)?;
    let optional = (|| {
        let channels = inputs
            .iter()
            .map(|r| channel(r, true))
            .collect::<Result<Vec<_>, _>>()?;
        let data = fetch(
            config,
            "/current",
            Some(json!({"version":1,"source":config.source,"channels":channels,"search":""})),
            http,
        )?;
        let ids = inputs.iter().map(|r| r.entry_id.clone()).collect();
        let snapshot = wire::current(&data, &meta, &ids)
            .map_err(|_| "Invalid or stale hosted current guide")?;
        if snapshot.programmes.keys().any(|id| !map.contains_key(id)) {
            return Err("Hosted guide mapping changed");
        }
        Ok(snapshot)
    })();
    let mut snapshot = match optional {
        Ok(snapshot) => snapshot,
        Err(error) => {
            if http.cancelled() {
                return Err("Provider request cancelled");
            }
            let mut report = rows::CoverageReport::new(selectors.len());
            report.configured_sources = 1;
            report.attempted_sources = 1;
            report.reasons.insert(if http.row_guide_expired() {
                rows::CoverageReason::Deadline
            } else if matches!(
                error,
                "Invalid or stale hosted current guide" | "Hosted guide mapping changed"
            ) {
                rows::CoverageReason::SourceInvalid
            } else {
                rows::CoverageReason::SourceUnavailable
            });
            if limited {
                report.reasons.insert(rows::CoverageReason::SourceLimit);
            }
            return Ok((
                result,
                rows::Outcome {
                    snapshot: None,
                    report,
                },
            ));
        }
    };
    let wanted: BTreeSet<_> = selectors.iter().map(|r| r.entry_id.as_str()).collect();
    snapshot
        .programmes
        .retain(|id, _| wanted.contains(id.as_str()));
    let before = snapshot.programmes.len();
    snapshot
        .programmes
        .retain(|_, p| p.start_s <= now && now < p.end_s);
    let crossed = snapshot.now_s != now || before != snapshot.programmes.len();
    snapshot.now_s = now;
    snapshot.next_transition_s = snapshot.programmes.values().map(|p| p.end_s).min();
    let mut report = rows::CoverageReport::new(selectors.len());
    report.configured_sources = 1;
    report.attempted_sources = 1;
    report.loaded_sources = 1;
    if limited {
        report.reasons.insert(rows::CoverageReason::SourceLimit);
    }
    if crossed {
        report.reasons.insert(rows::CoverageReason::Deadline);
    }
    report.finish(
        selectors
            .iter()
            .filter(|r| map.contains_key(&r.entry_id))
            .count(),
        snapshot.programmes.len(),
    );
    Ok((
        result,
        rows::Outcome {
            snapshot: Some(snapshot),
            report,
        },
    ))
}
#[cfg(test)]
mod tests {
    use super::super::Response;
    use super::*;
    use std::collections::VecDeque;
    struct Http {
        responses: VecDeque<Value>,
        requests: Vec<Request>,
        cancel: bool,
    }
    impl ProviderHttp for Http {
        fn cancelled(&self) -> bool {
            self.cancel
        }
        fn request(&mut self, r: &Request) -> Result<Response, &'static str> {
            self.requests.push(Request {
                url: r.url.clone(),
                headers: r.headers.clone(),
                body: r.body.clone(),
            });
            Ok(Response {
                status: 200,
                body: serde_json::to_vec(
                    &self.responses.pop_front().ok_or("Unexpected fixture IO")?,
                )
                .unwrap(),
            })
        }
    }
    fn metadata() -> Value {
        json!({"version":1,"source":"epg-one","generation":"fixture-generation","fetchedAt":1000000,"refreshMs":7200000,"stale":false})
    }
    fn fixture() -> (Config, ProviderProfile, CatalogEntry, guide::Request) {
        let config = Config::parse(
            &json!({"version":1,"source":"epg-one","url":"https://guide.invalid/epg/v1"}),
        )
        .unwrap();
        let profile = ProviderProfile {
            id: "p".into(),
            name: "Local".into(),
            kind: ott::ProviderKind::M3u,
            source: "https://playlist.invalid/list.m3u".into(),
            source_alias: String::new(),
            enabled: true,
        };
        let entry = CatalogEntry { protection: None,
            id: "local".into(),
            title: "Station +1".into(),
            category: String::new(),
            description: String::new(),
            synopsis: None,
            poster: String::new(),
            media: ott::MediaLocator::Http {
                url: "https://stream.invalid/live".into(),
            },
            duration_ms: None,
            provider_id: Some("18".into()),
            provider_id_field: None,
            live: Some(guide::ChannelMetadata {
                guide_shift_seconds: 60,
                ..Default::default()
            }),
        };
        let request = guide::Request {
            context: guide::Context {
                generation: 7,
                profile_id: "p".into(),
                entry_id: "local".into(),
            },
            query: guide::Query::CurrentNext { now: 1000 },
        };
        (config, profile, entry, request)
    }
    fn mapping() -> Value {
        let mut v = metadata();
        v["mappings"] = json!({"local":{"channelId":"18","shift":3600,"logo":""}});
        v
    }
    fn programmes() -> Value {
        let mut v = metadata();
        v["rows"] = json!([{"time":900,"time_to":1100,"name":"Now","descr":"Full first sentence. And the second."}]);
        v
    }
    #[test]
    fn exact_mapping_generation_and_separate_provider_shift_are_applied_once() {
        let (c, p, e, r) = fixture();
        let mut h = Http {
            responses: VecDeque::from([mapping(), programmes()]),
            requests: Vec::new(),
            cancel: false,
        };
        let result = guide(&c, &p, &e, &r, &mut h).unwrap();
        assert_eq!(result.programmes[0].start, 960);
        assert_eq!(
            result.programmes[0].description,
            "Full first sentence. And the second."
        );
        assert!(h.requests[1].url.contains("shift=3600"));
        let body: Value = serde_json::from_slice(h.requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["channels"][0]["tvgId"], "18");
        assert!(body.to_string().find("stream.invalid").is_none());
        assert!(result.can_publish(&r, false));
    }
    #[test]
    fn changed_generation_cancellation_and_wrong_source_never_publish() {
        let (c, p, e, mut r) = fixture();
        let mut bad = programmes();
        bad["generation"] = json!("other");
        let mut h = Http {
            responses: VecDeque::from([mapping(), bad]),
            requests: Vec::new(),
            cancel: false,
        };
        assert!(guide(&c, &p, &e, &r, &mut h).is_err());
        h.cancel = true;
        let n = h.requests.len();
        assert!(guide(&c, &p, &e, &r, &mut h).is_err());
        assert_eq!(h.requests.len(), n);
        r.context.profile_id = "other".into();
        h.cancel = false;
        assert!(guide(&c, &p, &e, &r, &mut h).is_err());
        assert_eq!(h.requests.len(), n);
    }
    #[test]
    fn cancellation_after_match_io_rejects_the_response_before_followup_requests() {
        struct Cancel(Http);
        impl ProviderHttp for Cancel {
            fn cancelled(&self) -> bool {
                self.0.cancel
            }
            fn request(&mut self, r: &Request) -> Result<Response, &'static str> {
                let response = self.0.request(r);
                self.0.cancel = true;
                response
            }
        }
        let (c, p, e, r) = fixture();
        let mut h = Cancel(Http {
            responses: VecDeque::from([mapping(), programmes()]),
            requests: Vec::new(),
            cancel: false,
        });
        assert!(guide(&c, &p, &e, &r, &mut h).is_err());
        assert_eq!(h.0.requests.len(), 1);
    }

    #[test]
    fn optional_current_failure_or_replaced_generation_preserves_selected_guide() {
        let (c, p, e, r) = fixture();
        for optional in [
            None,
            Some(json!({"version":1,"source":"epg-one","generation":"replaced","stale":false})),
        ] {
            let mut replies = VecDeque::from([mapping(), programmes()]);
            if let Some(value) = optional {
                replies.push_back(value);
            }
            let mut h = Http {
                responses: replies,
                requests: Vec::new(),
                cancel: false,
            };
            let (result, outcome) = rows(&c, &p, &e, &r, &[selector(&e)], &mut h).unwrap();
            assert_eq!(result.programmes.len(), 1);
            assert_eq!(
                result.programmes[0].description,
                "Full first sentence. And the second."
            );
            assert!(h.requests[1].url.contains("/programmes?"));
            assert!(h.requests[2].url.ends_with("/current"));
            assert!(outcome.snapshot.is_none());
            assert_eq!(outcome.report.state, rows::CoverageState::Unavailable);
            assert!(!outcome.report.reasons.is_empty());
        }
    }

    #[test]
    fn oversized_catalogue_keeps_selected_guide_and_reports_truthful_partial_rows() {
        let (c, p, e, r) = fixture();
        let mut selectors = vec![selector(&e)];
        for n in 1..3000 {
            let mut row = selector(&e);
            row.entry_id = format!("row-{n}");
            selectors.push(row);
        }
        let mut current = metadata();
        current["asOf"] = json!(1000);
        current["checked"] = json!(2048);
        current["total"] = json!(2048);
        current["programs"] = json!([{"id":"local","start":960,"end":1160,"title":"Now"}]);
        let mut h = Http {
            responses: VecDeque::from([mapping(), programmes(), current]),
            requests: Vec::new(),
            cancel: false,
        };
        let (result, outcome) = rows(&c, &p, &e, &r, &selectors, &mut h).unwrap();
        assert_eq!(result.programmes.len(), 1);
        assert_eq!(outcome.report.requested, 3000);
        assert_eq!(outcome.report.state, rows::CoverageState::Partial);
        assert!(outcome
            .report
            .reasons
            .contains(&rows::CoverageReason::SourceLimit));
        let body: Value = serde_json::from_slice(h.requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["channels"].as_array().unwrap().len(), 2048);
        assert_eq!(body["channels"][0]["id"], "local");
    }
    #[test]
    fn newer_current_response_cannot_publish_a_future_title_at_the_owner_clock() {
        let (c, p, e, r) = fixture();
        let mut current = metadata();
        current["asOf"] = json!(1002);
        current["checked"] = json!(1);
        current["total"] = json!(1);
        current["programs"] = json!([{"id":"local","start":1001,"end":1100,"title":"New show"}]);
        let mut h = Http {
            responses: VecDeque::from([mapping(), programmes(), current]),
            requests: Vec::new(),
            cancel: false,
        };
        let (_, outcome) = rows(&c, &p, &e, &r, &[selector(&e)], &mut h).unwrap();
        assert_eq!(outcome.snapshot.as_ref().unwrap().now_s, 1000);
        assert!(outcome.snapshot.unwrap().programmes.is_empty());
        assert!(outcome
            .report
            .reasons
            .contains(&rows::CoverageReason::Deadline));
        assert_eq!(outcome.report.state, rows::CoverageState::Partial);
    }

    #[test]
    fn later_current_clock_reports_partial_even_when_visible_interval_is_safe() {
        let (c, p, e, r) = fixture();
        let mut current = metadata();
        current["asOf"] = json!(1002);
        current["checked"] = json!(1);
        current["total"] = json!(1);
        current["programs"] = json!([{"id":"local","start":960,"end":1160,"title":"Now"}]);
        let mut http = Http {
            responses: VecDeque::from([mapping(), programmes(), current]),
            requests: Vec::new(),
            cancel: false,
        };
        let (selected, outcome) = rows(&c, &p, &e, &r, &[selector(&e)], &mut http).unwrap();
        assert!(!selected.programmes.is_empty());
        assert_eq!(outcome.snapshot.unwrap().programmes["local"].start_s, 960);
        assert_eq!(outcome.report.state, rows::CoverageState::Partial);
        assert!(
            outcome
                .report
                .reasons
                .contains(&rows::CoverageReason::Deadline),
            "a later clock cannot prove the absence of omitted titles at the owner clock"
        );
    }

    #[test]
    fn compact_rows_and_selected_guide_share_one_snapshot_generation() {
        let (c, p, e, r) = fixture();
        let mut current = metadata();
        current["asOf"] = json!(1000);
        current["checked"] = json!(1);
        current["total"] = json!(1);
        current["programs"] = json!([{"id":"local","start":960,"end":1160,"title":"Now"}]);
        let mut h = Http {
            responses: VecDeque::from([mapping(), programmes(), current]),
            requests: Vec::new(),
            cancel: false,
        };
        let (result, outcome) = rows(&c, &p, &e, &r, &[selector(&e)], &mut h).unwrap();
        assert_eq!(result.programmes[0].start, 960);
        assert_eq!(outcome.report.state, rows::CoverageState::Complete);
        assert_eq!(outcome.snapshot.unwrap().programmes["local"].start_s, 960);
        let body: Value = serde_json::from_slice(h.requests[2].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["channels"][0]["shift"], 60);
    }
}

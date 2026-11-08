//! Application-owned Xtream requests. Runs on the catalogue/resolve worker, never SDL.
use super::{get_json, ProviderHttp};
use plx_data::ott::{
    self,
    xtream::{self, Config},
    CatalogEntry, MediaLocator, ProviderProfile,
};
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_REQUESTS: usize = 4096;
const BATCH_SIZE: usize = 128;

struct Requests<'a> {
    http: &'a mut dyn ProviderHttp,
    remaining: usize,
}
impl Requests<'_> {
    fn json(&mut self, url: String) -> Result<Value, &'static str> {
        if self.http.cancelled() {
            return Err("Provider request cancelled");
        }
        if self.remaining == 0 {
            return Err("Xtream request limit reached");
        }
        self.remaining -= 1;
        let value = get_json(self.http, url, Vec::new())?;
        if self.http.cancelled() {
            return Err("Provider request cancelled");
        }
        // Some portals answer expired credentials with HTTP 200.
        if value.get("user_info").is_some() {
            xtream::validate_account(&value).map_err(|_| "Xtream account is unavailable")?;
        }
        Ok(value)
    }
    fn authenticate(&mut self, config: &Config) -> Result<(), &'static str> {
        xtream::validate_account(&self.json(config.api_url("", &[]))?)
            .map_err(|_| "Xtream authentication failed")
    }
}

pub(super) fn visit(
    profile: &ProviderProfile,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let config = Config::parse(&profile.source).map_err(|_| "Invalid Xtream profile")?;
    let mut req = Requests {
        http,
        remaining: MAX_REQUESTS,
    };
    req.authenticate(&config)?;
    let mut seen = BTreeSet::new();
    for kind in ["live", "vod", "series"] {
        let groups =
            xtream::categories(&req.json(config.api_url(&format!("get_{kind}_categories"), &[]))?)
                .map_err(|_| "Invalid Xtream categories")?;
        // Standard Xtream has no numeric pagination. Fetch the complete bounded
        // array: category-only queries would lose unassigned or stale-category rows.
        // Publish each section before fetching the next, and episodes per series.
        let action = if kind == "series" {
            "get_series".to_owned()
        } else {
            format!("get_{kind}_streams")
        };
        let value = req.json(config.api_url(&action, &[]))?;
        if kind == "series" {
            for series in
                xtream::series(&value, &config, &groups).map_err(|_| "Invalid Xtream series")?
            {
                let value =
                    req.json(config.api_url("get_series_info", &[("series_id", &series.id)]))?;
                let catalog = xtream::episodes(&value, profile, &config, &series)
                    .map_err(|_| "Invalid Xtream episodes")?;
                publish(catalog.entries, &mut seen, &mut req, emit)?;
            }
        } else {
            let stream_kind = if kind == "vod" { "movie" } else { "live" };
            let catalog = xtream::streams(&value, profile, &config, stream_kind, &groups)
                .map_err(|_| "Invalid Xtream streams")?;
            publish(catalog.entries, &mut seen, &mut req, emit)?;
        }
    }
    Ok(())
}
fn publish(
    rows: Vec<CatalogEntry>,
    seen: &mut BTreeSet<String>,
    req: &mut Requests<'_>,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut batch = Vec::new();
    for row in rows {
        if req.http.cancelled() {
            return Err("Provider request cancelled");
        }
        if !seen.insert(row.id.clone()) {
            continue;
        }
        if seen.len() > ott::MAX_CATALOG_ENTRIES {
            return Err("Xtream catalogue limit reached");
        }
        batch.push(row);
        if batch.len() == BATCH_SIZE {
            emit(std::mem::take(&mut batch))?;
        }
    }
    if !batch.is_empty() {
        emit(batch)?;
    }
    Ok(())
}

pub(super) fn resolve(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<String, &'static str> {
    let config = Config::parse(&profile.source).map_err(|_| "Invalid Xtream profile")?;
    let MediaLocator::Xtream {
        stream_id,
        stream_kind,
        extension,
        series_id,
        has_direct_source,
    } = &entry.media
    else {
        return Err("Invalid Xtream media locator");
    };
    // Validate BEFORE putting an imported identifier on the wire.
    let generated = config
        .stream_url(stream_kind, stream_id, extension)
        .map_err(|_| "Invalid Xtream stream identity")?;
    let mut req = Requests { http, remaining: 2 };
    req.authenticate(&config)?;
    if stream_kind == "live" && *has_direct_source {
        let value = req.json(config.api_url("get_live_streams", &[]))?;
        let rows = value.as_array().ok_or("Invalid Xtream streams")?;
        if rows.len() > ott::MAX_CATALOG_ENTRIES {
            return Err("Xtream catalogue limit reached");
        }
        if let Some(row) = rows
            .iter()
            .find(|row| id_matches(&row["stream_id"], stream_id))
        {
            if let Some(direct) = xtream::direct_source(&config, row) {
                return Ok(direct);
            }
        }
    } else if stream_kind == "series" {
        if let Some(series_id) = series_id {
            if !xtream::numeric(series_id) {
                return Err("Invalid Xtream series identity");
            }
            let value = req.json(config.api_url("get_series_info", &[("series_id", series_id)]))?;
            let rows = xtream::episode_rows(&value).map_err(|_| "Invalid Xtream episodes")?;
            let (_, row) = rows
                .into_iter()
                .find(|(_, row)| id_matches(&row["id"], stream_id))
                .ok_or("Xtream episode no longer exists")?;
            if let Some(direct) = xtream::direct_source(&config, row) {
                return Ok(direct);
            }
            if let Some(ext) = row["container_extension"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                return config
                    .stream_url(stream_kind, stream_id, ext)
                    .map_err(|_| "Invalid Xtream container extension");
            }
        }
    }
    if stream_kind == "movie" {
        let value = req.json(config.api_url("get_vod_info", &[("vod_id", stream_id)]))?;
        let movie = value
            .get("movie_data")
            .filter(|v| v.is_object())
            .ok_or("Invalid Xtream movie metadata")?;
        if movie.get("stream_id").is_some() && !id_matches(&movie["stream_id"], stream_id) {
            return Err("Xtream returned a different movie");
        }
        if let Some(direct) = xtream::direct_source(&config, movie) {
            return Ok(direct);
        }
        if let Some(ext) = movie
            .get("container_extension")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return config
                .stream_url(stream_kind, stream_id, ext)
                .map_err(|_| "Invalid Xtream container extension");
        }
    }
    Ok(generated)
}
fn id_matches(value: &Value, id: &str) -> bool {
    value
        .as_str()
        .map(|s| s == id)
        .unwrap_or_else(|| value.as_u64().is_some_and(|n| n.to_string() == id))
}

/// Lazy metadata read for a selected item; it never delays earlier catalogue batches.
/// Result is plain provider-neutral JSON for the application's details/EPG presentation.
pub(super) fn metadata(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<Value, &'static str> {
    let config = Config::parse(&profile.source).map_err(|_| "Invalid Xtream profile")?;
    let MediaLocator::Xtream {
        stream_id,
        stream_kind,
        extension,
        ..
    } = &entry.media
    else {
        return Err("Invalid Xtream media locator");
    };
    config
        .stream_url(stream_kind, stream_id, extension)
        .map_err(|_| "Invalid Xtream stream identity")?;
    if stream_kind == "live" {
        let guide = short_guide(&config, stream_id, http)?;
        return Ok(
            serde_json::json!({"programmes":guide.into_iter().map(|p|serde_json::json!({"start":p.start,"end":p.end,"title":p.title,"description":p.description})).collect::<Vec<_>>()}),
        );
    }
    let mut req = Requests { http, remaining: 2 };
    req.authenticate(&config)?;
    match stream_kind.as_str() {
        "movie" => {
            let value = req.json(config.api_url("get_vod_info", &[("vod_id", stream_id)]))?;
            let info =
                xtream::movie_info(&value, &config).map_err(|_| "Invalid Xtream movie metadata")?;
            // Keep only the fields the UI needs, never return raw provider credentials.
            serde_json::to_value(info).map_err(|_| "Invalid Xtream movie metadata")
        }
        "series" => Ok(
            serde_json::json!({"description":entry.description,"poster":entry.poster,"durationSeconds":entry.duration_ms.map(|m|m/1000)}),
        ),
        _ => Err("Invalid Xtream stream kind"),
    }
}

fn short_guide(
    config: &Config,
    stream_id: &str,
    http: &mut dyn ProviderHttp,
) -> Result<Vec<xtream::Programme>, &'static str> {
    let mut req = Requests { http, remaining: 2 };
    req.authenticate(config)?;
    let value =
        req.json(config.api_url("get_short_epg", &[("stream_id", stream_id), ("limit", "8")]))?;
    xtream::short_epg(&value).map_err(|_| "Invalid Xtream programme guide")
}

/// Project the existing short-guide response, without inventing a day endpoint.
pub(super) fn guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    if request.context.profile_id != profile.id || request.context.entry_id != entry.id {
        return Err("Guide context mismatch");
    }
    request
        .query
        .validate()
        .map_err(|_| "Invalid guide interval")?;
    if matches!(request.query, ott::guide::Query::Day { .. }) {
        return Err("Provider offers only a short programme guide");
    }
    if profile.kind != ott::ProviderKind::Xtream {
        return Err("Invalid Xtream profile");
    }
    let config = Config::parse(&profile.source).map_err(|_| "Invalid Xtream profile")?;
    let MediaLocator::Xtream {
        stream_id,
        stream_kind,
        extension,
        ..
    } = &entry.media
    else {
        return Err("Invalid Xtream media locator");
    };
    if stream_kind != "live" {
        return Err("Guide requires a live channel");
    }
    config
        .stream_url(stream_kind, stream_id, extension)
        .map_err(|_| "Invalid Xtream stream identity")?;
    let rows: Vec<_> = short_guide(&config, stream_id, http)?
        .into_iter()
        .map(|row| ott::epg::Programme {
            channel: stream_id.clone(),
            start: row.start,
            end: row.end,
            title: row.title,
            description: row.description,
        })
        .collect();
    let programmes = ott::guide::select(&rows, stream_id, request.query, 0)
        .map_err(|_| "Invalid Xtream programme guide")?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    Ok(ott::guide::ResultSet {
        context: request.context.clone(),
        query: request.query,
        coverage: ott::guide::Coverage::ProviderShortGuide,
        programmes,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Request, Response};
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    struct Fixture {
        responses: BTreeMap<String, Value>,
        calls: Vec<String>,
        cancelled: bool,
        cancel_after: Option<usize>,
    }
    impl ProviderHttp for Fixture {
        fn request(&mut self, r: &Request) -> Result<Response, &'static str> {
            self.calls.push(r.url.clone());
            let body = self
                .responses
                .get(&r.url)
                .ok_or("Unexpected fixture request")?
                .to_string()
                .into_bytes();
            if self.cancel_after == Some(self.calls.len()) {
                self.cancelled = true;
            }
            Ok(Response { status: 200, body })
        }
        fn cancelled(&self) -> bool {
            self.cancelled
        }
    }
    fn setup() -> (ProviderProfile, Fixture, Config) {
        let source =
            r#"{"server":"https://fixture.invalid","username":"test","password":"secret"}"#;
        let profile = ProviderProfile {
            id: "xc".into(),
            name: "Fixture".into(),
            kind: ott::ProviderKind::Xtream,
            source: source.into(),
            source_alias: "Fixture".into(),
            enabled: true,
        };
        let c = Config::parse(source).unwrap();
        let mut responses = BTreeMap::new();
        responses.insert(
            c.api_url("", &[]),
            json!({"user_info":{"auth":1,"status":"Active"}}),
        );
        for kind in ["live", "vod", "series"] {
            responses.insert(c.api_url(&format!("get_{kind}_categories"), &[]), json!([]));
            responses.insert(
                c.api_url(
                    if kind == "series" {
                        "get_series"
                    } else if kind == "vod" {
                        "get_vod_streams"
                    } else {
                        "get_live_streams"
                    },
                    &[],
                ),
                json!([]),
            );
        }
        (
            profile,
            Fixture {
                responses,
                calls: vec![],
                cancelled: false,
                cancel_after: None,
            },
            c,
        )
    }
    #[test]
    fn publishes_live_before_following_requests() {
        let (p, mut f, c) = setup();
        f.responses.insert(
            c.api_url("get_live_streams", &[]),
            json!([{"stream_id":42,"name":"News"}]),
        );
        let mut rows = vec![];
        visit(&p, &mut f, &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "News");
        assert_eq!(f.calls.len(), 7);
    }
    #[test]
    fn auth_failure_does_not_query_or_publish() {
        let (p, mut f, c) = setup();
        f.responses
            .insert(c.api_url("", &[]), json!({"user_info":{"auth":0}}));
        let mut published = false;
        assert!(visit(&p, &mut f, &mut |_| {
            published = true;
            Ok(())
        })
        .is_err());
        assert!(!published);
        assert_eq!(f.calls.len(), 1);
    }
    #[test]
    fn cancelled_response_is_never_published() {
        let (p, mut f, c) = setup();
        f.responses.insert(
            c.api_url("get_live_streams", &[]),
            json!([{"stream_id":42,"name":"News"}]),
        );
        f.cancel_after = Some(3);
        let mut rows = vec![];
        assert!(visit(&p, &mut f, &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .is_err());
        assert!(rows.is_empty());
        assert_eq!(f.calls.len(), 3);
    }
    #[test]
    fn includes_uncategorized_rows_and_deduplicates_server_repeats() {
        let (p, mut f, c) = setup();
        f.responses.insert(c.api_url("get_live_categories",&[]),json!([{"category_id":1,"category_name":"One"},{"category_id":2,"category_name":"Two"}]));
        f.responses.insert(
            c.api_url("get_live_streams", &[]),
            json!([{"stream_id":42,"name":"News","category_id":1},
                {"stream_id":42,"name":"Repeat","category_id":1},
                {"stream_id":43,"name":"Uncategorized","category_id":0}]),
        );
        let mut rows = vec![];
        visit(&p, &mut f, &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].title, "Uncategorized");
        assert!(f.calls.contains(&c.api_url("get_live_streams", &[])));
    }
    #[test]
    fn resolution_refreshes_container_without_changing_id() {
        let (p, mut f, c) = setup();
        let e = xtream::streams(
            &json!([{"stream_id":5,"name":"Film","container_extension":"mp4"}]),
            &p,
            &c,
            "movie",
            &BTreeMap::new(),
        )
        .unwrap()
        .entries
        .remove(0);
        f.responses.insert(
            c.api_url("get_vod_info", &[("vod_id", "5")]),
            json!({"movie_data":{"stream_id":5,"container_extension":"mkv"}}),
        );
        assert!(resolve(&p, &e, &mut f)
            .unwrap()
            .ends_with("/movie/test/secret/5.mkv"));
    }
    #[test]
    fn live_and_episode_resolution_refresh_direct_sources() {
        let (p, mut f, c) = setup();
        let live = xtream::streams(
            &json!([{"stream_id":42,"name":"News","direct_source":"https://media.invalid/expired.m3u8"}]),
            &p,
            &c,
            "live",
            &BTreeMap::new(),
        )
        .unwrap()
        .entries
        .remove(0);
        f.responses.insert(
            c.api_url("get_live_streams", &[]),
            json!([{"stream_id":42,"direct_source":"https://media.invalid/live.m3u8"}]),
        );
        assert_eq!(
            resolve(&p, &live, &mut f).unwrap(),
            "https://media.invalid:443/live.m3u8"
        );
        let parent = xtream::Series {
            id: "10".into(),
            name: "Show".into(),
            category: "Series".into(),
            poster: String::new(),
            description: String::new(),
        };
        let ep = xtream::episodes(
            &json!({"episodes":{"1":[{"id":15,"container_extension":"mp4"}]}}),
            &p,
            &c,
            &parent,
        )
        .unwrap()
        .entries
        .remove(0);
        f.responses.insert(
            c.api_url("get_series_info", &[("series_id", "10")]),
            json!({"episodes":[{"id":15,"direct_source":"https://media.invalid/episode.mp4"}]}),
        );
        assert_eq!(
            resolve(&p, &ep, &mut f).unwrap(),
            "https://media.invalid:443/episode.mp4"
        );
    }
    #[test]
    fn normal_live_resolution_does_not_download_catalog_again() {
        let (p, mut f, c) = setup();
        let live = xtream::streams(
            &json!([{"stream_id":42,"name":"News"}]),
            &p,
            &c,
            "live",
            &BTreeMap::new(),
        )
        .unwrap()
        .entries
        .remove(0);
        assert!(resolve(&p, &live, &mut f)
            .unwrap()
            .ends_with("/live/test/secret/42.m3u8"));
        assert_eq!(f.calls, vec![c.api_url("", &[])]);
    }
    #[test]
    fn missing_episode_is_not_replaced_by_a_different_file() {
        let (p, mut f, c) = setup();
        let parent = xtream::Series {
            id: "10".into(),
            name: "Show".into(),
            category: "Series".into(),
            poster: String::new(),
            description: String::new(),
        };
        let ep = xtream::episodes(
            &json!({"episodes":[{"id":15,"container_extension":"mp4"}]}),
            &p,
            &c,
            &parent,
        )
        .unwrap()
        .entries
        .remove(0);
        f.responses.insert(
            c.api_url("get_series_info", &[("series_id", "10")]),
            json!({"episodes":[{"id":16,"container_extension":"mp4"}]}),
        );
        assert_eq!(
            resolve(&p, &ep, &mut f).unwrap_err(),
            "Xtream episode no longer exists"
        );
    }
    fn guide_fixture() -> (
        ProviderProfile,
        Fixture,
        Config,
        CatalogEntry,
        ott::guide::Request,
    ) {
        let (profile, mut fixture, config) = setup();
        let mut entry = xtream::streams(
            &json!([{ "stream_id": 42, "name": "News" }]),
            &profile,
            &config,
            "live",
            &BTreeMap::new(),
        )
        .unwrap()
        .entries
        .remove(0);
        entry.provider_id = Some("unrelated-display-id".into());
        let request = ott::guide::Request {
            context: ott::guide::Context {
                generation: 8,
                profile_id: profile.id.clone(),
                entry_id: entry.id.clone(),
            },
            query: ott::guide::Query::CurrentNext { now: 250 },
        };
        fixture.responses.insert(
            config.api_url("get_short_epg", &[("stream_id", "42"), ("limit", "8")]),
            json!({"epg_listings":[
                {"start_timestamp":100,"stop_timestamp":200,"title":"Past programme"},
                {"start_timestamp":200,"stop_timestamp":300,"title":"Current programme"},
                {"start_timestamp":300,"stop_timestamp":400,"title":"Next programme"},
                {"start_timestamp":400,"stop_timestamp":300,"title":"Invalid interval"}
            ]}),
        );
        (profile, fixture, config, entry, request)
    }

    #[test]
    fn dispatched_typed_short_guide_preserves_context_and_uses_locator_channel() {
        let (profile, mut fixture, config, entry, request) = guide_fixture();
        let result = super::super::guide_with(&profile, &entry, &request, &mut fixture).unwrap();
        assert_eq!(
            fixture.calls,
            [
                config.api_url("", &[]),
                config.api_url("get_short_epg", &[("stream_id", "42"), ("limit", "8")])
            ]
        );
        assert_eq!(result.coverage, ott::guide::Coverage::ProviderShortGuide);
        assert_eq!(result.context, request.context);
        assert_eq!(result.query, request.query);
        assert_eq!(
            result
                .programmes
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            ["Current programme", "Next programme"]
        );
        assert!(result.programmes.iter().all(|p| p.channel == "42"));
        let legacy = metadata(&profile, &entry, &mut fixture).unwrap();
        assert_eq!(legacy["programmes"].as_array().unwrap().len(), 3);
        assert_eq!(legacy["programmes"][0]["title"], "Past programme");
    }

    #[test]
    fn short_guide_only_providers_reject_day_before_io() {
        let (mut profile, mut fixture, _, mut entry, mut request) = guide_fixture();
        request.query = ott::guide::Query::Day {
            start: 100,
            end: 400,
        };
        assert_eq!(
            super::super::guide_with(&profile, &entry, &request, &mut fixture),
            Err("Provider offers only a short programme guide")
        );
        assert!(fixture.calls.is_empty());
        profile.kind = ott::ProviderKind::Stalker;
        profile.source =
            r#"{"portal":"https://portal.invalid/c/","mac":"00:1a:79:01:02:03"}"#.into();
        entry.media = MediaLocator::Stalker {
            cmd: "https://stream.invalid/live".into(),
            stream_kind: "itv".into(),
            series: String::new(),
        };
        assert_eq!(
            super::super::guide_with(&profile, &entry, &request, &mut fixture),
            Err("Provider offers only a short programme guide")
        );
        assert!(fixture.calls.is_empty());
    }

    #[test]
    fn malformed_channel_and_cancelled_short_guide_do_not_escape() {
        let (profile, mut fixture, _, mut entry, request) = guide_fixture();
        let original = entry.media.clone();
        if let MediaLocator::Xtream { stream_id, .. } = &mut entry.media {
            *stream_id = "../42".into();
        }
        assert!(super::super::guide_with(&profile, &entry, &request, &mut fixture).is_err());
        assert!(fixture.calls.is_empty());
        entry.media = original;
        fixture.cancel_after = Some(2);
        assert_eq!(
            super::super::guide_with(&profile, &entry, &request, &mut fixture),
            Err("Provider request cancelled")
        );
        assert_eq!(fixture.calls.len(), 2);
    }
}

//! Native transport for the canonical closed operator registry. The worker owns all IO.
use super::{ott, CatalogEntry, MediaLocator, ProviderHttp, ProviderProfile, Request};
use ott::{OperatorId, ProviderKind};
use plx_shared_core::operators::{
    self as core, CatalogProfile, NamedProfile, Phase, SourceAction, StreamProfile,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .unwrap_or_default()
}
fn response(http: &mut dyn ProviderHttp, url: String) -> Result<Vec<u8>, &'static str> {
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    if ott::http_url(&url).is_none() {
        return Err("Invalid operator request");
    }
    let response = http.request(&Request {
        url,
        headers: vec!["User-Agent: Mozilla/5.0".into()],
        body: None,
    })?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    if !(200..300).contains(&response.status) || response.body.len() > ott::MAX_CATALOG_BYTES {
        return Err("Operator request failed or exceeds limit");
    }
    Ok(response.body)
}
fn parse_json(bytes: &[u8]) -> Result<Value, &'static str> {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Ok(value);
    }
    let body = std::str::from_utf8(bytes)
        .map_err(|_| "Invalid operator JSON")?
        .trim();
    let (callback, rest) = body.split_once('(').ok_or("Invalid operator JSON")?;
    if callback.is_empty()
        || !callback
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.$".contains(&b))
    {
        return Err("Invalid operator JSON callback");
    }
    let json = rest
        .trim_end_matches(';')
        .strip_suffix(')')
        .ok_or("Invalid operator JSON")?;
    serde_json::from_str(json).map_err(|_| "Invalid operator JSON")
}
fn as_m3u(profile: &ProviderProfile, url: String) -> ProviderProfile {
    let mut p = profile.clone();
    p.kind = ProviderKind::M3u;
    p.source = url;
    p
}

pub(super) fn visit(
    profile: &ProviderProfile,
    id: OperatorId,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let config = ott::operators::Config::parse(id, &profile.source)
        .map_err(|_| "Invalid operator profile")?;
    if matches!(id.driver_kind(), "operator" | "xtream-fallback") {
        let cfg = core::GenericConfig {
            m3u: config.playlist.clone(),
            server: config.server.clone(),
            user: config.username.clone(),
            pass: config.password.clone(),
        };
        let initial = core::generic_plan(id.canonical_id(), &cfg, None, false)
            .map_err(|_| "Invalid operator configuration")?;
        match initial.action {
            SourceAction::Configure => return Err("Configure provider credentials or playlist"),
            SourceAction::Playlist => {
                return super::m3u(
                    &as_m3u(profile, initial.request.ok_or("Missing operator playlist")?),
                    http,
                    emit,
                )
            }
            SourceAction::Api => {
                let result = response(http, initial.request.ok_or("Missing operator request")?)
                    .and_then(|body| parse_json(&body));
                let plan = core::generic_plan(
                    id.canonical_id(),
                    &cfg,
                    result.as_ref().ok(),
                    result.is_err(),
                )
                .map_err(|_| "Invalid operator response")?;
                if plan.action == SourceAction::Playlist {
                    return super::m3u(
                        &as_m3u(profile, plan.request.ok_or("Missing operator fallback")?),
                        http,
                        emit,
                    );
                }
                let mut rows = Vec::new();
                for channel in plan.channels {
                    if http.cancelled() {
                        return Err("Provider request cancelled");
                    }
                    rows.push(generic_row(profile, &channel)?);
                    if rows.len() >= 64 {
                        emit(std::mem::take(&mut rows))?;
                    }
                }
                if !rows.is_empty() {
                    emit(rows)?;
                }
                return Ok(());
            }
        }
    }
    if id.driver_kind() == "catalog" {
        return visit_catalog(profile, id, &config, http, emit);
    }
    let named = core::OperatorId::from_id(id.canonical_id())
        .and_then(|id| id.named_profile())
        .ok_or("Unsupported named operator")?;
    let first = if id.canonical_id() == "kb-team" {
        config.mac.as_str()
    } else {
        config.username.as_str()
    };
    if id.canonical_id() != "tvteam"
        && !core::credentials_valid(named, first, &config.password)
            .map_err(|_| "Invalid operator credentials")?
    {
        return Err("Configure valid operator credentials");
    }
    let mac = config.mac.replace(':', "").to_lowercase();
    let mut parameters = json!({"base":"http://list.1ott.net","id":config.username,"login":config.username,"password":config.password,"pin":config.password,"token":config.username,"key":config.username,"mac":mac,"list":config.list,"url":config.playlist});
    if id.canonical_id() == "1ott" {
        let account = core::profile_url(named, Phase::Account, &parameters)
            .map_err(|_| "Invalid operator account request")?;
        let account = parse_json(&response(http, account)?)?;
        let token = account["token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("Operator rejected account credentials")?;
        parameters["token"] = json!(token);
    }
    let url = core::profile_url(named, Phase::Playlist, &parameters)
        .map_err(|_| "Invalid operator playlist request")?;
    let bytes = response(http, url)?;
    let body = std::str::from_utf8(&bytes).map_err(|_| "Invalid operator playlist text")?;
    let mut hashes = BTreeMap::new();
    if id.canonical_id() == "kb-team" {
        for line in body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let key = line.split('?').next().unwrap_or(line);
            hashes.insert(key.to_owned(), murmur3(key, 10));
        }
    }
    let catalog = core::parse_operator_playlist(named, body, &[], &hashes)
        .map_err(|_| "Invalid operator playlist")?;
    publish(profile, id, &config, &catalog, http, emit)
}
fn generic_row(profile: &ProviderProfile, channel: &Value) -> Result<CatalogEntry, &'static str> {
    let id = text(&channel["itemId"]);
    let title = text(&channel["name"]);
    let url = ott::http_url(&text(&channel["url"])).ok_or("Invalid operator media address")?;
    if id.is_empty() || id.len() > 1024 || title.len() > 1024 {
        return Err("Invalid operator channel");
    }
    Ok(CatalogEntry {
        id: ott::favorites_wire::hash(format!("{}\0{id}", profile.id).as_bytes()),
        title,
        category: text(&channel["groupName"]),
        description: String::new(),
        synopsis: None,
        poster: ott::http_url(&text(&channel["logo"])).unwrap_or_default(),
        media: MediaLocator::Http { url },
        duration_ms: None,
        provider_id: Some(text(&channel["providerId"])),
        provider_id_field: None,
        live: Some(Default::default()),
    })
}
fn publish(
    profile: &ProviderProfile,
    id: OperatorId,
    config: &ott::operators::Config,
    catalog: &Value,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if catalog["malformed"].as_bool() == Some(true) {
        return Err("Invalid operator catalogue");
    }
    let ids = catalog["ids"]
        .as_array()
        .filter(|rows| rows.len() <= ott::MAX_CATALOG_ENTRIES)
        .ok_or("Invalid operator catalogue")?;
    let mut rows = Vec::new();
    for item in ids {
        if http.cancelled() {
            return Err("Provider request cancelled");
        }
        let key = text(item);
        let channel = &catalog["channels"][&key];
        if key.is_empty() || key.len() > 512 || !channel.is_object() {
            return Err("Invalid operator channel identity");
        }
        let mut url = text(&channel["url"]);
        let stream = match id.canonical_id() {
            "only4" => Some(StreamProfile::Only4),
            "antifriz" => Some(StreamProfile::Antifriz),
            "itv" => Some(StreamProfile::Itv),
            "ottclub" => Some(StreamProfile::Ottclub),
            "shura" => Some(StreamProfile::Shura),
            _ => None,
        };
        if let Some(stream) = stream {
            url = core::live_url(
                stream,
                &key,
                channel,
                &json!({"mode":config.mode,"hls":0,"key":config.username,"server":config.server}),
            )
            .map_err(|_| "Invalid operator stream")?;
        }
        let url = ott::http_url(&url).ok_or("Invalid operator media address")?;
        let title = text(&channel["channel_name"]);
        let category = text(&channel["category"]["name"]);
        if title.len() > 1024 || category.len() > 1024 {
            return Err("Operator channel metadata exceeds limit");
        }
        rows.push(CatalogEntry {
            id: ott::favorites_wire::hash(format!("{}\0{key}", profile.id).as_bytes()),
            title,
            category,
            description: String::new(),
            synopsis: None,
            poster: logo_url(id, config, &key, channel),
            media: MediaLocator::Http { url },
            duration_ms: None,
            provider_id: Some(key.clone()),
            provider_id_field: None,
            live: Some(ott::guide::ChannelMetadata {
                guide_id: (!text(&channel["epg_id"]).is_empty())
                    .then(|| text(&channel["epg_id"]))
                    .or_else(|| (!text(&channel["epg"]).is_empty()).then(|| text(&channel["epg"]))),
                guide_feed: text(&channel["utvg"]),
                tvg_name: text(&channel["tn"]),
                catchup: archive_source(id, config, &key, channel),
                ..Default::default()
            }),
        });
        if rows.len() >= 64 {
            emit(std::mem::take(&mut rows))?;
        }
    }
    if !rows.is_empty() {
        emit(rows)?;
    }
    Ok(())
}
fn visit_catalog(
    profile: &ProviderProfile,
    id: OperatorId,
    config: &ott::operators::Config,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let kind = match id.canonical_id() {
        "itv" => CatalogProfile::Itv,
        "ottclub" => CatalogProfile::Ottclub,
        "shura" => CatalogProfile::Shura,
        _ => return Err("Unsupported catalogue operator"),
    };
    if !core::catalog_credentials_valid(kind, &config.username, &config.server)
        .map_err(|_| "Invalid catalogue credentials")?
    {
        return Err("Configure valid operator credentials");
    }
    let url = match id.canonical_id() {
        "itv" => format!("http://api.01cdn.wf/data/{}", config.username),
        "ottclub" => format!("http://{}/api/channel_now", config.server),
        _ => format!(
            "{}?type=jsonp&uid=shxxxxxxxxxxx",
            core::profile_url(NamedProfile::Shura, Phase::Account, &json!({}))
                .map_err(|_| "Invalid operator request")?
        ),
    };
    let bytes = response(http, url)?;
    let raw = std::str::from_utf8(&bytes).map_err(|_| "Invalid operator catalogue text")?;
    let ids = if id.canonical_id() == "ottclub" {
        core::club_ids(raw).map_err(|_| "Invalid operator channel identities")?
    } else {
        Vec::new()
    };
    let data = parse_json(&bytes)?;
    let mut catalog = core::catalog(kind, &data, &ids).map_err(|_| "Invalid operator catalogue")?;
    if id.canonical_id() == "shura" {
        let url = core::profile_url(NamedProfile::Shura, Phase::Categories, &json!({}))
            .map_err(|_| "Invalid operator category request")?;
        let bytes = response(http, url)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid operator category text")?;
        let ids = catalog["ids"]
            .as_array()
            .ok_or("Invalid operator catalogue")?
            .iter()
            .map(self::text)
            .collect::<Vec<_>>();
        let groups =
            core::parse_operator_playlist(NamedProfile::Shura, text, &ids, &BTreeMap::new())
                .map_err(|_| "Invalid operator categories")?;
        if let Some(entries) = groups["entries"].as_array() {
            for entry in entries {
                let id = self::text(&entry["id"]);
                catalog["channels"][&id]["category"] = entry["channel"]["category"].clone();
            }
        }
    }
    publish(profile, id, config, &catalog, http, emit)
}
fn murmur3(text: &str, seed: u32) -> u32 {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return 0;
    }
    let mut hash = seed;
    for chunk in bytes.chunks_exact(4) {
        let mut key = u32::from_le_bytes(chunk.try_into().unwrap());
        key = key
            .wrapping_mul(0xcc9e2d51)
            .rotate_left(15)
            .wrapping_mul(0x1b873593);
        hash ^= key;
        hash = hash
            .rotate_left(13)
            .wrapping_mul(5)
            .wrapping_add(0xe6546b64);
    }
    let tail = bytes.chunks_exact(4).remainder();
    let mut key = 0u32;
    for (i, b) in tail.iter().enumerate() {
        key |= (*b as u32) << (i * 8);
    }
    if !tail.is_empty() {
        hash ^= key
            .wrapping_mul(0xcc9e2d51)
            .rotate_left(15)
            .wrapping_mul(0x1b873593);
    }
    hash ^= bytes.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85ebca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2ae35);
    hash ^ (hash >> 16)
}

fn logo_url(id: OperatorId, config: &ott::operators::Config, key: &str, channel: &Value) -> String {
    let logo = match id.canonical_id() {
        "itv" => format!("http://api.01cdn.wf/icon/{key}"),
        "shura" => format!("http://s{}.tvshka.net:81/picon/{key}.png", config.server),
        "ottclub" if !text(&channel["img"]).is_empty() => {
            format!("http://{}/images/{}", config.server, text(&channel["img"]))
        }
        _ => text(&channel["logo"]),
    };
    ott::http_url(&logo).unwrap_or_default()
}

fn archive_source(
    id: OperatorId,
    config: &ott::operators::Config,
    key: &str,
    channel: &Value,
) -> Option<ott::archive::CatchupSource> {
    let hours = channel["rec"]
        .as_f64()
        .or_else(|| channel["rec"].as_str().and_then(|value| value.parse().ok()))?;
    if !hours.is_finite() || hours <= 0.0 || hours > 8760.0 {
        return None;
    }
    let raw = text(&channel["url"]);
    let (mode, url, first, second, streammode) = match id.canonical_id() {
        "only4" => ("only4", raw, String::new(), String::new(), config.mode),
        "tvteam" => ("auto-utc-now", raw, String::new(), String::new(), 0),
        "1ott" | "shara-tv" => ("utc", raw, String::new(), String::new(), 0),
        "itv" => (
            "itv",
            format!("http://{}/{key}/", text(&channel["server_cdn"])),
            format!("?token={}", text(&channel["token"])),
            String::new(),
            config.mode,
        ),
        "antifriz" => (
            "antifriz",
            format!("http://{}:80/{key}/", text(&channel["server"])),
            format!("?token={}", text(&channel["token"])),
            String::new(),
            config.mode,
        ),
        "kb-team" => ("kb", raw, text(&channel["caso"]), text(&channel["ca"]), 0),
        "ottclub" | "shura" => {
            let profile = if id.canonical_id() == "ottclub" {
                StreamProfile::Ottclub
            } else {
                StreamProfile::Shura
            };
            let url = core::live_url(
                profile,
                key,
                channel,
                &json!({"key":config.username,"server":config.server,"mode":config.mode}),
            )
            .ok()?;
            (
                if id.canonical_id() == "ottclub" {
                    "club"
                } else {
                    "archive"
                },
                url,
                String::new(),
                String::new(),
                0,
            )
        }
        _ => return None,
    };
    ott::http_url(&url)?;
    Some(ott::archive::CatchupSource{mode:ott::archive::CatchupMode::Provider,source:json!({"operator":id.canonical_id(),"mode":mode,"url":url,"first":first,"second":second,"streammode":streammode}).to_string(),retention_seconds:(hours*3600.0).round() as u64,correction_seconds:0})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Http {
        replies: VecDeque<(&'static str, u16, &'static str)>,
        calls: Vec<String>,
    }
    impl ProviderHttp for Http {
        fn cancelled(&self) -> bool {
            false
        }
        fn request(&mut self, request: &Request) -> Result<super::super::Response, &'static str> {
            let (expected, status, body) = self
                .replies
                .pop_front()
                .expect("Unexpected fixture request");
            assert!(
                request.url.contains(expected),
                "Wrong operator request stage"
            );
            self.calls.push(request.url.clone());
            Ok(super::super::Response {
                status,
                body: body.as_bytes().to_vec(),
            })
        }
    }
    fn profile(id: &str, source: &str) -> ProviderProfile {
        ProviderProfile {
            id: "operator-fixture".into(),
            name: "Fixture".into(),
            kind: ProviderKind::Operator(OperatorId::from_id(id).unwrap()),
            source: source.into(),
            source_alias: String::new(),
            enabled: true,
        }
    }
    #[test]
    fn native_operator_identities_match_core_and_keep_real_serialized_provider_names() {
        assert_eq!(OperatorId::ALL.len(), core::OperatorId::ALL.len());
        for id in OperatorId::ALL {
            let canonical = core::OperatorId::from_id(id.canonical_id()).unwrap();
            assert_eq!(id.name(), canonical.name());
            assert_eq!(id.driver_kind(), canonical.driver_kind());
            let kind = ProviderKind::Operator(*id);
            let encoded = serde_json::to_string(&kind).unwrap();
            assert_eq!(encoded, serde_json::to_string(id.canonical_id()).unwrap());
            assert_eq!(
                serde_json::from_str::<ProviderKind>(&encoded).unwrap(),
                kind
            );
        }
    }
    #[test]
    fn operator_api_and_playlist_fallback_run_actual_canonical_plans() {
        let p = profile(
            "great",
            r#"{"server":"https://service.invalid","user":"fixture","pass":"fixture"}"#,
        );
        let mut http=Http{calls:Vec::new(),replies:vec![("player_api.php",200,r#"{"categories":[{"category_id":"1","category_name":"News"}],"live_streams":[{"stream_id":42,"name":"Fixture","category_id":"1","stream_icon":""}]}"#)].into()};
        let mut rows = Vec::new();
        visit(
            &p,
            OperatorId::from_id("great").unwrap(),
            &mut http,
            &mut |batch| {
                rows.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Fixture");
        assert_eq!(rows[0].category, "News");
        assert!(
            matches!(&rows[0].media,MediaLocator::Http{url} if url.contains("/live/fixture/fixture/42.m3u8"))
        );
        let mut fallback = Http {
            calls: Vec::new(),
            replies: vec![
                ("player_api.php", 503, ""),
                (
                    "get.php",
                    200,
                    "#EXTM3U\n#EXTINF:-1,From fallback\nhttp://media.invalid/fallback.ts\n",
                ),
            ]
            .into(),
        };
        let mut rows = Vec::new();
        visit(
            &p,
            OperatorId::from_id("great").unwrap(),
            &mut fallback,
            &mut |batch| {
                rows.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rows[0].title, "From fallback");
        assert_eq!(fallback.calls.len(), 2);
    }
    #[test]
    fn oneott_account_token_is_carried_to_playlist_then_native_catalogue() {
        let p = profile("1ott", r#"{"username":"1234567890","password":"12345678"}"#);
        let mut http=Http{calls:Vec::new(),replies:vec![("list.1ott.net",200,r#"{"token":"fresh-token"}"#),("fresh-token",200,"#EXTM3U\n#EXTINF:-1 tvg-id=\"42\" group-title=\"News\",Fixture\nhttp://media.invalid/42.ts\n")].into()};
        let mut rows = Vec::new();
        visit(
            &p,
            OperatorId::from_id("1ott").unwrap(),
            &mut http,
            &mut |batch| {
                rows.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(http.calls.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider_id.as_deref(), Some("42"));
        assert_eq!(rows[0].category, "News");
    }
}

fn encode(value: &str) -> String {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push('%');
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 15) as usize] as char);
        }
    }
    output
}
pub(super) fn guide(
    profile: &ProviderProfile,
    id: OperatorId,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    if entry.live.is_none() || entry.provider_id_field.is_some() {
        return Err("This item has no live programme guide");
    }
    let config = ott::operators::Config::parse(id, &profile.source)
        .map_err(|_| "Invalid operator profile")?;
    if matches!(id.driver_kind(), "operator" | "xtream-fallback") {
        let source = if !config.playlist.is_empty() {
            config.playlist.clone()
        } else {
            let cfg = core::GenericConfig {
                m3u: String::new(),
                server: config.server.clone(),
                user: config.username.clone(),
                pass: config.password.clone(),
            };
            core::generic_plan(id.canonical_id(), &cfg, None, true)
                .map_err(|_| "Invalid operator guide source")?
                .request
                .ok_or("No operator guide source")?
        };
        return super::live::m3u_guide(&as_m3u(profile, source), entry, request, http);
    }
    let key = entry
        .provider_id
        .as_deref()
        .ok_or("Missing operator channel identity")?;
    let current = matches!(request.query, ott::guide::Query::CurrentNext { .. });
    if id.driver_kind() == "catalog" {
        let kind = match id.canonical_id() {
            "itv" => CatalogProfile::Itv,
            "ottclub" => CatalogProfile::Ottclub,
            "shura" => CatalogProfile::Shura,
            _ => return Err("Unsupported operator guide"),
        };
        let rec = entry
            .live
            .as_ref()
            .and_then(|live| live.catchup.as_ref())
            .map_or(0, |source| source.retention_seconds / 3600);
        let mut replies = Vec::new();
        for _ in 0..=4 {
            let plan = core::guide_plan(kind, current, &replies, rec)
                .map_err(|_| "Invalid operator guide response")?;
            let Some(phase) = plan.phase else {
                return super::edem::json_guide(&plan.result, key, entry, request);
            };
            let route = if id.canonical_id() == "itv" && current {
                core::GuidePhase::Current
            } else {
                match phase.as_str() {
                    "all" => core::GuidePhase::All,
                    "week" => core::GuidePhase::Week,
                    "current" => core::GuidePhase::Current,
                    "archive" => core::GuidePhase::Archive,
                    _ => return Err("Unsupported operator guide phase"),
                }
            };
            let url=core::guide_url(kind,key,&json!({"base":"http://api.01cdn.wf/","next":false,"rec":rec,"server":config.server}),route).map_err(|_|"Invalid operator guide request")?;
            let data = parse_json(&response(http, url)?)?;
            replies.push(core::GuideReply {
                phase,
                data,
                failed: false,
            });
        }
        return Err("Operator guide phase limit reached");
    }
    let guide_id = entry
        .live
        .as_ref()
        .and_then(|live| live.guide_id.as_deref())
        .unwrap_or("");
    let url = match id.canonical_id() {
        "tvteam" => format!("http://tvteam.eu/{}.json", encode(key)),
        "1ott" => {
            if guide_id.is_empty() {
                return Err("Channel has no guide identifier");
            }
            format!(
                "http://epg.drm-play.com/{}.json",
                encode(&format!("propg.net/epg/{guide_id}"))
            )
        }
        "only4" | "shara-tv" => {
            if guide_id.is_empty() {
                return Err("Channel has no guide identifier");
            }
            format!(
                "http://epg.drm-play.com/{}/epg/{}.json",
                id.canonical_id(),
                encode(guide_id)
            )
        }
        "antifriz" => {
            if guide_id.is_empty() {
                return Err("Channel has no guide identifier");
            }
            if current {
                format!(
                    "http://protected-api.com/epg/current/{}?num=1",
                    encode(guide_id)
                )
            } else {
                format!("http://protected-api.com/epg/{}?date=", encode(guide_id))
            }
        }
        "kb-team" => {
            let live = entry.live.as_ref().ok_or("Channel has no guide")?;
            let name = if live.tvg_name.is_empty() {
                &entry.title
            } else {
                &live.tvg_name
            };
            let mut record = json!({"n":name});
            if !live.guide_feed.is_empty() {
                record["u"] = json!(live.guide_feed);
                if !guide_id.is_empty() {
                    record["e"] = json!(guide_id);
                }
            }
            let data = json!({(key):record}).to_string();
            let response = http.request(&Request {
                url: "https://epg.drm-play.com/m3u/gelist.php".into(),
                headers: vec![
                    "User-Agent: Mozilla/5.0".into(),
                    "Content-Type: application/x-www-form-urlencoded".into(),
                ],
                body: Some(format!("list={}", encode(&data)).into_bytes()),
            })?;
            if http.cancelled() {
                return Err("Provider request cancelled");
            }
            if !(200..300).contains(&response.status)
                || response.body.len() > ott::MAX_METADATA_BYTES
            {
                return Err("Operator guide lookup failed");
            }
            let mapping = parse_json(&response.body)?;
            let reference = mapping[key]
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 8192)
                .ok_or("Channel has no programme guide")?;
            format!("https://epg.drm-play.com/{}.json", encode(reference))
        }
        _ => return Err("Unsupported operator guide"),
    };
    let data = parse_json(&response(http, url)?)?;
    let rows = if id.canonical_id() == "antifriz" {
        &data
    } else {
        &data["epg_data"]
    };
    super::edem::json_guide(
        rows,
        if guide_id.is_empty() { key } else { guide_id },
        entry,
        request,
    )
}

#[cfg(test)]
mod guide_flow_tests {
    use super::*;
    use std::collections::VecDeque;
    struct Http {
        replies: VecDeque<(&'static str, &'static str)>,
        calls: usize,
    }
    impl ProviderHttp for Http {
        fn cancelled(&self) -> bool {
            false
        }
        fn request(&mut self, request: &Request) -> Result<super::super::Response, &'static str> {
            let (path, body) = self.replies.pop_front().expect("Unexpected fixture phase");
            assert!(request.url.contains(path), "Incorrect guide phase route");
            self.calls += 1;
            Ok(super::super::Response {
                status: 200,
                body: body.as_bytes().to_vec(),
            })
        }
    }
    fn entry(profile: &ProviderProfile) -> CatalogEntry {
        let source = as_m3u(profile, "https://playlist.invalid/list.m3u".into());
        let mut row = ott::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Channel\nhttps://media.invalid/live.ts\n",
            &source,
        )
        .unwrap()
        .entries
        .remove(0);
        row.provider_id = Some("42".into());
        row
    }
    #[test]
    fn kbc_hash_matches_current_foss_utf8_implementation_including_unicode() {
        assert_eq!(murmur3("http://media.invalid/42.ts", 10), 159584532);
        assert_eq!(
            murmur3("http://media.invalid/Мультфильм😀.ts", 10),
            1534169757
        );
        assert_eq!(murmur3("", 10), 0);
    }
    #[test]
    fn shura_actual_week_and_archive_requests_preserve_both_response_sets() {
        let id = OperatorId::from_id("shura").unwrap();
        let profile = ProviderProfile {
            id: "shura-1".into(),
            name: id.name().into(),
            kind: ProviderKind::Operator(id),
            source: r#"{"key":"fixture","server":"1"}"#.into(),
            source_alias: String::new(),
            enabled: true,
        };
        let mut row = entry(&profile);
        row.live.as_mut().unwrap().catchup = Some(ott::archive::CatchupSource {
            mode: ott::archive::CatchupMode::Shift,
            source: String::new(),
            retention_seconds: 86400,
            correction_seconds: 0,
        });
        let request = ott::guide::Request {
            context: ott::guide::Context {
                generation: 1,
                profile_id: profile.id.clone(),
                entry_id: row.id.clone(),
            },
            query: ott::guide::Query::Day { start: 0, end: 300 },
        };
        let mut http=Http{calls:0,replies:vec![("/epg/week.jsonp","cb([{\"start_time\":100,\"duration\":100,\"name\":\"Current\",\"text\":\"Full synopsis\"}]);"),("/epg/archive.jsonp","cb([{\"start_time\":0,\"duration\":100,\"name\":\"Earlier\"}]);")].into()};
        let result = guide(&profile, id, &row, &request, &mut http).unwrap();
        assert_eq!(http.calls, 2);
        assert_eq!(result.programmes.len(), 2);
        assert_eq!(result.programmes[0].title, "Earlier");
        assert_eq!(result.programmes[1].description, "Full synopsis");
    }
    #[test]
    fn operator_http_vod_without_duration_is_not_admitted_as_a_live_guide() {
        let id = OperatorId::from_id("great").unwrap();
        let profile = ProviderProfile {
            id: "great-1".into(),
            name: id.name().into(),
            kind: ProviderKind::Operator(id),
            source: r#"{"m3u":"https://playlist.invalid/list.m3u"}"#.into(),
            source_alias: String::new(),
            enabled: true,
        };
        let mut row = entry(&profile);
        row.live = None;
        row.duration_ms = None;
        row.provider_id_field = Some("id".into());
        let request = ott::guide::Request {
            context: ott::guide::Context {
                generation: 1,
                profile_id: profile.id.clone(),
                entry_id: row.id.clone(),
            },
            query: ott::guide::Query::CurrentNext { now: 100 },
        };
        let mut http = Http {
            replies: VecDeque::new(),
            calls: 0,
        };
        assert!(guide(&profile, id, &row, &request, &mut http).is_err());
        assert_eq!(http.calls, 0);
    }
    #[test]
    fn kbc_selected_guide_lookup_uses_attested_feed_and_channel_identity() {
        let id = OperatorId::from_id("kb-team").unwrap();
        let profile = ProviderProfile {
            id: "kbc-1".into(),
            name: id.name().into(),
            kind: ProviderKind::Operator(id),
            source: r#"{"mac":"001122334455","list":0}"#.into(),
            source_alias: String::new(),
            enabled: true,
        };
        let mut row = entry(&profile);
        let live = row.live.as_mut().unwrap();
        live.guide_id = Some("777".into());
        live.guide_feed = "custom-feed".into();
        live.tvg_name = "Guide name".into();
        let request = ott::guide::Request {
            context: ott::guide::Context {
                generation: 1,
                profile_id: profile.id.clone(),
                entry_id: row.id.clone(),
            },
            query: ott::guide::Query::CurrentNext { now: 150 },
        };
        let mut http = Http {
            calls: 0,
            replies: vec![
                ("gelist.php", r#"{"42":"custom/epg/42"}"#),
                (
                    "custom%2Fepg%2F42.json",
                    r#"{"epg_data":[{"time":100,"time_to":200,"name":"Fixture"}]}"#,
                ),
            ]
            .into(),
        };
        assert_eq!(
            guide(&profile, id, &row, &request, &mut http)
                .unwrap()
                .programmes[0]
                .title,
            "Fixture"
        );
        assert_eq!(http.calls, 2);
    }
}

pub(super) fn metadata(
    profile: &ProviderProfile,
    id: OperatorId,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<Value, &'static str> {
    profile.validate().map_err(|_| "Invalid operator profile")?;
    let mut poster = entry.poster.clone();
    if id.canonical_id() == "kb-team" && entry.live.is_some() && poster.is_empty() {
        let key = entry
            .provider_id
            .as_deref()
            .ok_or("Missing operator channel identity")?;
        let feed = &entry
            .live
            .as_ref()
            .ok_or("Missing operator guide metadata")?
            .guide_feed;
        let name = if feed.is_empty() {
            entry.title.clone()
        } else {
            format!("{}|{feed}", entry.title)
        };
        let request = json!({(key):name}).to_string();
        let response = http.request(&Request {
            url: "https://epg.drm-play.com/m3u/geicons.php".into(),
            headers: vec![
                "User-Agent: Mozilla/5.0".into(),
                "Content-Type: application/x-www-form-urlencoded".into(),
            ],
            body: Some(format!("list={}", encode(&request)).into_bytes()),
        })?;
        if http.cancelled() {
            return Err("Provider request cancelled");
        }
        if !(200..300).contains(&response.status) || response.body.len() > ott::MAX_METADATA_BYTES {
            return Err("Operator logo lookup failed");
        }
        let icons = parse_json(&response.body)?;
        if let Some(url) = icons[key].as_str() {
            poster = ott::http_url(url).unwrap_or_default();
        }
    }
    Ok(
        json!({"description":entry.description,"poster":poster,"durationSeconds":entry.duration_ms.map(|ms|ms/1000)}),
    )
}

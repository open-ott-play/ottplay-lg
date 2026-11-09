//! Classic MAG/Ministra protocol, with no network or process-global state.
use super::{
    http_url, identity, resolve_http_url, text, Catalog, CatalogEntry, CatalogError, MediaLocator,
    ProviderProfile, MAX_CATALOG_ENTRIES, MAX_REQUEST_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn model() -> String {
    "MAG250".into()
}
fn language() -> String {
    "en".into()
}
fn timezone() -> String {
    "UTC".into()
}
fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub portal: String,
    pub mac: String,
    #[serde(default = "model")]
    pub stb_type: String,
    #[serde(default = "language")]
    pub language: String,
    #[serde(default = "timezone")]
    pub timezone: String,
    #[serde(default = "yes")]
    pub include_vod: bool,
    #[serde(default)]
    pub sn: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_id2: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub ver: String,
    #[serde(default)]
    pub image_version: String,
    #[serde(default)]
    pub hw_version: String,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StalkerConfig(<private>)")
    }
}
impl Config {
    pub fn parse(input: &str) -> Result<Self, CatalogError> {
        if input.len() > MAX_REQUEST_BYTES {
            return Err(CatalogError::LimitExceeded);
        }
        let mut config: Self =
            serde_json::from_str(input).map_err(|_| CatalogError::InvalidProfile)?;
        config.mac = config.mac.trim().to_string();
        if config.mac.len() != 17
            || !config.mac.bytes().enumerate().all(|(i, b)| {
                if i % 3 == 2 {
                    b == b':'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
            || config.stb_type.is_empty()
            || config.stb_type.len() > 40
            || !config
                .stb_type
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || [
                &config.language,
                &config.timezone,
                &config.sn,
                &config.device_id,
                &config.device_id2,
                &config.signature,
                &config.ver,
                &config.image_version,
                &config.hw_version,
            ]
            .iter()
            .any(|s| s.len() > 2048 || s.chars().any(char::is_control))
        {
            return Err(CatalogError::InvalidProfile);
        }
        config.portal = endpoint(&config.portal)?;
        if config.dialect() == Dialect::Mag {
            config.mac.make_ascii_uppercase();
        }
        Ok(config)
    }
    pub fn dialect(&self) -> Dialect {
        if self
            .portal
            .trim_end_matches('/')
            .to_ascii_lowercase()
            .ends_with("/api")
        {
            Dialect::JsonRpc
        } else {
            Dialect::Mag
        }
    }
    pub fn url(&self, kind: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut fields = vec![
            ("type", kind.to_string()),
            ("action", action.to_string()),
            ("JsHttpRequest", "1-xml".into()),
        ];
        fields.extend_from_slice(params);
        format!(
            "{}?{}",
            self.portal,
            fields
                .iter()
                .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
                .collect::<Vec<_>>()
                .join("&")
        )
    }
    pub fn headers(&self, token: Option<&str>) -> Vec<String> {
        let base = ["/server/load.php", "/portal.php", "/load.php"]
            .iter()
            .find_map(|suffix| self.portal.strip_suffix(*suffix))
            .unwrap_or_else(|| {
                self.portal
                    .rsplit_once('/')
                    .map_or(self.portal.as_str(), |(base, _)| base)
            });
        let mut headers = vec![
            format!(
                "Cookie: mac={}; stb_lang={}; timezone={}",
                percent(&self.mac),
                percent(&self.language),
                percent(&self.timezone)
            ),
            format!("X-User-Agent: Model: {}; Link: Ethernet", self.stb_type),
            format!("Referer: {base}/c/"),
            "Accept: application/json".into(),
        ];
        if let Some(token) = token {
            headers.push(format!("Authorization: Bearer {token}"));
        }
        headers
    }
    pub fn profile_params(&self) -> Vec<(&'static str, String)> {
        let mut result = vec![
            ("hd", "1".into()),
            ("auth_second_step", "0".into()),
            ("not_valid_token", "0".into()),
            ("stb_type", self.stb_type.clone()),
        ];
        for (key, value) in [
            ("sn", &self.sn),
            ("device_id", &self.device_id),
            ("device_id2", &self.device_id2),
            ("signature", &self.signature),
            ("ver", &self.ver),
            ("image_version", &self.image_version),
            ("hw_version", &self.hw_version),
        ] {
            if !value.is_empty() {
                result.push((key, value.clone()));
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Mag,
    JsonRpc,
}

pub fn endpoint(input: &str) -> Result<String, CatalogError> {
    if input.contains(['?', '#']) {
        return Err(CatalogError::InvalidProfile);
    }
    let canonical = http_url(input).ok_or(CatalogError::InvalidProfile)?;
    let clean = canonical.trim_end_matches('/');
    let lower = clean.to_ascii_lowercase();
    if lower.ends_with("/api") {
        return Ok(format!("{clean}/"));
    }
    let endpoint = if lower.ends_with(".php") {
        clean.to_string()
    } else if lower.ends_with("/c/index.html") {
        format!("{}/server/load.php", &clean[..clean.len() - 13])
    } else if lower.ends_with("/c") {
        format!("{}/server/load.php", &clean[..clean.len() - 2])
    } else {
        format!("{clean}/server/load.php")
    };
    http_url(&endpoint).ok_or(CatalogError::InvalidProfile)
}

pub fn percent(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
pub fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn flag(value: &Value) -> bool {
    value == true || value == 1 || value == "1"
}
pub fn unwrap(value: &Value) -> Result<&Value, &'static str> {
    let data = value
        .get("js")
        .or_else(|| value.get("result"))
        .ok_or("Invalid Stalker response envelope")?;
    if data.is_null() || data == false {
        return Err("Stalker returned an empty response");
    }
    if flag(&data["not_valid_token"])
        || data
            .get("error")
            .is_some_and(|v| !v.is_null() && v != false && v != 0 && v != "")
    {
        return Err("Stalker authorization failed");
    }
    Ok(data)
}
pub fn token(data: &Value) -> Result<String, &'static str> {
    let token = data["token"].as_str().ok_or("Invalid Stalker handshake")?;
    let plain = token.trim_end_matches('=');
    if plain.is_empty()
        || token.len() > 2048
        || token.len() - plain.len() > 2
        || !plain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~+/-".contains(&b))
    {
        return Err("Invalid Stalker handshake");
    }
    Ok(token.into())
}
pub fn validate_profile(data: &Value) -> Result<(), &'static str> {
    if !data.is_object()
        || (!data.get("id").is_some_and(|v| !scalar(v).is_empty()) && data.get("status").is_none())
        || data.get("status").is_some_and(|v| scalar(v) != "0")
        || flag(&data["blocked"])
    {
        return Err("Stalker profile is not authorized");
    }
    if flag(&data["auth_second_step"]) {
        return Err("Stalker requires additional authentication");
    }
    Ok(())
}
pub fn categories(data: &Value) -> Result<BTreeMap<String, String>, &'static str> {
    let rows = data.as_array().ok_or("Invalid Stalker categories")?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err("Stalker categories exceed limit");
    }
    Ok(rows
        .iter()
        .filter_map(|row| {
            let id = scalar(&row["id"]);
            if id.is_empty() {
                None
            } else {
                Some((
                    id,
                    text(
                        row["title"]
                            .as_str()
                            .or_else(|| row["name"].as_str())
                            .unwrap_or("Other"),
                        1024,
                    ),
                ))
            }
        })
        .collect())
}

/// Tracks unique records rather than trusting page number or changing server totals.
#[derive(Default)]
pub struct Pages {
    seen: BTreeSet<String>,
    expected: Option<usize>,
    pages: usize,
    pub done: bool,
}
impl Pages {
    pub fn accept(&mut self, data: &Value) -> Result<Vec<Value>, &'static str> {
        if self.done {
            return Err("Stalker page received after completion");
        }
        let rows = data
            .as_array()
            .or_else(|| data["data"].as_array())
            .ok_or("Invalid Stalker page")?;
        if let Some(total) = data.get("total_items") {
            let total = scalar(total)
                .parse::<usize>()
                .map_err(|_| "Invalid Stalker total")?;
            if total > MAX_CATALOG_ENTRIES {
                return Err("Stalker catalogue exceeds limit");
            }
            if self.expected.is_some_and(|previous| previous != total) {
                return Err("Stalker catalogue changed during pagination");
            }
            self.expected = Some(total);
        }
        self.pages += 1;
        if self.pages > 1000 {
            return Err("Stalker pagination exceeds limit");
        }
        let before = self.seen.len();
        let mut fresh = Vec::new();
        for row in rows {
            let id = scalar(&row["id"]);
            let id = if id.is_empty() {
                scalar(&row["ch_id"])
            } else {
                id
            };
            if id.is_empty() || id.len() > 1024 {
                return Err("Stalker row has no valid identity");
            }
            if self.seen.insert(id) {
                fresh.push(row.clone());
            }
        }
        if self.seen.len() > MAX_CATALOG_ENTRIES
            || self.expected.is_some_and(|n| self.seen.len() > n)
        {
            return Err("Stalker page exceeds declared total");
        }
        if rows.is_empty() && self.expected.is_some_and(|n| self.seen.len() < n) {
            return Err("Stalker catalogue is incomplete");
        }
        if !rows.is_empty() && before == self.seen.len() {
            return Err("Stalker repeated a page");
        }
        let page_size = data
            .get("max_page_items")
            .and_then(|v| scalar(v).parse::<usize>().ok());
        self.done = rows.is_empty()
            || self.expected == Some(self.seen.len())
            || (self.expected.is_none()
                && (data.is_array() || page_size.is_some_and(|n| rows.len() < n)));
        Ok(fresh)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Folder {
    pub category: String,
    pub name: String,
    pub movie: String,
    pub season: String,
    pub episode: String,
    pub depth: usize,
}
pub fn rows(
    profile: &ProviderProfile,
    config: &Config,
    kind: &str,
    group_names: &BTreeMap<String, String>,
    parent: &Folder,
    input: &[Value],
) -> Result<(Catalog, Vec<Folder>), &'static str> {
    let mut result = Catalog::default();
    let mut folders = Vec::new();
    for row in input {
        let id = scalar(&row["id"]);
        let id = if id.is_empty() {
            scalar(&row["ch_id"])
        } else {
            id
        };
        let title = text(
            row["name"]
                .as_str()
                .or_else(|| row["title"].as_str())
                .unwrap_or(&id),
            1024,
        );
        if kind == "vod"
            && (flag(&row["is_series"])
                || flag(&row["is_season"])
                || flag(&row["is_episode"])
                || scalar(&row["has_files"]).parse::<u64>().unwrap_or(0) > 0)
        {
            if parent.depth >= 12 {
                return Err("Stalker folder depth exceeds limit");
            }
            let mut folder = parent.clone();
            folder.depth += 1;
            folder.name = if parent.name.is_empty() {
                title
            } else {
                format!("{} / {}", parent.name, title)
            };
            if flag(&row["is_episode"]) {
                folder.episode = id;
            } else if flag(&row["is_season"]) {
                folder.season = id;
            } else {
                folder.movie = id;
            }
            folders.push(folder);
            continue;
        }
        let cmd = scalar(&row["cmd"]);
        let cmd = if cmd.is_empty() {
            scalar(&row["url"])
        } else {
            cmd
        };
        if cmd.is_empty() || cmd.len() > 8192 || cmd.chars().any(char::is_control) {
            // Portals may mix section markers and unsupported entries with channels.
            continue;
        }
        let category = group_names
            .get(&scalar(&row["tv_genre_id"]))
            .or_else(|| group_names.get(&scalar(&row["category_id"])))
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if parent.name.is_empty() {
                    "Other".into()
                } else {
                    parent.name.clone()
                }
            });
        let poster = row["logo"]
            .as_str()
            .or_else(|| row["screenshot_uri"].as_str())
            .and_then(|s| resolve_http_url(&config.portal, s))
            .unwrap_or_default();
        let series = scalar(&row["series_number"]);
        let series = if series.is_empty() {
            "0".into()
        } else {
            series
        };
        let stable = format!(
            "stalker:{kind}:{}:{}:{}:{id}",
            parent.movie, parent.season, parent.episode
        );
        result.entries.push(CatalogEntry {
            id: identity(&profile.id, &stable),
            title,
            category,
            description: text(row["description"].as_str().unwrap_or(""), 128 * 1024),
            synopsis: None,
            poster,
            media: MediaLocator::Stalker {
                cmd,
                stream_kind: kind.into(),
                series,
            },
            duration_ms: scalar(&row["time"])
                .parse::<u64>()
                .ok()
                .and_then(|n| n.checked_mul(60_000)),
            provider_id: Some(id.clone()),
            provider_id_field: Some("id".into()),
            live: (kind == "itv").then(|| super::guide::ChannelMetadata {
                tvg_name: String::new(), guide_id: Some(id.clone()), guide_feed: String::new(), guide_shift_seconds: 0,
                catchup: scalar(&row["tv_archive_duration"]).parse::<u64>().ok()
                    .filter(|hours| (1..=8784).contains(hours))
                    .map(|hours| super::archive::CatchupSource { mode: super::archive::CatchupMode::Provider,
                        source: "stalker".into(), retention_seconds: hours * 3600, correction_seconds: 0 }),
            }),
        });
    }
    Ok((result, folders))
}

pub fn playback_url(config: &Config, data: &Value) -> Result<String, &'static str> {
    let command = data["cmd"]
        .as_str()
        .or_else(|| data.as_str())
        .ok_or("Stalker returned no playback command")?
        .trim();
    let mut parts = command.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let raw = if matches!(
        first.to_ascii_lowercase().as_str(),
        "ffmpeg" | "ffrt" | "auto"
    ) {
        parts.next().unwrap_or("").trim()
    } else {
        command
    };
    if raw.is_empty()
        || raw.chars().any(char::is_whitespace)
        || !(raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with('/'))
    {
        return Err("Unsupported Stalker playback command");
    }
    let url = resolve_http_url(&config.portal, raw).ok_or("Invalid Stalker playback address")?;
    let origin =
        plx_net::net::origin::Origin::parse(&url).ok_or("Invalid Stalker playback address")?;
    let host = origin.host().trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        return Err("Stalker returned a loopback playback address");
    }
    Ok(url)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Programme {
    pub channel_id: String,
    pub title: String,
    pub description: String,
    pub start: i64,
    pub stop: i64,
}
pub fn short_epg(channel: &str, data: &Value) -> Result<Vec<Programme>, &'static str> {
    let rows = data.as_array().ok_or("Invalid Stalker programme list")?;
    let mut result = Vec::new();
    programme_rows(channel, rows, &mut result)?;
    Ok(result)
}
pub fn epg(data: &Value) -> Result<Vec<Programme>, &'static str> {
    let channels = data
        .get("data")
        .unwrap_or(data)
        .as_object()
        .ok_or("Invalid Stalker programme response")?;
    let mut result = Vec::new();
    for (channel, list) in channels {
        let list = list.as_array().ok_or("Invalid Stalker programme list")?;
        programme_rows(channel, list, &mut result)?;
    }
    Ok(result)
}
fn programme_rows(
    channel: &str,
    list: &[Value],
    result: &mut Vec<Programme>,
) -> Result<(), &'static str> {
    for row in list {
        if result.len() >= MAX_CATALOG_ENTRIES {
            return Err("Stalker programme count exceeds limit");
        }
        let start = scalar(&row["start_timestamp"])
            .parse::<i64>()
            .or_else(|_| scalar(&row["timestamp"]).parse())
            .map_err(|_| "Invalid Stalker programme time")?;
        let stop = scalar(&row["stop_timestamp"])
            .parse::<i64>()
            .or_else(|_| scalar(&row["end_timestamp"]).parse())
            .ok()
            .or_else(|| {
                scalar(&row["duration"])
                    .parse::<i64>()
                    .ok()
                    .and_then(|n| n.checked_mul(60))
                    .and_then(|n| start.checked_add(n))
            })
            .ok_or("Invalid Stalker programme duration")?;
        if start < 0 || stop <= start {
            return Err("Invalid Stalker programme interval");
        }
        result.push(Programme {
            channel_id: text(channel, 1024),
            title: text(
                row["name"]
                    .as_str()
                    .or_else(|| row["title"].as_str())
                    .unwrap_or(""),
                1024,
            ),
            description: text(
                row["descr"]
                    .as_str()
                    .or_else(|| row["description"].as_str())
                    .unwrap_or(""),
                128 * 1024,
            ),
            start,
            stop,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> Config {
        Config::parse(
            r#"{"portal":"https://portal.invalid/stalker_portal/c/","mac":"00:1a:79:aa:bb:cc"}"#,
        )
        .unwrap()
    }
    #[test]
    fn config_endpoint_cookie_and_private_debug() {
        let c = config();
        assert_eq!(
            c.portal,
            "https://portal.invalid:443/stalker_portal/server/load.php"
        );
        assert!(c.headers(None)[0].contains("00%3A1A%3A79%3AAA%3ABB%3ACC"));
        assert!(c
            .headers(None)
            .iter()
            .any(|header| header == "Referer: https://portal.invalid:443/stalker_portal/c/"));
        assert!(!format!("{c:?}").contains("portal.invalid"));
        assert_eq!(
            endpoint("https://p.invalid/c/index.html").unwrap(),
            "https://p.invalid:443/server/load.php"
        );
        assert_eq!(
            endpoint("https://p.invalid/stalker_portal/api/").unwrap(),
            "https://p.invalid:443/stalker_portal/api/"
        );
        assert!(
            Config::parse(r#"{"portal":"https://p.invalid/?x=y","mac":"00:1a:79:aa:bb:cc"}"#)
                .is_err()
        );
        assert!(
            Config::parse(r#"{"portal":"https://p.invalid/c","mac":"00:1a:79:aa:bb:\r\n"}"#)
                .is_err()
        );
    }
    #[test]
    fn authentication_is_not_success_without_evidence() {
        assert!(validate_profile(&json!({})).is_err());
        assert!(validate_profile(&json!({"id":1,"status":1})).is_err());
        assert!(validate_profile(&json!({"id":1,"status":0,"auth_second_step":1})).is_err());
        assert!(validate_profile(&json!({"id":1,"status":"0"})).is_ok());
        assert!(token(&json!({"token":"x\r\nHeader:y"})).is_err());
        assert!(unwrap(&json!({"js":{"not_valid_token":true}})).is_err());
    }
    #[test]
    fn pages_reject_repeat_truncation_and_changed_totals() {
        let first = json!({"total_items":2,"data":[{"id":1}]});
        let mut p = Pages::default();
        p.accept(&first).unwrap();
        assert!(!p.done);
        assert!(p.accept(&first).is_err());
        let mut p = Pages::default();
        p.accept(&first).unwrap();
        assert!(p.accept(&json!({"total_items":2,"data":[]})).is_err());
        let mut p = Pages::default();
        p.accept(&first).unwrap();
        assert!(p
            .accept(&json!({"total_items":3,"data":[{"id":2}]}))
            .is_err());
    }
    #[test]
    fn commands_are_never_shell_interpreted() {
        assert_eq!(
            playback_url(
                &config(),
                &json!({"cmd":"ffmpeg https://media.invalid/live.m3u8?token=x"})
            )
            .unwrap(),
            "https://media.invalid:443/live.m3u8?token=x"
        );
        for cmd in [
            "ffmpeg -i https://media.invalid/a",
            "ffmpeg http://localhost/ch/1",
            "ffmpeg http://127.0.0.2/ch/1",
            "ffmpeg http://[::1]/ch/1",
            "ffmpeg http://localhost./ch/1",
            "not-a-media-command",
            "file:///etc/passwd",
            "https://media.invalid/a ; touch /tmp/x",
        ] {
            assert!(
                playback_url(&config(), &json!({"cmd":cmd})).is_err(),
                "{cmd}"
            );
        }
    }
    #[test]
    fn epg_intervals_and_text_are_retained() {
        let rows=epg(&json!({"data":{"12":[{"name":"News","timestamp":1700000000,"duration":30,"descr":"Full description"}]}})).unwrap();
        assert_eq!(rows[0].stop - rows[0].start, 1800);
        assert_eq!(rows[0].description, "Full description");
    }
    #[test]
    fn unsupported_markers_do_not_hide_valid_neighbours() {
        let profile = ProviderProfile {
            id: "fixture".into(),
            name: "Fixture".into(),
            kind: super::super::ProviderKind::Stalker,
            source: String::new(),
            source_alias: String::new(),
            enabled: true,
        };
        let input = vec![
            json!({"id":"1","name":"Section"}),
            json!({"id":"2","name":"Programme","cmd":"ffmpeg /live/2"}),
        ];
        let parsed = rows(
            &profile,
            &config(),
            "itv",
            &BTreeMap::new(),
            &Folder::default(),
            &input,
        )
        .unwrap();
        assert_eq!(parsed.0.entries.len(), 1);
        assert_eq!(parsed.0.entries[0].title, "Programme");
    }
}

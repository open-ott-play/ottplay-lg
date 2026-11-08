//! Xtream wire data and credential-independent identities. No transport or persistence.
use super::{
    http_url, identity, resolve_http_url, text, Catalog, CatalogEntry, CatalogError, MediaLocator,
    ProviderProfile, MAX_CATALOG_ENTRIES, MAX_METADATA_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: String,
    pub username: String,
    pub password: String,
    #[serde(default = "output_default")]
    pub output: String,
}
fn output_default() -> String {
    "m3u8".into()
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("XtreamConfig(<private>)")
    }
}
impl Config {
    pub fn parse(source: &str) -> Result<Self, CatalogError> {
        if source.len() > 16384 {
            return Err(CatalogError::InvalidProfile);
        }
        let mut c: Self = serde_json::from_str(source).map_err(|_| CatalogError::InvalidProfile)?;
        if c.username.is_empty()
            || c.password.is_empty()
            || c.username.len() > 1024
            || c.password.len() > 1024
            || c.username
                .chars()
                .chain(c.password.chars())
                .any(char::is_control)
            || !matches!(c.output.as_str(), "m3u8" | "ts")
        {
            return Err(CatalogError::InvalidProfile);
        }
        if c.server.contains(['?', '#']) {
            return Err(CatalogError::InvalidProfile);
        }
        c.server = http_url(c.server.trim_end_matches('/')).ok_or(CatalogError::InvalidProfile)?;
        c.server = c.server.trim_end_matches('/').to_owned();
        for name in ["/player_api.php", "/get.php", "/xmltv.php"] {
            if c.server.ends_with(name) {
                c.server.truncate(c.server.len() - name.len());
                break;
            }
        }
        Ok(c)
    }
    pub fn api_url(&self, action: &str, params: &[(&str, &str)]) -> String {
        let mut result = format!(
            "{}/player_api.php?username={}&password={}",
            self.server,
            component(&self.username),
            component(&self.password)
        );
        if !action.is_empty() {
            result.push_str("&action=");
            result.push_str(&component(action));
        }
        for (k, v) in params {
            result.push('&');
            result.push_str(&component(k));
            result.push('=');
            result.push_str(&component(v));
        }
        result
    }
    pub fn stream_url(
        &self,
        kind: &str,
        id: &str,
        extension: &str,
    ) -> Result<String, CatalogError> {
        if !matches!(kind, "live" | "movie" | "series")
            || !numeric(id)
            || !valid_extension(extension)
        {
            return Err(CatalogError::InvalidFormat);
        }
        Ok(format!(
            "{}/{}/{}/{}/{}.{}",
            self.server,
            kind,
            component(&self.username),
            component(&self.password),
            id,
            extension
        ))
    }
}
fn component(input: &str) -> String {
    let mut result = String::new();
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            result.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{b:02X}");
        }
    }
    result
}
pub fn numeric(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}
pub fn valid_extension(s: &str) -> bool {
    !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn number(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    numeric(&s).then_some(s)
}
fn label(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn description(v: &Value) -> String {
    text(label(v), MAX_METADATA_BYTES)
}

pub fn validate_account(value: &Value) -> Result<(), CatalogError> {
    let user = value
        .get("user_info")
        .and_then(Value::as_object)
        .ok_or(CatalogError::InvalidFormat)?;
    if !matches!(user.get("auth"),Some(Value::Number(n)) if n.as_u64()==Some(1))
        && user.get("auth").and_then(Value::as_str) != Some("1")
    {
        return Err(CatalogError::InvalidProfile);
    }
    if let Some(status) = user.get("status") {
        if !status
            .as_str()
            .is_some_and(|s| s.is_empty() || s.eq_ignore_ascii_case("active"))
        {
            return Err(CatalogError::InvalidProfile);
        }
    }
    Ok(())
}

pub fn categories(value: &Value) -> Result<BTreeMap<String, String>, CatalogError> {
    let rows = value.as_array().ok_or(CatalogError::InvalidFormat)?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogError::LimitExceeded);
    }
    let mut output = BTreeMap::new();
    for row in rows {
        let Some(id) = number(&row["category_id"]) else {
            continue;
        };
        let name = text(label(&row["category_name"]), 1024);
        output.insert(
            id,
            if name.is_empty() {
                "Other".into()
            } else {
                name
            },
        );
    }
    Ok(output)
}

#[derive(Clone, Debug)]
pub struct Series {
    pub id: String,
    pub name: String,
    pub category: String,
    pub poster: String,
    pub description: String,
}

fn rows(value: &Value) -> Result<&Vec<Value>, CatalogError> {
    let rows = value.as_array().ok_or(CatalogError::InvalidFormat)?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogError::LimitExceeded);
    }
    Ok(rows)
}
fn group(row: &Value, groups: &BTreeMap<String, String>, fallback: &str) -> String {
    number(&row["category_id"])
        .and_then(|id| groups.get(&id).cloned())
        .unwrap_or_else(|| fallback.into())
}
fn image(config: &Config, v: &Value) -> String {
    let s = label(v);
    if s.is_empty() {
        String::new()
    } else {
        resolve_http_url(&format!("{}/", config.server), s).unwrap_or_default()
    }
}
pub fn direct_source(config: &Config, row: &Value) -> Option<String> {
    let url = image(config, &row["direct_source"]);
    (!url.is_empty()).then_some(url)
}
fn title(row: &Value, key: &str, fallback: String) -> String {
    let s = text(label(&row[key]), 1024);
    if s.is_empty() {
        fallback
    } else {
        s
    }
}

/// Standard Xtream APIs return bounded arrays, not arbitrary offset pages. The runner
/// publishes each complete section before requesting the next.
pub fn streams(
    value: &Value,
    profile: &ProviderProfile,
    config: &Config,
    kind: &str,
    groups: &BTreeMap<String, String>,
) -> Result<Catalog, CatalogError> {
    if !matches!(kind, "live" | "movie") {
        return Err(CatalogError::InvalidFormat);
    }
    let mut catalog = Catalog::default();
    let mut seen = BTreeSet::new();
    for row in rows(value)? {
        let Some(id) = number(&row["stream_id"]) else {
            continue;
        };
        let extension = if kind == "live" {
            config.output.clone()
        } else {
            let ext = label(&row["container_extension"]);
            if ext.is_empty() {
                "mp4".into()
            } else {
                ext.to_owned()
            }
        };
        if !valid_extension(&extension) {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        catalog.entries.push(CatalogEntry {
            id: identity(&profile.id, &format!("xtream:{kind}:{id}")),
            title: title(row, "name", format!("Channel {id}")),
            category: group(row, groups, if kind == "live" { "Other" } else { "Movies" }),
            description: description(&row["plot"]),
            poster: image(config, &row["stream_icon"]),
            media: MediaLocator::Xtream {
                stream_id: id.clone(),
                stream_kind: kind.into(),
                extension,
                series_id: None,
                has_direct_source: direct_source(config, row).is_some(),
            },
            duration_ms: None,
            provider_id: Some(id),
            provider_id_field: Some("stream_id".into()), live: None,
        });
    }
    let mut bounded = Catalog::default();
    bounded.merge(catalog)?;
    Ok(bounded)
}

pub fn series(
    value: &Value,
    config: &Config,
    groups: &BTreeMap<String, String>,
) -> Result<Vec<Series>, CatalogError> {
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows(value)? {
        let Some(id) = number(&row["series_id"]) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        output.push(Series {
            id: id.clone(),
            name: title(row, "name", format!("Series {id}")),
            category: group(row, groups, "Series"),
            poster: image(config, &row["cover"]),
            description: description(&row["plot"]),
        });
    }
    Ok(output)
}

/// Flatten both valid API layouts without trusting malformed individual records.
/// Array layouts may contain seasons (arrays) or direct episode objects.
pub fn episode_rows(value: &Value) -> Result<Vec<(u64, &Value)>, CatalogError> {
    let mut out = Vec::new();
    let mut append = |season: u64, row| {
        if out.len() >= MAX_CATALOG_ENTRIES {
            return Err(CatalogError::LimitExceeded);
        }
        out.push((season, row));
        Ok(())
    };
    match &value["episodes"] {
        Value::Object(seasons) => {
            if seasons.len() > MAX_CATALOG_ENTRIES {
                return Err(CatalogError::LimitExceeded);
            }
            for (season, entries) in seasons {
                let Ok(season) = season.parse::<u64>() else {
                    continue;
                };
                let Some(entries) = entries.as_array() else {
                    continue;
                };
                for row in entries {
                    append(season, row)?;
                }
            }
        }
        Value::Array(entries) => {
            if entries.len() > MAX_CATALOG_ENTRIES {
                return Err(CatalogError::LimitExceeded);
            }
            for (season, entry) in entries.iter().enumerate() {
                if let Some(rows) = entry.as_array() {
                    for row in rows {
                        append(season as u64, row)?;
                    }
                } else {
                    append(0, entry)?;
                }
            }
        }
        _ => return Err(CatalogError::InvalidFormat),
    }
    Ok(out)
}

pub fn episodes(
    value: &Value,
    profile: &ProviderProfile,
    config: &Config,
    parent: &Series,
) -> Result<Catalog, CatalogError> {
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for (fallback_season, row) in episode_rows(value)? {
        let Some(id) = number(&row["id"]) else {
            continue;
        };
        let season = number(&row["season"])
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(fallback_season);
        let mut extension = label(&row["container_extension"]).to_owned();
        if extension.is_empty() && !image(config, &row["direct_source"]).is_empty() {
            extension = "mp4".into();
        }
        if !valid_extension(&extension) {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let ep = number(&row["episode_num"])
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let own = description(&row["info"]["plot"]);
        let poster = image(config, &row["info"]["movie_image"]);
        let seconds = row["info"]["duration_secs"]
            .as_u64()
            .or_else(|| label(&row["info"]["duration_secs"]).parse().ok());
        ordered.push((
            season,
            ep,
            CatalogEntry {
                id: identity(&profile.id, &format!("xtream:series:{id}")),
                title: title(
                    row,
                    "title",
                    format!(
                        "{} — Episode {}",
                        parent.name,
                        if ep > 0 { ep.to_string() } else { id.clone() }
                    ),
                ),
                category: format!("{} / {} / Season {season}", parent.category, parent.name),
                description: if own.is_empty() {
                    parent.description.clone()
                } else {
                    own
                },
                poster: if poster.is_empty() {
                    parent.poster.clone()
                } else {
                    poster
                },
                media: MediaLocator::Xtream {
                    stream_id: id.clone(),
                    stream_kind: "series".into(),
                    extension,
                    series_id: Some(parent.id.clone()),
                    has_direct_source: direct_source(config, row).is_some(),
                },
                duration_ms: seconds.and_then(|s| s.checked_mul(1000)),
                provider_id: Some(id),
                provider_id_field: Some("episode_id".into()), live: None,
            },
        ));
    }
    ordered.sort_by_key(|(s, e, _)| (*s, *e));
    let mut output = Catalog::default();
    output.merge(Catalog { hierarchy: None,
        entries: ordered.into_iter().map(|(_, _, e)| e).collect(),
    })?;
    Ok(output)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieInfo {
    pub description: String,
    pub poster: String,
    pub duration_seconds: Option<u64>,
}
pub fn movie_info(value: &Value, config: &Config) -> Result<MovieInfo, CatalogError> {
    let info = value
        .get("info")
        .filter(|i| i.is_object())
        .ok_or(CatalogError::InvalidFormat)?;
    Ok(MovieInfo {
        description: description(&info["plot"]),
        poster: image(config, &info["movie_image"]),
        duration_seconds: info["duration_secs"]
            .as_u64()
            .or_else(|| label(&info["duration_secs"]).parse().ok()),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Programme {
    pub start: i64,
    pub end: i64,
    pub title: String,
    pub description: String,
}
fn epoch(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|x| *x >= 0)
}
/// The numeric epoch fields are timezone-independent. Ambiguous server-local date
/// strings are deliberately not reinterpreted as UTC.
pub fn short_epg(value: &Value) -> Result<Vec<Programme>, CatalogError> {
    let entries = rows(&value["epg_listings"])?;
    let mut output = Vec::new();
    for row in entries {
        let start = epoch(&row["start_timestamp"]);
        let end = epoch(&row["stop_timestamp"]).or_else(|| epoch(&row["end_timestamp"]));
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        if end <= start {
            continue;
        }
        output.push(Programme {
            start,
            end,
            title: wire_text(label(&row["title"])),
            description: wire_text(label(&row["description"])),
        });
    }
    output.sort_by_key(|e| e.start);
    Ok(output)
}
/// Xtream short EPG uses base64 UTF-8. Invalid base64 is retained as plain text.
fn wire_text(input: &str) -> String {
    fn value(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if input.is_empty() || input.len() % 4 != 0 || input.len() > MAX_METADATA_BYTES {
        return text(input, MAX_METADATA_BYTES);
    }
    let mut out = Vec::new();
    for (i, chunk) in input.as_bytes().chunks_exact(4).enumerate() {
        let last = (i + 1) * 4 == input.len();
        let Some(a) = value(chunk[0]) else {
            return text(input, MAX_METADATA_BYTES);
        };
        let Some(b) = value(chunk[1]) else {
            return text(input, MAX_METADATA_BYTES);
        };
        let c = value(chunk[2]);
        let d = value(chunk[3]);
        if (c.is_none() && !(last && chunk[2] == b'=' && chunk[3] == b'='))
            || (d.is_none() && !(last && chunk[3] == b'='))
        {
            return text(input, MAX_METADATA_BYTES);
        }
        out.push((a << 2) | (b >> 4));
        if let Some(c) = c {
            out.push((b << 4) | (c >> 2));
            if let Some(d) = d {
                out.push((c << 6) | d);
            }
        }
    }
    String::from_utf8(out)
        .map(|s| text(&s, MAX_METADATA_BYTES))
        .unwrap_or_else(|_| text(input, MAX_METADATA_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> Config {
        Config::parse(r#"{"server":"https://tv.invalid/base/player_api.php","username":"a/b","password":"p?&"}"#).unwrap()
    }
    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "xc".into(),
            name: "TV".into(),
            kind: super::super::ProviderKind::Xtream,
            source: "{}".into(),
            source_alias: "TV".into(),
            enabled: true,
        }
    }
    #[test]
    fn credentials_encoded_and_not_debugged() {
        let c = config();
        assert_eq!(
            c.stream_url("live", "42", "ts").unwrap(),
            "https://tv.invalid:443/base/live/a%2Fb/p%3F%26/42.ts"
        );
        assert!(!format!("{c:?}").contains("a/b"));
        assert!(c.stream_url("movie", "../x", "mkv").is_err());
    }
    #[test]
    fn auth_requires_positive_answer() {
        assert!(validate_account(&json!({"user_info":{"auth":1,"status":"Active"}})).is_ok());
        for v in [
            json!({}),
            json!({"user_info":{"auth":0}}),
            json!({"user_info":{"auth":1,"status":"Expired"}}),
        ] {
            assert!(validate_account(&v).is_err());
        }
    }
    #[test]
    fn malformed_records_do_not_discard_valid_neighbors() {
        let rows = streams(
            &json!([
                {"stream_id":"../invalid", "name":"Bad"},
                {"stream_id":7,"name":"Valid","container_extension":"mp4"},
                {"stream_id":8,"container_extension":"../../bad"}
            ]),
            &profile(),
            &config(),
            "movie",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(rows.entries.len(), 1);
        assert_eq!(rows.entries[0].title, "Valid");
    }
    #[test]
    fn array_episodes_preserve_season_and_never_save_direct_credentials() {
        let c = config();
        let parent = Series {
            id: "5".into(),
            name: "Show".into(),
            category: "Series".into(),
            poster: String::new(),
            description: String::new(),
        };
        let rows = episodes(&json!({"episodes":[
            {"id":7,"season":2,"episode_num":1,"title":"Episode","direct_source":"https://media.invalid/private-token"},
            {"id":"../bad","season":2,"container_extension":"mp4"}
        ]}),&profile(),&c,&parent).unwrap();
        assert_eq!(rows.entries.len(), 1);
        assert!(rows.entries[0].category.ends_with("Season 2"));
        assert!(
            matches!(&rows.entries[0].media, MediaLocator::Xtream {series_id:Some(id), ..} if id=="5")
        );
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("private-token"));
    }
    #[test]
    fn direct_sources_resolve_only_http_addresses() {
        assert_eq!(
            direct_source(&config(), &json!({"direct_source":"/media/file.mp4"})).unwrap(),
            "https://tv.invalid:443/media/file.mp4"
        );
        for source in [
            "file:///tmp/private",
            "javascript:alert(1)",
            "data:text/html,no",
        ] {
            assert!(direct_source(&config(), &json!({"direct_source":source})).is_none());
        }
    }
    #[test]
    fn stable_identity_ignores_credentials_and_order() {
        let c = config();
        let p = profile();
        let data = json!([{"stream_id":1,"name":"One","category_id":2},{"stream_id":2,"name":"Two","category_id":2}]);
        let g = categories(&json!([{"category_id":2,"category_name":"News"}])).unwrap();
        let a = streams(&data, &p, &c, "live", &g).unwrap();
        let mut c2 = c.clone();
        c2.password = "new".into();
        let b = streams(
            &json!([data[1].clone(), data[0].clone()]),
            &p,
            &c2,
            "live",
            &g,
        )
        .unwrap();
        assert_eq!(a.entries[0].id, b.entries[1].id);
        assert_eq!(a.entries[0].category, "News");
        assert!(!serde_json::to_string(&a).unwrap().contains("password"));
    }
    #[test]
    fn series_flattened_in_season_episode_order() {
        let c = config();
        let parent = Series {
            id: "1".into(),
            name: "Show".into(),
            category: "Series".into(),
            poster: String::new(),
            description: "Overview".into(),
        };
        let data = json!({"episodes":{"2":[{"id":"22","episode_num":2,"container_extension":"mkv"}],"1":[{"id":"11","episode_num":1,"container_extension":"mp4"}]}});
        let rows = episodes(&data, &profile(), &c, &parent).unwrap().entries;
        assert!(rows[0].category.ends_with("Season 1"));
        assert_eq!(rows[0].description, "Overview");
    }
    #[test]
    fn epg_decodes_utf8_and_uses_epoch() {
        let epg=short_epg(&json!({"epg_listings":[{"start_timestamp":"100","stop_timestamp":"200","title":"TmV3cw==","description":"VGV4dA=="},{"start":"2026-11-02 10:00:00","end":"2026-11-02 11:00:00"}]})).unwrap();
        assert_eq!(
            epg,
            vec![Programme {
                start: 100,
                end: 200,
                title: "News".into(),
                description: "Text".into()
            }]
        );
    }
}

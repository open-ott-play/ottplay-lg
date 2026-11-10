//! Explicit hosted guide v1 wire. No URL guessing or XMLTV substitution.
use super::{CatalogError, MAX_METADATA_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub url: String,
    pub source: String,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedEpgConfig(<private>)")
    }
}
impl Config {
    pub fn parse(value: &Value) -> Result<Self, CatalogError> {
        let mut value: Self =
            serde_json::from_value(value.clone()).map_err(|_| CatalogError::InvalidFormat)?;
        value.url = value.url.trim_end_matches('/').to_owned();
        if value.version != 1
            || value.source != "epg-one"
            || value.url.contains(['?', '#', '@'])
            || !value.url.ends_with("/epg/v1")
        {
            return Err(CatalogError::InvalidFormat);
        }
        value.url = super::http_url(&value.url).ok_or(CatalogError::InvalidFormat)?;
        Ok(value)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub generation: String,
    pub fetched_ms: i64,
    pub refresh_ms: i64,
}
fn text<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str, CatalogError> {
    value[key]
        .as_str()
        .filter(|v| {
            v.len() <= max
                && !v
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        })
        .ok_or(CatalogError::InvalidFormat)
}
pub fn envelope(value: &Value, expected: Option<&str>) -> Result<Envelope, CatalogError> {
    let generation = text(value, "generation", 80)?;
    let fetched = value["fetchedAt"]
        .as_i64()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
        .ok_or(CatalogError::InvalidFormat)?;
    let refresh = value["refreshMs"]
        .as_i64()
        .filter(|n| *n > 0 && *n <= 86_400_000)
        .ok_or(CatalogError::InvalidFormat)?;
    if value["version"].as_u64() != Some(1)
        || value["source"] != "epg-one"
        || value["stale"] != false
        || generation.is_empty()
        || generation.chars().any(char::is_control)
        || expected.is_some_and(|g| g != generation)
    {
        return Err(CatalogError::InvalidFormat);
    }
    Ok(Envelope {
        generation: generation.into(),
        fetched_ms: fetched,
        refresh_ms: refresh,
    })
}
#[derive(Clone, Debug)]
pub struct Mapping {
    pub channel: String,
    pub shift: i64,
}
pub fn mappings(
    value: &Value,
    ids: &BTreeSet<String>,
) -> Result<(Envelope, BTreeMap<String, Mapping>), CatalogError> {
    let meta = envelope(value, None)?;
    let object = value["mappings"]
        .as_object()
        .filter(|v| v.len() <= ids.len())
        .ok_or(CatalogError::InvalidFormat)?;
    let mut result = BTreeMap::new();
    for (id, row) in object {
        let channel = text(row, "channelId", 2048)?;
        let shift = row["shift"]
            .as_i64()
            .filter(|v| (-86400..=86400).contains(v) && v % 3600 == 0)
            .ok_or(CatalogError::InvalidFormat)?;
        if !ids.contains(id) || channel.is_empty() || channel.encode_utf16().count() > 512 {
            return Err(CatalogError::InvalidFormat);
        }
        let _ = text(row, "logo", 8192)?;
        result.insert(
            id.clone(),
            Mapping {
                channel: channel.into(),
                shift,
            },
        );
    }
    Ok((meta, result))
}
pub fn programmes(
    value: &Value,
    meta: &Envelope,
    channel: &str,
) -> Result<Vec<super::epg::Programme>, CatalogError> {
    let actual = envelope(value, Some(&meta.generation))?;
    if actual != *meta {
        return Err(CatalogError::InvalidFormat);
    }
    let rows = value["rows"]
        .as_array()
        .filter(|v| v.len() <= 20_000)
        .ok_or(CatalogError::LimitExceeded)?;
    let mut result = Vec::with_capacity(rows.len());
    let mut bytes = 0usize;
    for row in rows {
        let start = row["time"]
            .as_i64()
            .filter(|v| *v >= 0)
            .ok_or(CatalogError::InvalidFormat)?;
        let end = row["time_to"]
            .as_i64()
            .filter(|v| *v > start)
            .ok_or(CatalogError::InvalidFormat)?;
        let title = text(row, "name", MAX_METADATA_BYTES)?;
        let description = text(row, "descr", MAX_METADATA_BYTES)?;
        bytes = bytes.saturating_add(title.len() + description.len() + channel.len() + 96);
        if bytes > 8 * 1024 * 1024 {
            return Err(CatalogError::LimitExceeded);
        }
        result.push(super::epg::Programme {
            channel: channel.into(),
            start,
            end,
            title: title.into(),
            description: description.into(),
        });
    }
    Ok(result)
}
pub fn current(
    value: &Value,
    meta: &Envelope,
    ids: &BTreeSet<String>,
) -> Result<super::guide::rows::Snapshot, CatalogError> {
    let actual = envelope(value, Some(&meta.generation))?;
    if actual != *meta
        || value["checked"].as_u64() != Some(ids.len() as u64)
        || value["total"].as_u64() != Some(ids.len() as u64)
    {
        return Err(CatalogError::InvalidFormat);
    }
    let now = value["asOf"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or(CatalogError::InvalidFormat)?;
    let clock_ms = now.checked_mul(1000).ok_or(CatalogError::InvalidFormat)?;
    if clock_ms.saturating_add(999) < meta.fetched_ms
        || clock_ms >= meta.fetched_ms.saturating_add(meta.refresh_ms)
    {
        return Err(CatalogError::InvalidFormat);
    }
    let rows = value["programs"]
        .as_array()
        .filter(|v| v.len() <= ids.len())
        .ok_or(CatalogError::InvalidFormat)?;
    let mut programmes = std::collections::HashMap::new();
    let mut next = None;
    for row in rows {
        let id = text(row, "id", 2048)?;
        let title = text(row, "title", MAX_METADATA_BYTES)?;
        let start = row["start"]
            .as_i64()
            .filter(|v| *v >= 0 && *v <= now)
            .ok_or(CatalogError::InvalidFormat)?;
        let end = row["end"]
            .as_i64()
            .filter(|v| *v > now)
            .ok_or(CatalogError::InvalidFormat)?;
        if !ids.contains(id) || title.is_empty() || programmes.contains_key(id) {
            return Err(CatalogError::InvalidFormat);
        }
        programmes.insert(
            id.into(),
            super::guide::rows::CurrentProgramme {
                title: title.into(),
                start_s: start,
                end_s: end,
            },
        );
        next = Some(next.map_or(end, |old: i64| old.min(end)));
    }
    Ok(super::guide::rows::Snapshot {
        now_s: now,
        next_transition_s: next,
        programmes,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn meta() -> Value {
        json!({"version":1,"source":"epg-one","generation":"g1","fetchedAt":1000000,"refreshMs":7200000,"stale":false})
    }
    #[test]
    fn explicit_config_rejects_unknown_sources_credentials_query_and_fields() {
        let valid = json!({"version":1,"source":"epg-one","url":"https://guide.invalid/epg/v1/"});
        assert_eq!(
            Config::parse(&valid).unwrap().url,
            "https://guide.invalid:443/epg/v1"
        );
        for url in [
            "file:///epg/v1",
            "https://user:pass@guide.invalid/epg/v1",
            "https://guide.invalid/epg/v1?token=x",
            "https://guide.invalid/xmltv",
        ] {
            let mut v = valid.clone();
            v["url"] = json!(url);
            assert!(Config::parse(&v).is_err());
        }
        let mut unknown = valid.clone();
        unknown["token"] = json!("not-a-config-field");
        assert!(Config::parse(&unknown).is_err());
        let mut wrong = valid.clone();
        wrong["version"] = json!(2);
        assert!(Config::parse(&wrong).is_err());
        let mut v = valid;
        v["source"] = json!("other");
        assert!(Config::parse(&v).is_err());
    }
    #[test]
    fn wire_rejects_stale_or_replaced_generation_and_preserves_full_description() {
        let value = meta();
        let m = envelope(&value, None).unwrap();
        let mut p = value.clone();
        p["rows"] = json!([{"time":900,"time_to":1100,"name":"Programme","descr":"First sentence. Second sentence."}]);
        assert_eq!(
            programmes(&p, &m, "18").unwrap()[0].description,
            "First sentence. Second sentence."
        );
        for (key, value) in [
            ("generation", json!("g2")),
            ("source", json!("other")),
            ("stale", json!(true)),
        ] {
            let mut bad = p.clone();
            bad[key] = value;
            assert!(programmes(&bad, &m, "18").is_err());
        }
    }
    #[test]
    fn current_requires_complete_identity_bound_valid_intervals() {
        let mut v = meta();
        let m = envelope(&v, None).unwrap();
        let ids = BTreeSet::from(["local".into()]);
        v["asOf"] = json!(1000);
        v["checked"] = json!(1);
        v["total"] = json!(1);
        v["programs"] = json!([{"id":"local","start":900,"end":1100,"title":"Now"}]);
        assert_eq!(current(&v, &m, &ids).unwrap().programmes.len(), 1);
        v["programs"][0]["id"] = json!("other");
        assert!(current(&v, &m, &ids).is_err());
    }
}

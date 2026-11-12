//! Retained-MAC JSON-RPC dialect. Deliberately separate from MAG `/c` requests.
use super::{
    identity, resolve_http_url,
    stalker::{percent, scalar, Config, Dialect},
    text, Catalog, CatalogEntry, MediaLocator, ProviderProfile, MAX_CATALOG_ENTRIES,
    MAX_METADATA_BYTES,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub enum Operation<'a> {
    Handshake,
    Channels,
    Guide {
        channel: &'a str,
        from: i64,
        to: i64,
    },
}

pub fn request(config: &Config, operation: Operation<'_>) -> Result<Value, &'static str> {
    if config.dialect() != Dialect::JsonRpc {
        return Err("Stalker dialect mismatch");
    }
    let (method, mut params) = match operation {
        Operation::Handshake => ("handshake", json!({})),
        Operation::Channels => ("get_channels", json!({})),
        Operation::Guide { channel, from, to } => {
            valid_id(channel)?;
            if from < 0 || to <= from || to - from > 172800 {
                return Err("Invalid Stalker guide interval");
            }
            ("get_epg", json!({"ch_id":channel,"from":from,"to":to}))
        }
    };
    params["mac"] = Value::String(config.mac.clone());
    Ok(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
}

pub fn result(response: &Value) -> Result<&Value, &'static str> {
    if !response.is_object()
        || response.get("id").is_some_and(|v| v != &json!(1))
        || response.get("jsonrpc").is_some_and(|v| v != "2.0")
    {
        return Err("Invalid Stalker JSON-RPC response");
    }
    if response.get("error").is_some_and(|v| !v.is_null()) {
        return Err("Stalker JSON-RPC request refused");
    }
    let value = response
        .get("result")
        .ok_or("Missing Stalker JSON-RPC result")?;
    if value.is_null() || value == &Value::Bool(false) {
        return Err("Stalker JSON-RPC request refused");
    }
    Ok(value)
}

pub fn handshake(value: &Value) -> Result<(), &'static str> {
    if value == &Value::Bool(true)
        || value.as_u64() == Some(1)
        || value.is_object()
        || value.is_array()
    {
        Ok(())
    } else {
        Err("Invalid Stalker JSON-RPC handshake")
    }
}
pub fn valid_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control) {
        Err("Invalid Stalker channel identity")
    } else {
        Ok(())
    }
}

fn input_rows(data: &Value) -> Result<Vec<&Value>, &'static str> {
    if let Some(rows) = data.as_array() {
        return Ok(rows.iter().collect());
    }
    let object = data.as_object().ok_or("Invalid Stalker channel list")?;
    for key in ["data", "items", "channels"] {
        if let Some(value) = object.get(key) {
            return value
                .as_array()
                .map(|rows| rows.iter().collect())
                .ok_or("Invalid Stalker channel list");
        }
    }
    Ok(object.values().collect())
}

pub fn catalog(
    profile: &ProviderProfile,
    config: &Config,
    data: &Value,
) -> Result<Catalog, &'static str> {
    if config.dialect() != Dialect::JsonRpc {
        return Err("Stalker dialect mismatch");
    }
    let rows = input_rows(data)?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err("Stalker catalogue exceeds limit");
    }
    let mut seen = BTreeSet::new();
    let mut catalogue = Catalog::default();
    for row in rows {
        let id = scalar(
            row.get("id")
                .or_else(|| row.get("ch_id"))
                .unwrap_or(&Value::Null),
        );
        valid_id(&id)?;
        if !seen.insert(id.clone()) {
            return Err("Stalker channel identity is repeated");
        }
        let title = row["name"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .ok_or("Stalker channel has no name")?;
        let group = row
            .get("genre")
            .or_else(|| row.get("categories"))
            .or_else(|| row.get("category"));
        let group = group
            .and_then(|g| {
                g.as_str()
                    .or_else(|| g.as_array().and_then(|a| a.first()).and_then(Value::as_str))
            })
            .unwrap_or("Other");
        let logo = ["logo", "icon", "tv_icon"]
            .iter()
            .find_map(|key| row[*key].as_str())
            .unwrap_or("");
        // Validate the advertised URL now, but retain provider identity and resolve afresh at play time.
        stream_url(config, row, &id)?;
        catalogue.entries.push(CatalogEntry { protection: super::protection::advertised(row),
            synopsis: None,
            id: identity(&profile.id, &format!("stalker:api:live:{id}")),
            title: text(title, 1024),
            category: text(group, 1024),
            description: text(
                row["description"].as_str().unwrap_or(""),
                MAX_METADATA_BYTES,
            ),
            poster: resolve_http_url(&config.portal, logo).unwrap_or_default(),
            media: MediaLocator::StalkerApi {
                channel_id: id.clone(),
            },
            duration_ms: None,
            provider_id: Some(id),
            provider_id_field: Some("id".into()),
            live: None,
        });
    }
    Ok(catalogue)
}

pub fn resolve(config: &Config, data: &Value, channel: &str) -> Result<String, &'static str> {
    if config.dialect() != Dialect::JsonRpc {
        return Err("Stalker dialect mismatch");
    }
    valid_id(channel)?;
    let rows = input_rows(data)?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err("Stalker catalogue exceeds limit");
    }
    let mut selected = None;
    for row in rows {
        let id = scalar(
            row.get("id")
                .or_else(|| row.get("ch_id"))
                .unwrap_or(&Value::Null),
        );
        if id == channel {
            if selected.is_some() {
                return Err("Stalker channel identity is repeated");
            }
            selected = Some(stream_url(config, row, channel)?);
        }
    }
    selected.ok_or("Stalker channel is no longer available")
}
fn stream_url(config: &Config, row: &Value, id: &str) -> Result<String, &'static str> {
    let raw = if let Some(url) = row["url"].as_str().filter(|v| !v.is_empty()) {
        url.to_owned()
    } else {
        format!(
            "{}/stream/{}.m3u8?mac={}",
            &config.portal.trim_end_matches('/')[..config.portal.trim_end_matches('/').len() - 4],
            percent(id),
            percent(&config.mac)
        )
    };
    super::stalker::playback_url(config, &json!(raw))
}

pub fn guide(channel: &str, value: &Value) -> Result<Vec<super::epg::Programme>, &'static str> {
    valid_id(channel)?;
    let rows = value.as_array().ok_or("Invalid Stalker programme list")?;
    if rows.len() > MAX_CATALOG_ENTRIES {
        return Err("Stalker programme list exceeds limit");
    }
    rows.iter()
        .map(|row| {
            let number = |keys: &[&str]| {
                keys.iter().find_map(|key| {
                    let v = &row[*key];
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
            };
            let start =
                number(&["start_timestamp", "start"]).ok_or("Invalid Stalker programme time")?;
            let end = number(&["end_timestamp", "end"]).ok_or("Invalid Stalker programme time")?;
            if start < 0 || end <= start {
                return Err("Invalid Stalker programme interval");
            }
            Ok(super::epg::Programme {
                channel: channel.into(),
                start,
                end,
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
                    MAX_METADATA_BYTES,
                ),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "stalker_api_tests.rs"]
mod tests;

//! Strict raw poll decoding before delivery can remember or dispatch any IDs.
//! This stays at the native remote boundary: it does not change profile, pairing
//! or discovery formats and does not import a UI or build-time JSON reader.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{value::RawValue, Map, Number, Value};
use std::fmt;

/// A poll whose whole JSON tree and media-track projections were checked before
/// Delivery can remember any ID. The projection belongs to its first request,
/// matching the existing Delivery admission rule; later requests are still checked.
pub(super) struct Poll {
    pub(super) value: Value,
    pub(super) media_tracks_raw: Option<String>,
    pub(super) plex_servers_raw: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawRequests<'a> {
    #[serde(borrow)]
    requests: Vec<&'a RawValue>,
}

#[derive(serde::Deserialize)]
struct RawActionFields<'a> {
    #[serde(borrow)]
    action: &'a RawValue,
    #[serde(borrow)]
    params: &'a RawValue,
}

/// Decode the whole response within the existing transport limit. Serde's
/// normal recursion guard remains enabled; duplicate decoded keys are rejected
/// even inside nested arrays or unused request entries, before producing Value.
/// The native media_tracks limit is the received action/params projection:
/// 21 + raw action bytes + raw params bytes. Transport fields are excluded; this
/// does not claim knowledge of the controller's original inbound HTTP length.
pub(super) fn decode_poll(bytes: &[u8]) -> Result<Poll, serde_json::Error> {
    if bytes.len() > super::protocol::BODY_LIMIT {
        return Err(de::Error::custom("remote poll exceeds the body limit"));
    }
    let value = decode_unique(bytes)?;
    let mut media_tracks_raw = None;
    let mut plex_servers_raw = None;
    // Preserve existing envelope validation in Delivery. In particular, absent,
    // null or wrongly typed requests retain their existing admission semantics.
    if let Some(requests) = value.get("requests").and_then(Value::as_array).filter(|items| {
        items.iter().any(|request| matches!(request.get("action").and_then(Value::as_str), Some("media_tracks" | "plex_servers")))
    }) {
        let raw: RawRequests<'_> = serde_json::from_slice(bytes)?;
        for (index, request) in requests.iter().enumerate() {
            // Decode the action before recognition: escapes cannot bypass validation.
            let action = request.get("action").and_then(Value::as_str);
            if !matches!(action, Some("media_tracks" | "plex_servers")) { continue; }
            let received = raw.requests.get(index)
                .ok_or_else(|| <serde_json::Error as de::Error>::custom("invalid raw request index"))?;
            let fields: RawActionFields<'_> = serde_json::from_str(received.get())?;
            // Check raw field lengths before allocating the projection. The full
            // poll was already bounded by BODY_LIMIT and checked for duplicate keys.
            if action == Some("plex_servers")
                && 21 + fields.action.get().len() + fields.params.get().len() > 16 * 1024 {
                return Err(de::Error::custom("managed-server request projection exceeds the limit"));
            }
            let projection = format!("{{\"action\":{},\"params\":{}}}",
                fields.action.get(), fields.params.get());
            if action == Some("media_tracks") {
                super::media_tracks::Request::decode(projection.as_bytes())
                    .map_err(|_| <serde_json::Error as de::Error>::custom("invalid media_tracks request projection"))?;
                if index == 0 { media_tracks_raw = Some(projection); }
            } else {
                // This is a bound on the received action/params projection only.
                // The controller must enforce its original whole-HTTP-request bound.
                // Retain semantic errors for the app's correlated typed rejection;
                // the whole raw tree was already checked for duplicate keys above.
                if index == 0 { plex_servers_raw = Some(projection); }
            }
        }
    }
    Ok(Poll { value, media_tracks_raw, plex_servers_raw })
}

pub(super) fn decode_unique(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Unique>(bytes).map(|unique| unique.0)
}

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                // Do not include the potentially private key or value in errors.
                // Reject before allocating the duplicate key's nested value.
                return Err(de::Error::custom("duplicate remote JSON key"));
            }
            let value = map.next_value::<Unique>()?;
            values.insert(key, value.0);
        }
        Ok(Unique(Value::Object(values)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Unique, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<Unique>()? {
            values.push(value.0);
        }
        Ok(Unique(Value::Array(values)))
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(value.into())))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Unique, E> {
        Number::from_f64(value)
            .map(|value| Unique(Value::Number(value)))
            .ok_or_else(|| de::Error::custom("nonfinite remote JSON number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
        Ok(Unique(Value::String(value.to_owned())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Unique, E> {
        Ok(Unique(Value::String(value)))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
}

//! Bounded list/current/play adapter for the existing global Favorites document.
//! Reads neither allocate identities nor write, resolve, scan, fetch or wake sources.
use super::*;
use plx_data::ott::favorites_wire as wire;

const METADATA_BYTES: usize = 65_536;
const ENVELOPE_BYTES: usize = 16_384;
const PAGE_ENTRIES: usize = 100;
const CURSOR_BYTES: usize = 512;
pub(super) const ACTION: &str = "favorites_global_items";

fn common(inv: &Invocation) -> Value {
    json!({"version":1,"runtime":inv.params["runtime"].as_str().filter(|s|s.len()<=64).unwrap_or(""),
        "op":inv.params["op"].as_str().filter(|s|s.len()<=16).unwrap_or(""),"subject":"global"})
}
pub(super) fn error(inv: &Invocation, code: &str) -> Value {
    crate::app::ott_remote::favorites_rejection(&inv.params, code)
}
fn exact(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}
// Web clients parse 1, 1.0 and 1e0 as the same Number. The small page
// bound makes conversion exact; fractions and non-number values remain invalid.
fn page_size(value: &Value) -> Option<usize> {
    value.as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && (1.0..=PAGE_ENTRIES as f64).contains(n))
        .map(|n| n as usize)
}
fn parse(inv: &Invocation) -> Result<(), &'static str> {
    let p = &inv.params;
    if p.to_string().len().saturating_add(160) > ENVELOPE_BYTES {
        return Err("envelope_too_large");
    }
    let runtime = p["runtime"].as_str().ok_or("invalid_request")?;
    if runtime.is_empty()
        || runtime.len() > 64
        || !runtime
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || p["subject"] != "global"
    {
        return Err("invalid_request");
    }
    let valid = match p["op"].as_str() {
        Some("list") => {
            exact(p, &["runtime", "op", "subject", "cursor", "max_entries"])
                && (p["cursor"].is_null() || p["cursor"].as_str().is_some_and(valid_cursor))
                && page_size(&p["max_entries"]).is_some()
        }
        Some("current") => exact(p, &["runtime", "op", "subject"]),
        Some("play") => {
            exact(
                p,
                &[
                    "runtime",
                    "op",
                    "subject",
                    "id",
                    "expected_revision",
                    "expected_context_revision",
                ],
            ) && p["id"].as_str().is_some_and(favorite_item_ids::uuid)
                && p["expected_revision"]
                    .as_str()
                    .is_some_and(favorite_item_ids::digest)
                && p["expected_context_revision"]
                    .as_str()
                    .is_some_and(favorite_item_ids::digest)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("invalid_request")
    }
}
fn valid_cursor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= CURSOR_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn encode_cursor(value: &Value) -> String {
    plx_base::b64::encode(value.to_string().as_bytes())
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}
fn decode_cursor(value: &str) -> Result<Value, &'static str> {
    if !valid_cursor(value) {
        return Err("invalid_cursor");
    }
    let mut padded = value.replace('-', "+").replace('_', "/");
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = plx_base::b64::decode(&padded).ok_or("invalid_cursor")?;
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid_cursor")?;
    if encode_cursor(&parsed) != value {
        return Err("invalid_cursor");
    }
    Ok(parsed)
}
fn fence(runtime: &Runtime) -> Result<(String, String), &'static str> {
    if !runtime.loaded
        || runtime.persistence_uncertain
        || runtime.remote.persisted_device_id().is_none()
        || runtime
            .saved
            .receipts
            .iter()
            .any(|receipt| receipt.outcome == wire::Outcome::Unknown)
    {
        return Err("storage_unavailable");
    }
    let revision = wire::revision(&runtime.saved.favorites).map_err(|_| "storage_unavailable")?;
    let context = favorites::resolution_context(
        &runtime.saved.profiles,
        &runtime.saved.settings,
        runtime.catalog_revision,
    )
    .map_err(|_| "storage_unavailable")?;
    Ok((revision, context))
}
fn data(runtime: &Runtime, inv: &Invocation) -> Result<Value, &'static str> {
    parse(inv)?;
    if inv.params["runtime"] != runtime.remote.runtime() {
        return Err("runtime_changed");
    }
    if inv.deadline_ms <= runtime.now_ms() {
        return Err("expired");
    }
    if !runtime.remote.invocation_is_current(inv) {
        return Err("cancelled");
    }
    let (revision, context) = fence(runtime)?;
    let mut data = common(inv);
    data["revision"] = json!(revision);
    data["context_revision"] = json!(context);
    Ok(data)
}
fn bounded(data: Value) -> Result<Value, &'static str> {
    if data.to_string().len() > METADATA_BYTES {
        Err("limits_exceeded")
    } else {
        Ok(ok(data))
    }
}
fn list(runtime: &Runtime, inv: &Invocation, mut data: Value) -> Result<Value, &'static str> {
    let ids = favorite_item_ids::ordered(&runtime.saved)?;
    let mut offset = 0;
    if let Some(cursor) = inv.params["cursor"].as_str() {
        let token = decode_cursor(cursor)?;
        if !exact(
            &token,
            &["runtime", "revision", "context_revision", "offset"],
        ) || !token["runtime"].is_string()
            || !token["revision"]
                .as_str()
                .is_some_and(favorite_item_ids::digest)
            || !token["context_revision"]
                .as_str()
                .is_some_and(favorite_item_ids::digest)
        {
            return Err("invalid_cursor");
        }
        if token["runtime"] != data["runtime"] {
            return Err("runtime_changed");
        }
        if token["revision"] != data["revision"] {
            return Err("revision_conflict");
        }
        if token["context_revision"] != data["context_revision"] {
            return Err("context_changed");
        }
        offset = token["offset"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or("invalid_cursor")?;
        if offset == 0 || offset >= ids.len() {
            return Err("invalid_cursor");
        }
    }
    data["cursor"] = inv.params["cursor"].clone();
    data["page_offset"] = json!(offset);
    data["total"] = json!(ids.len());
    // This native Document contains portable entries only; legacy-only rows have
    // no separate native store and are never omitted from the portable count.
    data["unresolved_legacy_count"] = json!(0);
    let requested = page_size(&inv.params["max_entries"]).ok_or("invalid_request")?;
    let end = offset.saturating_add(requested).min(ids.len());
    let rows: Vec<Value> = (offset..end)
        .map(|i| json!({"id":ids[i],"entry":runtime.saved.favorites.entries()[i]}))
        .collect();
    data["entries"] = json!(rows);
    loop {
        let count = data["entries"].as_array().ok_or("invalid_request")?.len();
        let next = offset + count;
        data["next_cursor"] = if next < ids.len() {
            json!(encode_cursor(
                &json!({"runtime":data["runtime"],"revision":data["revision"],
                "context_revision":data["context_revision"],"offset":next})
            ))
        } else {
            Value::Null
        };
        if data.to_string().len() <= METADATA_BYTES {
            return bounded(data);
        }
        if count <= 1 {
            return Err("limits_exceeded");
        }
        data["entries"]
            .as_array_mut()
            .ok_or("invalid_request")?
            .pop();
    }
}
fn current(runtime: &Runtime, app: &App, mut data: Value) -> Result<Value, &'static str> {
    data["entry"] = Value::Null;
    data["unavailable_reason"] = json!("no_portable_current_item");
    if !app.running || !playback_allowed(app) {
        data["unavailable_reason"] = json!("standby");
    } else if let Some(playing) = runtime.playing.as_ref().filter(|playing| {
        playing.identity.is_some()
            && playing_matches(playing, app)
            && !playing_superseded(playing, app)
            && matches!(
                plx_media::player::state(&app.player.session),
                plx_media::player::PlaybackState::Playing
                    | plx_media::player::PlaybackState::Paused
                    | plx_media::player::PlaybackState::Buffering
                    | plx_media::player::PlaybackState::Seeking
            )
    }) {
        match managed::guard_profile(runtime, app, &playing.profile)
            .and_then(|()| playing_favorite::capture(runtime, playing))
        {
            Ok(favorite) => {
                data["entry"] = serde_json::to_value(favorite).map_err(|_| "unsupported_item")?;
                data["unavailable_reason"] = Value::Null;
            }
            Err(_) => data["unavailable_reason"] = json!("no_portable_current_item"),
        }
    }
    bounded(data)
}

/// Check CAS and the receiver-private row mapping again before native admission.
/// This does not resolve a new source or manufacture a second playback owner.
pub(super) fn pending_error(runtime: &Runtime) -> Option<&'static str> {
    let inv = runtime
        .pending_play
        .as_ref()
        .filter(|inv| inv.action == ACTION)?;
    match play_id(runtime, inv) {
        Ok(id)
            if runtime
                .play_target
                .as_ref()
                .is_some_and(|target| target.id == id) =>
        {
            None
        }
        Ok(_) => Some("cancelled"),
        Err(error) => Some(error),
    }
}
fn play_id(runtime: &Runtime, inv: &Invocation) -> Result<String, &'static str> {
    let data = data(runtime, inv)?;
    if inv.params["expected_revision"] != data["revision"] {
        return Err("revision_conflict");
    }
    if inv.params["expected_context_revision"] != data["context_revision"] {
        return Err("context_changed");
    }
    let ids = favorite_item_ids::ordered(&runtime.saved)?;
    let index = ids
        .iter()
        .position(|id| inv.params["id"] == *id)
        .ok_or("favorite_not_found")?;
    Ok(format!(
        "favorite:{}",
        runtime.saved.favorites.entries()[index].identity()
    ))
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    let result = (|| {
        let data = data(runtime, inv)?;
        match inv.params["op"].as_str() {
            Some("list") => list(runtime, inv, data).map(Some),
            Some("current") => current(runtime, app, data).map(Some),
            Some("play") => {
                let id = play_id(runtime, inv)?;
                if runtime.saving {
                    return Err("busy");
                }
                if !app.running || !playback_allowed(app) {
                    return Err("policy_denied");
                }
                let mut request = PlaybackRequest::ordinary(&id, 0);
                request.advance_on_failure = false;
                runtime
                    .play_request_with_policy(app, request)
                    .map_err(route_error)?;
                commands::cancel_pending_play(runtime);
                runtime.pending_play = Some(inv.clone());
                kiosk_capture::explicit_admitted(runtime);
                Ok(None)
            }
            _ => Err("invalid_request"),
        }
    })();
    match result {
        Ok(value) => value,
        Err(code) => Some(error(inv, code)),
    }
}
fn route_error(reason: &'static str) -> &'static str {
    match reason {
        "Favorite no longer exists" => "favorite_not_found",
        "Favorite source is unavailable or ambiguous" | "Source bindings are unavailable" => {
            "source_unavailable"
        }
        "Media work is waiting for foreground or worker retirement" => "busy",
        _ => "playback_failed",
    }
}

/// Normalize every terminal path of the existing addressed playback request.
/// Only an actual accepted native start produces dispatched:true. Failures never
/// echo internal source URLs, profile credentials or raw resolver diagnostics.
pub(super) fn finish(inv: &Invocation, response: Value, code: Option<&str>) -> Value {
    if inv.action != ACTION {
        return response;
    }
    if response["status"] != "ok" || code.is_some() {
        return error(inv, code.unwrap_or("playback_failed"));
    }
    let mut data = common(inv);
    data["revision"] = inv.params["expected_revision"].clone();
    data["context_revision"] = inv.params["expected_context_revision"].clone();
    data["id"] = inv.params["id"].clone();
    data["dispatched"] = json!(true);
    ok(data)
}

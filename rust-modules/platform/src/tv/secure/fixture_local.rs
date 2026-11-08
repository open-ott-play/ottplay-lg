//! Explicit synthetic mount observations and passive wire evidence for HostFixture only.
//! This child is excluded with its parent from every shipping/TV feature set.
use super::{overlap, private_directory, read_bounded, same_directory};
use crate::tv::removable::{
    identity::{directory_device, IdentityError, IdentityObservation, StableIdentity},
    MountCandidate, ReadError,
};
use plx_base::task::OffFrame;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

const DECLARATION_BYTES: usize = 64 * 1024;
const VOLUME_LIMIT: usize = 1;
const WIRE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_LIMIT: usize = 256;
const FIELD_LIMIT: usize = 64;
const EVENT_BYTES: usize = 4096;
const EVENT_LIMIT: usize = 8192;
const LIFECYCLE_LIMIT: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    version: u8,
    volumes: Vec<VolumeDeclaration>,
    observe_control: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeDeclaration {
    path: PathBuf,
    uuid: String,
}
struct Volume {
    path: PathBuf,
    directory: File,
    identity: StableIdentity,
}
pub(super) struct LocalFixture {
    volumes: Vec<Volume>,
    events: AtomicUsize,
    lifecycle: AtomicUsize,
}
impl LocalFixture {
    pub(super) fn read(path: &Path, excluded: &[&Path]) -> Result<Self, &'static str> {
        if !path.is_absolute()
            || std::fs::canonicalize(path).map_err(|_| "local declaration unavailable")? != path
        {
            return Err("local declaration must be canonical");
        }
        let parent = path.parent().ok_or("local declaration parent required")?;
        let directory = private_directory(parent)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid local declaration name")?;
        let bytes = read_bounded(
            super::checked_file_at(&directory, name, false, true)?,
            DECLARATION_BYTES,
        )?;
        same_directory(&directory, parent)?;
        let declaration: Declaration =
            serde_json::from_slice(&bytes).map_err(|_| "invalid local declaration")?;
        if declaration.version != 1
            || !declaration.observe_control
            || declaration.volumes.is_empty()
            || declaration.volumes.len() > VOLUME_LIMIT
        {
            return Err("invalid local fixture bounds or version");
        }
        let mut volumes: Vec<Volume> = Vec::new();
        for volume in declaration.volumes {
            if excluded.iter().any(|p| overlap(p, &volume.path))
                || volumes
                    .iter()
                    .any(|v| overlap(&v.path, &volume.path) || v.identity.uuid() == volume.uuid)
            {
                return Err("local fixture roots and identities must be distinct");
            }
            let directory = private_directory(&volume.path)?;
            let identity = StableIdentity::new(&volume.uuid, "ext4")
                .map_err(|_| "invalid local fixture identity")?;
            volumes.push(Volume {
                path: volume.path,
                directory,
                identity,
            });
        }
        Ok(Self {
            volumes,
            events: AtomicUsize::new(0),
            lifecycle: AtomicUsize::new(0),
        })
    }
    pub(super) fn mounts(
        &self,
        off: &OffFrame,
        check: &mut dyn FnMut() -> bool,
    ) -> Result<Vec<MountCandidate>, ReadError> {
        let mut mounts = Vec::new();
        for (index, volume) in self.volumes.iter().enumerate() {
            if !check() {
                return Err(ReadError::Cancelled);
            }
            match std::fs::symlink_metadata(&volume.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(ReadError::Unavailable),
                Ok(_) => (),
            }
            same_directory(&volume.directory, &volume.path).map_err(|_| ReadError::Unavailable)?;
            let device =
                directory_device(off, &volume.directory).map_err(|_| ReadError::Unavailable)?;
            let path = volume.path.to_str().ok_or(ReadError::Unavailable)?;
            let mount = MountCandidate::fixture_directory(path, index as u64 + 1, device)
                .map_err(|_| ReadError::Unavailable)?;
            mounts.push(mount);
        }
        if !check() {
            return Err(ReadError::Cancelled);
        }
        Ok(mounts)
    }
    pub(super) fn identities(
        &self,
        off: &OffFrame,
        mounts: &[MountCandidate],
        check: &mut dyn FnMut() -> bool,
    ) -> Result<Vec<IdentityObservation>, IdentityError> {
        let current = self.mounts(off, check).map_err(|e| {
            if e == ReadError::Cancelled {
                IdentityError::Cancelled
            } else {
                IdentityError::Unavailable
            }
        })?;
        if mounts != current {
            return Err(IdentityError::Changed);
        }
        let mut identities = Vec::new();
        for mount in current {
            if !check() {
                return Err(IdentityError::Cancelled);
            }
            let volume = self
                .volumes
                .iter()
                .find(|v| v.path.to_str() == Some(mount.mount_point()))
                .ok_or(IdentityError::Changed)?;
            identities.push(IdentityObservation::fixture_observation(
                mount,
                volume.identity.clone(),
            ));
        }
        if !check() {
            return Err(IdentityError::Cancelled);
        }
        Ok(identities)
    }
    pub(super) fn lifecycle(&self, token: &str) -> Option<u32> {
        let event = match token {
            "fixture:background" => 0x104,
            "fixture:foreground" => 0x106,
            _ => return None,
        };
        self.lifecycle
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < LIFECYCLE_LIMIT).then_some(n + 1)
            })
            .ok()?;
        Some(event)
    }
    pub(super) fn lifecycle_queued(&self, event: u32, accepted: bool) {
        self.emit(json!({"version":1,"kind":"lifecycle_queued","event":event,"accepted":accepted}));
    }
    pub(super) fn control(
        &self,
        method: &str,
        url: &str,
        body: Option<&str>,
        reply: Option<(u16, &[u8])>,
    ) {
        // Origin trust is checked by the parent against its immutable CA grant.
        let path = url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("");
        let path = path.split('?').next().unwrap_or("");
        if method == "GET" && path == "/api/webhook/commands" {
            match reply
                .filter(|(status, _)| *status == 200)
                .ok_or("poll_transport")
                .and_then(|(_, bytes)| received(bytes))
            {
                Ok(events) => {
                    for event in events {
                        self.emit(event);
                    }
                }
                Err(code) => self.problem(code),
            }
        } else if method == "POST" && path == "/api/responses" {
            match body
                .ok_or("response_body_missing")
                .and_then(|body| sent(body, reply.map(|r| r.0)))
            {
                Ok(event) => self.emit(event),
                Err(code) => self.problem(code),
            }
        }
    }
    pub(super) fn unavailable(&self) {
        self.problem("fixture_authority_unavailable");
    }
    fn problem(&self, code: &'static str) {
        self.emit(json!({"version":1,"kind":"observer_error","error":code}));
    }
    fn emit(&self, event: Value) {
        let sequence = self
            .events
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n <= EVENT_LIMIT).then_some(n + 1)
            });
        let Ok(sequence) = sequence else {
            return;
        };
        let mut event = if sequence == EVENT_LIMIT {
            json!({"version":1,"kind":"observer_error","error":"event_limit"})
        } else {
            event
        };
        event["sequence"] = json!(sequence);
        let mut line = event.to_string();
        if line.len() > EVENT_BYTES {
            line = json!({"version":1,"kind":"observer_error","error":"event_size","sequence":sequence}).to_string();
        }
        plx_base::eventlog::log(&format!("LOCAL_LOOPBACK {line}"));
    }
}

fn digest(bytes: &[u8]) -> String {
    plx_base::sha256::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
const PARAM_KEYS: &[&str] = &[
    "version",
    "runtime",
    "op",
    "generation",
    "entry_id",
    "cursor",
    "select_id",
    "source",
    "source_handle",
    "include_paths",
    "path",
    "session",
    "progressive",
    "local_browser",
    "local_media_handles",
    "local_media_session",
    "local_media_progressive",
];
const DATA_KEYS: &[&str] = &[
    "version",
    "runtime",
    "op",
    "state",
    "error",
    "generation",
    "entries",
    "next_cursor",
    "select_id",
    "parent_id",
    "source_handle",
    "name",
    "id",
    "path",
    "sources",
    "source",
    "profile",
    "position",
    "scanning",
    "persistent",
    "capabilities",
    "player",
    "session",
    "progressive",
    "complete",
    "local_browser",
    "local_media_handles",
    "local_media_session",
    "local_media_progressive",
];
const OPS: &[&str] = &[
    "roots", "list", "more", "select", "sources", "connect", "play", "shuffle", "unknown",
];
const ERRORS: &[&str] = &[
    "invalid_request",
    "unsupported",
    "busy",
    "stale",
    "source_changed",
    "unavailable",
    "restricted",
    "expired",
    "cancelled",
    "limit_exceeded",
    "runtime_changed",
    "source_not_found",
    "source_ambiguous",
    "source_unavailable",
    "catalog_loading",
    "catalog_unavailable",
    "empty_source",
    "too_many_items",
    "operation_in_progress",
    "native_error",
];
fn closed(value: &Value, choices: &[&str]) -> Value {
    value
        .as_str()
        .filter(|s| choices.contains(s))
        .map_or(Value::Null, |s| json!(s))
}
fn id(value: &Value) -> Value {
    value
        .as_str()
        .filter(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .map_or(Value::Null, |s| json!(s))
}
fn hashed(value: &Value) -> Option<String> {
    value.as_str().map(|s| digest(s.as_bytes()))
}
fn fields<'a>(value: &'a Value, allowed: &[&str]) -> Result<(Vec<&'a str>, usize), &'static str> {
    let object = value.as_object().ok_or("object_required")?;
    if object.len() > FIELD_LIMIT {
        return Err("field_limit");
    }
    let keys: Vec<_> = object
        .keys()
        .filter(|key| allowed.contains(&key.as_str()))
        .map(String::as_str)
        .collect();
    let unknown = object.len() - keys.len();
    Ok((keys, unknown))
}
fn received(bytes: &[u8]) -> Result<Vec<Value>, &'static str> {
    if bytes.len() > WIRE_BYTES {
        return Err("wire_limit");
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "poll_json")?;
    let Some(requests) = value.get("requests") else {
        return Ok(Vec::new());
    };
    let requests = requests.as_array().ok_or("requests_array")?;
    if requests.len() > REQUEST_LIMIT {
        return Err("request_limit");
    }
    let mut events = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        if !matches!(
            request["action"].as_str(),
            Some("capabilities" | "local_browser" | "local_media")
        ) {
            continue;
        }
        let params = &request["params"];
        let (keys, unknown_keys) = fields(params, PARAM_KEYS)?;
        let mut extensions = serde_json::Map::new();
        for key in [
            "local_browser",
            "local_media_handles",
            "local_media_session",
            "local_media_progressive",
        ] {
            if let Some(value) = params.get(key) {
                extensions.insert(
                    key.into(),
                    value.as_bool().map_or(Value::Null, |v| json!(v)),
                );
            }
        }
        events.push(json!({"version":1,"kind":"rpc_received","index":index,"id":id(&request["id"]),
            "action":closed(&request["action"], &["capabilities", "local_browser", "local_media"]),"params_keys":keys,"unknown_params_count":unknown_keys,"runtime_sha256":hashed(&params["runtime"]),
            "op":closed(&params["op"], OPS),"extensions":extensions,
            "source_handle_sha256":params["source_handle"].as_str().map(|s| digest(s.as_bytes())),
            "envelope_sha256":digest(bytes),"expires_at":request["expires_at"].as_f64(),"server_time":value["server_time"].as_f64()}));
    }
    Ok(events)
}
fn sent(body: &str, http_status: Option<u16>) -> Result<Value, &'static str> {
    if body.len() > WIRE_BYTES {
        return Err("wire_limit");
    }
    let value: Value = serde_json::from_str(body).map_err(|_| "response_json")?;
    let data = &value["data"];
    let (keys, unknown_keys) = fields(data, DATA_KEYS)?;
    Ok(
        json!({"version":1,"kind":"rpc_response","id":id(&value["id"]),"status":closed(&value["status"], &["ok", "rejected", "unsupported"]),
        "data_keys":keys,"unknown_data_count":unknown_keys,"runtime_sha256":hashed(&data["runtime"]),"op":closed(&data["op"], OPS),
        "state":closed(&data["state"], &["ready", "connected", "playing", "error"]),"error":closed(&data["error"], ERRORS),"http_status":http_status,"body_sha256":digest(body.as_bytes())}),
    )
}

#[cfg(test)]
#[path = "fixture_local_tests.rs"]
mod tests;

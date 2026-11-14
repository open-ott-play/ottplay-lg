//! Closed multi-view wire codec. This grants no slot, source or decoder authority.
//! The app owner admits the original invocation and checks its runtime, revisions,
//! policy and deadline immediately before effects. No capability is advertised here.
use serde::Deserialize;
use serde_json::{json, value::RawValue, Map, Value};
use std::collections::{BTreeMap, HashSet};

pub(crate) const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Full serialized response envelope, including Delivery's original request ID.
/// Delivery independently enforces this limit after adding its transport fields.
pub(crate) const MAX_RESPONSE_BYTES: usize = 256 * 1024;
// This codec emits only {status,data}. Reserve framing space for Delivery's
// original ID; this is not permission to add unbounded fields after encoding.
const MAX_CODEC_RESPONSE_BYTES: usize = MAX_RESPONSE_BYTES - 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    InvalidRequest,
    Unsupported,
    RuntimeChanged,
    SceneChanged,
    LayoutChanged,
    FocusChanged,
    AudioChanged,
    SlotChanged,
    SourceChanged,
    SourceUnavailable,
    SourceExpired,
    PolicyRestricted,
    LimitExceeded,
    Busy,
    Expired,
    Unavailable,
    ReceiptUnavailable,
}
impl Reason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unsupported => "unsupported",
            Self::RuntimeChanged => "runtime_changed",
            Self::SceneChanged => "scene_changed",
            Self::LayoutChanged => "layout_changed",
            Self::FocusChanged => "focus_changed",
            Self::AudioChanged => "audio_changed",
            Self::SlotChanged => "slot_changed",
            Self::SourceChanged => "source_changed",
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceExpired => "source_expired",
            Self::PolicyRestricted => "policy_restricted",
            Self::LimitExceeded => "limit_exceeded",
            Self::Busy => "busy",
            Self::Expired => "expired",
            Self::Unavailable => "unavailable",
            Self::ReceiptUnavailable => "receipt_unavailable",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "invalid_request" => Self::InvalidRequest,
            "unsupported" => Self::Unsupported,
            "runtime_changed" => Self::RuntimeChanged,
            "scene_changed" => Self::SceneChanged,
            "layout_changed" => Self::LayoutChanged,
            "focus_changed" => Self::FocusChanged,
            "audio_changed" => Self::AudioChanged,
            "slot_changed" => Self::SlotChanged,
            "source_changed" => Self::SourceChanged,
            "source_unavailable" => Self::SourceUnavailable,
            "source_expired" => Self::SourceExpired,
            "policy_restricted" => Self::PolicyRestricted,
            "limit_exceeded" => Self::LimitExceeded,
            "busy" => Self::Busy,
            "expired" => Self::Expired,
            "unavailable" => Self::Unavailable,
            "receipt_unavailable" => Self::ReceiptUnavailable,
            _ => return None,
        })
    }
}

/// Errors contain only a closed reason, never raw JSON, labels or authority tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecodeError {
    pub(crate) reason: Reason,
}
impl From<Reason> for DecodeError {
    fn from(reason: Reason) -> Self {
        Self { reason }
    }
}
fn invalid() -> DecodeError {
    Reason::InvalidRequest.into()
}

// Deliberately no Debug: contexts, source tickets and filters are not diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) scene: String,
    pub(crate) layout_revision: String,
    pub(crate) focus_revision: String,
    pub(crate) audio_revision: String,
}
impl Context {
    fn decode(value: &Value) -> Option<Self> {
        let fields = value.as_object()?;
        if !keys(
            fields,
            &[
                "scene",
                "layout_revision",
                "focus_revision",
                "audio_revision",
            ],
            &[],
        ) {
            return None;
        }
        Some(Self {
            scene: text(fields, "scene", token)?.into(),
            layout_revision: text(fields, "layout_revision", token)?.into(),
            focus_revision: text(fields, "focus_revision", token)?.into(),
            audio_revision: text(fields, "audio_revision", token)?.into(),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SlotBinding {
    pub(crate) slot_id: u8,
    pub(crate) slot_generation: String,
    pub(crate) source_revision: String,
}
impl SlotBinding {
    fn decode(fields: &Map<String, Value>) -> Option<Self> {
        Some(Self {
            slot_id: integer(fields.get("slot_id")?, 8)?,
            slot_generation: text(fields, "slot_generation", token)?.into(),
            source_revision: text(fields, "source_revision", token)?.into(),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Default)]
pub(crate) struct SourceFilters {
    pub(crate) provider: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Single,
    Pip,
    Grid4,
    Grid9,
    Perimeter { satellites: u8 },
}
impl Layout {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Pip => "pip",
            Self::Grid4 => "grid4",
            Self::Grid9 => "grid9",
            Self::Perimeter { .. } => "perimeter",
        }
    }
    pub(crate) fn slots(self) -> u8 {
        match self {
            Self::Single => 1,
            Self::Pip => 2,
            Self::Grid4 => 4,
            Self::Grid9 => 9,
            Self::Perimeter { satellites } => satellites + 1,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum AudioPolicy {
    FollowsFocus,
    Muted,
    Slot(SlotBinding),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OffMode {
    PromoteActive,
    RestorePrevious,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Operation {
    Status,
    Sources {
        slot: SlotBinding,
        filters: SourceFilters,
    },
    Layout(Layout),
    Play {
        slot: SlotBinding,
        source_ticket: String,
    },
    Focus(SlotBinding),
    Audio(AudioPolicy),
    Stop(SlotBinding),
    Off(OffMode),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Requested,
    Applied,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Request {
    runtime: Option<String>,
    expected: Option<Context>,
    operation: Operation,
    binding: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawEnvelope<'a> {
    #[serde(borrow)]
    params: BTreeMap<String, &'a RawValue>,
}

impl Request {
    /// Decode the received action/params projection, including whitespace in its
    /// byte bound. The controller separately bounds its original queued envelope.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Reason::LimitExceeded.into());
        }
        let value = super::ingress::decode_unique(bytes).map_err(|_| invalid())?;
        let envelope = value.as_object().ok_or_else(invalid)?;
        if !keys(envelope, &["action", "params"], &[])
            || envelope.get("action").and_then(Value::as_str) != Some("multiview")
        {
            return Err(invalid());
        }
        let fields = envelope["params"].as_object().ok_or_else(invalid)?;
        let raw: RawEnvelope<'_> = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        // Inspect original integer lexemes, never a reserialized Value. In
        // particular, -0, 1.0 and 1e0 cannot gain authority by normalization.
        if raw.params.get("version").map(|v| v.get()) != Some("1") {
            return Err(invalid());
        }
        for name in ["slot_id", "satellites"] {
            if let Some(number) = raw.params.get(name) {
                let bytes = number.get().as_bytes();
                if bytes.len() != 1 || !(b'0'..=b'8').contains(&bytes[0]) {
                    return Err(invalid());
                }
            }
        }
        let op = fields
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let runtime = optional_text(fields, "runtime", token)?;
        let expected = if op == "status" {
            None
        } else {
            if runtime.is_none() {
                return Err(invalid());
            }
            Some(Context::decode(fields.get("expected").ok_or_else(invalid)?).ok_or_else(invalid)?)
        };
        let mut required = vec!["version", "op"];
        let mut optional = Vec::new();
        if op == "status" {
            optional.push("runtime");
        } else {
            required.extend(["runtime", "expected"]);
        }
        let slot = || SlotBinding::decode(fields).ok_or_else(invalid);
        let operation = match op {
            "status" => Operation::Status,
            "sources" => {
                required.extend(["slot_id", "slot_generation", "source_revision"]);
                optional.extend(["provider", "profile", "query", "cursor"]);
                Operation::Sources {
                    slot: slot()?,
                    filters: SourceFilters {
                        provider: optional_text(fields, "provider", provider)?,
                        profile: optional_text(fields, "profile", token)?,
                        query: optional_text(fields, "query", |v| public_text(v, false))?,
                        cursor: optional_text(fields, "cursor", ticket)?,
                    },
                }
            }
            "layout" => {
                required.push("layout");
                let layout = match fields.get("layout").and_then(Value::as_str) {
                    Some("single") => Layout::Single,
                    Some("pip") => Layout::Pip,
                    Some("grid4") => Layout::Grid4,
                    Some("grid9") => Layout::Grid9,
                    Some("perimeter") => {
                        required.push("satellites");
                        Layout::Perimeter {
                            satellites: integer(fields.get("satellites").ok_or_else(invalid)?, 8)
                                .ok_or_else(invalid)?,
                        }
                    }
                    _ => return Err(invalid()),
                };
                Operation::Layout(layout)
            }
            "play" => {
                required.extend([
                    "slot_id",
                    "slot_generation",
                    "source_revision",
                    "source_ticket",
                ]);
                Operation::Play {
                    slot: slot()?,
                    source_ticket: text(fields, "source_ticket", ticket)
                        .ok_or_else(invalid)?
                        .into(),
                }
            }
            "focus" | "stop" => {
                required.extend(["slot_id", "slot_generation", "source_revision"]);
                if op == "focus" {
                    Operation::Focus(slot()?)
                } else {
                    Operation::Stop(slot()?)
                }
            }
            "audio" => {
                required.push("policy");
                let policy = match fields.get("policy").and_then(Value::as_str) {
                    Some("follows_focus") => AudioPolicy::FollowsFocus,
                    Some("muted") => AudioPolicy::Muted,
                    Some("slot") => {
                        required.extend(["slot_id", "slot_generation", "source_revision"]);
                        AudioPolicy::Slot(slot()?)
                    }
                    _ => return Err(invalid()),
                };
                Operation::Audio(policy)
            }
            "off" => {
                required.push("mode");
                Operation::Off(match fields.get("mode").and_then(Value::as_str) {
                    Some("promote_active") => OffMode::PromoteActive,
                    Some("restore_previous") => OffMode::RestorePrevious,
                    _ => return Err(invalid()),
                })
            }
            _ => return Err(invalid()),
        };
        if !keys(fields, &required, &optional) {
            return Err(invalid());
        }
        let mut binding = fields.clone();
        for key in ["version", "runtime", "op"] {
            binding.remove(key);
        }
        Ok(Self {
            runtime,
            expected,
            operation,
            binding,
        })
    }
    pub(crate) fn runtime(&self) -> Option<&str> {
        self.runtime.as_deref()
    }
    pub(crate) fn expected(&self) -> Option<&Context> {
        self.expected.as_ref()
    }
    pub(crate) fn operation(&self) -> &Operation {
        &self.operation
    }
    pub(crate) fn op(&self) -> &'static str {
        match &self.operation {
            Operation::Status => "status",
            Operation::Sources { .. } => "sources",
            Operation::Layout(_) => "layout",
            Operation::Play { .. } => "play",
            Operation::Focus(_) => "focus",
            Operation::Audio(_) => "audio",
            Operation::Stop(_) => "stop",
            Operation::Off(_) => "off",
        }
    }
    pub(crate) fn slot(&self) -> Option<&SlotBinding> {
        match &self.operation {
            Operation::Sources { slot, .. }
            | Operation::Play { slot, .. }
            | Operation::Focus(slot)
            | Operation::Stop(slot)
            | Operation::Audio(AudioPolicy::Slot(slot)) => Some(slot),
            _ => None,
        }
    }

    /// Negative replies echo the requested runtime, even for runtime_changed.
    /// Only an unbound status uses current_runtime. No new context is returned.
    pub(crate) fn rejected(
        &self,
        current_runtime: &str,
        reason: Reason,
    ) -> Result<Value, DecodeError> {
        self.reply(
            current_runtime,
            if reason == Reason::Unsupported {
                "unsupported"
            } else {
                "rejected"
            },
            json!({"reason": reason.code()}),
        )
    }
    /// This proves only that the operation was requested/applied, never frames or
    /// audible samples. The app owner must first admit the exact invocation.
    pub(crate) fn accepted(
        &self,
        current_runtime: &str,
        outcome: Outcome,
    ) -> Result<Value, DecodeError> {
        if matches!(
            self.operation,
            Operation::Status | Operation::Sources { .. }
        ) {
            return Err(invalid());
        }
        self.reply(current_runtime, "ok", json!({"accepted":true,"outcome": match outcome { Outcome::Requested => "requested", Outcome::Applied => "applied" }}))
    }
    pub(crate) fn status_response(
        &self,
        current_runtime: &str,
        snapshot: Value,
    ) -> Result<Value, DecodeError> {
        if !matches!(self.operation, Operation::Status) || !valid_snapshot(&snapshot) {
            return Err(invalid());
        }
        self.reply(current_runtime, "ok", json!({"snapshot":snapshot}))
    }
    pub(crate) fn sources_response(
        &self,
        current_runtime: &str,
        items: Vec<Value>,
        cursor: Option<&str>,
    ) -> Result<Value, DecodeError> {
        if !matches!(self.operation, Operation::Sources { .. })
            || items.len() > 128
            || cursor.is_some_and(|v| !ticket(v))
        {
            return Err(invalid());
        }
        let mut ids = HashSet::new();
        let mut tickets = HashSet::new();
        for item in &items {
            if !valid_source(item, true)
                || !ids.insert(item["source_id"].as_str().unwrap())
                || !tickets.insert(item["ticket"].as_str().unwrap())
            {
                return Err(invalid());
            }
        }
        self.reply(
            current_runtime,
            "ok",
            json!({"items":items,"cursor":cursor,"complete":cursor.is_none()}),
        )
    }
    fn reply(
        &self,
        current_runtime: &str,
        status: &str,
        payload: Value,
    ) -> Result<Value, DecodeError> {
        if !token(current_runtime)
            || (status == "ok" && self.runtime().is_some_and(|v| v != current_runtime))
        {
            return Err(Reason::RuntimeChanged.into());
        }
        let runtime = self.runtime().unwrap_or(current_runtime);
        let mut data = payload.as_object().cloned().ok_or_else(invalid)?;
        data.extend([
            (String::from("version"), json!(1)),
            (String::from("runtime"), json!(runtime)),
            (String::from("op"), json!(self.op())),
            (String::from("binding"), Value::Object(self.binding.clone())),
        ]);
        let result = json!({"status":status,"data":data});
        if serde_json::to_vec(&result).map_err(|_| invalid())?.len() > MAX_CODEC_RESPONSE_BYTES {
            return Err(Reason::LimitExceeded.into());
        }
        Ok(result)
    }
}

fn keys(fields: &Map<String, Value>, required: &[&str], optional: &[&str]) -> bool {
    required.iter().all(|k| fields.contains_key(*k))
        && fields
            .keys()
            .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
}
fn text<'a>(
    fields: &'a Map<String, Value>,
    key: &str,
    valid: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    fields.get(key)?.as_str().filter(|v| valid(v))
}
fn optional_text(
    fields: &Map<String, Value>,
    key: &str,
    valid: impl Fn(&str) -> bool,
) -> Result<Option<String>, DecodeError> {
    fields
        .get(key)
        .map(|v| {
            v.as_str()
                .filter(|s| valid(s))
                .map(String::from)
                .ok_or_else(invalid)
        })
        .transpose()
}
fn integer(value: &Value, max: u8) -> Option<u8> {
    value
        .as_u64()
        .filter(|v| *v <= u64::from(max))
        .map(|v| v as u8)
}
fn token(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
}
fn ticket(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn provider(value: &str) -> bool {
    if !(1..=64).contains(&value.len()) || value.split('/').count() > 2 {
        return false;
    }
    value.split('/').all(|part| {
        !part.is_empty()
            && (part.as_bytes()[0].is_ascii_lowercase() || part.as_bytes()[0].is_ascii_digit())
            && part
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
    })
}

fn valid_source(value: &Value, with_ticket: bool) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    let mut required = vec!["source_id", "label", "provider", "profile", "kind"];
    if with_ticket {
        required.push("ticket");
    }
    keys(fields, &required, &[])
        && text(fields, "source_id", token).is_some()
        && text(fields, "label", |v| public_text(v, true)).is_some()
        && text(fields, "provider", provider).is_some()
        && text(fields, "profile", token).is_some()
        && matches!(fields["kind"].as_str(), Some("live" | "vod" | "file"))
        && (!with_ticket || text(fields, "ticket", ticket).is_some())
}
fn valid_snapshot(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    if !keys(
        fields,
        &[
            "context",
            "layout",
            "satellites",
            "focus_slot",
            "audio_policy",
            "audio_slot",
            "slots",
        ],
        &[],
    ) || Context::decode(&fields["context"]).is_none()
    {
        return false;
    }
    let Some(satellites) = integer(&fields["satellites"], 8) else {
        return false;
    };
    let layout = fields["layout"].as_str();
    let count = match layout {
        Some("off") => 0,
        Some("single") => 1,
        Some("pip") => 2,
        Some("grid4") => 4,
        Some("grid9") => 9,
        Some("perimeter") => satellites + 1,
        _ => return false,
    };
    if layout != Some("perimeter") && satellites != 0 {
        return false;
    }
    let Some(slots) = fields["slots"]
        .as_array()
        .filter(|v| v.len() == usize::from(count))
    else {
        return false;
    };
    let mut ids = HashSet::new();
    for slot in slots {
        let Some(s) = slot.as_object() else {
            return false;
        };
        if !keys(
            s,
            &[
                "slot_id",
                "slot_generation",
                "source_revision",
                "state",
                "source",
                "error",
            ],
            &[],
        ) {
            return false;
        }
        let Some(bound) = SlotBinding::decode(s) else {
            return false;
        };
        if bound.slot_id >= count || !ids.insert(bound.slot_id) {
            return false;
        }
        let state = s["state"].as_str();
        if !matches!(
            state,
            Some(
                "empty"
                    | "starting"
                    | "playing"
                    | "paused"
                    | "stalled"
                    | "ended"
                    | "failed"
                    | "stopped"
            )
        ) {
            return false;
        }
        if !s["source"].is_null() && !valid_source(&s["source"], false) {
            return false;
        }
        if matches!(state, Some("starting" | "playing" | "paused" | "stalled"))
            && s["source"].is_null()
        {
            return false;
        }
        if !s["error"].is_null() && s["error"].as_str().and_then(Reason::parse).is_none() {
            return false;
        }
        if state == Some("failed") && s["error"].is_null() {
            return false;
        }
    }
    let focus = if fields["focus_slot"].is_null() {
        None
    } else {
        match integer(&fields["focus_slot"], 8) {
            Some(v) if ids.contains(&v) => Some(v),
            _ => return false,
        }
    };
    let audio = if fields["audio_slot"].is_null() {
        None
    } else {
        match integer(&fields["audio_slot"], 8) {
            Some(v) if ids.contains(&v) => Some(v),
            _ => return false,
        }
    };
    let policy = fields["audio_policy"].as_str();
    if count == 0 {
        return focus.is_none() && audio.is_none() && policy == Some("muted");
    }
    if focus.is_none() {
        return false;
    }
    match policy {
        Some("muted") => audio.is_none(),
        Some("slot") => audio.is_some(),
        Some("follows_focus") => audio.is_none() || audio == focus,
        _ => false,
    }
}

fn public_text(value: &str, nonempty: bool) -> bool {
    if value.len() > 512
        || (nonempty && value.trim().is_empty())
        || value.chars().any(|c| c.is_control() || format_character(c))
    {
        return false;
    }
    if value.trim().is_empty() {
        return true;
    }
    // Reuse the native display guard for URLs and credential syntax, and extend
    // it with the shared media-track protocol's embedded-path classification.
    if super::display::text(value, 512).is_none() {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    for offset in 0..bytes.len() {
        if offset != 0
            && !bytes[offset - 1].is_ascii_whitespace()
            && !matches!(bytes[offset - 1], b'"' | b'\'' | b'(')
        {
            continue;
        }
        let rest = &bytes[offset..];
        if (rest[0] == b'/' && rest.get(1).is_some_and(|b| !b.is_ascii_whitespace()))
            || rest.starts_with(b"~/")
            || rest.starts_with(b"\\\\")
            || (rest.len() >= 3
                && rest[0].is_ascii_alphabetic()
                && rest[1] == b':'
                && matches!(rest[2], b'/' | b'\\'))
        {
            return false;
        }
    }
    for key in [
        "http:",
        "https:",
        "magnet:",
        "data:",
        "file:",
        "token",
        "apikey",
        "api_key",
        "api-key",
        "authorization",
        "password",
        "secret",
    ] {
        for (offset, _) in lower.match_indices(key) {
            if offset > 0
                && (bytes[offset - 1].is_ascii_alphanumeric() || bytes[offset - 1] == b'_')
            {
                continue;
            }
            if key.ends_with(':')
                || lower[offset + key.len()..]
                    .trim_start()
                    .starts_with([':', '='])
            {
                return false;
            }
        }
    }
    true
}
fn format_character(c: char) -> bool {
    // Unicode Cf ranges, excluding unassigned U+2065 (not a format character).
    matches!(c, '\u{00ad}' | '\u{0600}'..='\u{0605}' | '\u{061c}' | '\u{06dd}' | '\u{070f}' | '\u{0890}'..='\u{0891}' | '\u{08e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

#[cfg(test)]
#[path = "multiview_tests.rs"]
mod tests;

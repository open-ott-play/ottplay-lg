//! Closed, installation-scoped private OTT storage protocol. Values never enter Debug.
use super::{ErrorCode, SecretString};
use serde::{Deserialize, Serialize};

pub const LOGICAL_BYTES: usize = 1024 * 1024;
pub const ENVELOPE_BYTES: usize = 2 * LOGICAL_BYTES;
pub const FRAME_BYTES: usize = 8 * 1024 * 1024 + 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    Control,
    State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Barrier {
    Open,
    Pending,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expected {
    Missing {},
    Present {
        db_rev: String,
        revision: u64,
        epoch: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub db_rev: String,
    pub revision: u64,
    pub epoch: u64,
    pub barrier: Barrier,
    pub control_present: bool,
    pub state_present: bool,
    #[serde(default = "initialized", skip_serializing_if = "is_initialized")]
    pub private_initialized: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub status: Status,
    pub control: Option<SecretString>,
    pub state: Option<SecretString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_preferences: Option<PublicPreferences>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
    Initialize {
        control: Option<SecretString>,
        state: Option<SecretString>,
        barrier: Barrier,
    },
    CompletePrivateImport {
        control: Option<SecretString>,
        state: Option<SecretString>,
        barrier: Barrier,
    },
    PatchPublic { patch: PublicPatch },
    Put {
        namespace: Namespace,
        value: SecretString,
    },
    BeginErase {},
    FinishErase {},
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: String,
    pub digest: String,
    pub revision: u64,
    pub epoch: u64,
    pub verified: bool,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttSnapshot([REDACTED])")
    }
}
impl std::fmt::Debug for Mutation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttMutation([REDACTED])")
    }
}

/// Old canonical snapshots predate public-only initialization and already checked legacy state.
pub(crate) fn initialized() -> bool { true }
pub(crate) fn is_initialized(value: &bool) -> bool { *value }

/// Nonsecret Classic browse intent, never source authority or a playback target.
/// Source/category are caller-computed digests; focus is a bounded Plex ratingKey.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassicBookmark {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}
impl std::fmt::Debug for ClassicBookmark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClassicBookmark([REDACTED])")
    }
}
impl ClassicBookmark {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if !classic_digest(&self.source)
            || self.category.as_ref().is_some_and(|value| !classic_digest(value))
            || self.focus.as_ref().is_some_and(|value| value.is_empty() || value.len() > 256
                || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        { return Err(ErrorCode::Invalid); }
        Ok(())
    }
}
fn classic_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A closed nonsecret schema. No token, profile, nonce or arbitrary settings bag is admitted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPreferences {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_mode: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classic_bookmarks: Vec<ClassicBookmark>,
}
impl Default for PublicPreferences {
    fn default() -> Self {
        Self { version: 1, device_id: None, remote_address: None, interface_mode: None, notifications_enabled: None,
            classic_bookmarks: Vec::new() }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicPatch {
    EnsureDeviceId(String),
    RemoteAddress(String),
    InterfaceMode(u8),
    NotificationsEnabled(bool),
    ClassicBookmark(ClassicBookmark),
}
impl std::fmt::Debug for PublicPreferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PublicPreferences([REDACTED])")
    }
}
impl std::fmt::Debug for PublicPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PublicPatch([REDACTED])")
    }
}
impl PublicPreferences {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if self.version != 1
            || self.device_id.as_ref().is_some_and(|id| !valid_device(id))
            || self.remote_address.as_ref().is_some_and(|url| !public_address(url))
            || self.interface_mode.is_some_and(|mode| mode > 1)
            || self.classic_bookmarks.len() > 16
            || self.classic_bookmarks.iter().enumerate().any(|(index, bookmark)|
                bookmark.validate().is_err() || self.classic_bookmarks[..index].iter()
                    .any(|previous| previous.source == bookmark.source))
        { return Err(ErrorCode::Invalid); }
        Ok(())
    }
    /// Patch only the addressed field. An established installation identity never changes here.
    pub fn apply(&self, patch: &PublicPatch) -> Result<Self, ErrorCode> {
        self.validate()?;
        patch.validate()?;
        let mut next = self.clone();
        match patch {
            PublicPatch::EnsureDeviceId(id) => { next.device_id.get_or_insert_with(|| id.clone()); }
            PublicPatch::RemoteAddress(address) => next.remote_address = Some(address.clone()),
            PublicPatch::InterfaceMode(mode) => next.interface_mode = Some(*mode),
            PublicPatch::NotificationsEnabled(enabled) => next.notifications_enabled = Some(*enabled),
            PublicPatch::ClassicBookmark(bookmark) => {
                next.classic_bookmarks.retain(|previous| previous.source != bookmark.source);
                next.classic_bookmarks.push(bookmark.clone());
                if next.classic_bookmarks.len() > 16 { next.classic_bookmarks.remove(0); }
            }
        }
        Ok(next)
    }
}
impl PublicPatch {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let valid = match self {
            Self::EnsureDeviceId(id) => valid_device(id),
            Self::RemoteAddress(address) => public_address(address),
            Self::InterfaceMode(mode) => *mode <= 1,
            Self::NotificationsEnabled(_) => true,
            Self::ClassicBookmark(bookmark) => bookmark.validate().is_ok(),
        };
        if valid { Ok(()) } else { Err(ErrorCode::Invalid) }
    }
}
pub(super) fn valid_device(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
/// Defensive helper-side shape gate. The app must additionally use its canonical
/// control-address normalizer before admission. This path never accepts credentials,
/// query/fragment data, percent escapes, whitespace or cleartext HTTP.
pub(super) fn public_address(address: &str) -> bool {
    public_address_with_limit(address, 2048)
}
pub(super) fn public_address_with_limit(address: &str, max_bytes: usize) -> bool {
    let Some(rest) = address.strip_prefix("https://") else { return false; };
    if address.len() > max_bytes || !rest.is_ascii()
        || rest.bytes().any(|b| !b.is_ascii_alphanumeric() && !b"-._~:/[]".contains(&b))
    { return false; }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || path.split('/').any(|part| part == "." || part == "..") {
        return false;
    }
    let host_port = if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, port)) = ipv6.split_once(']') else { return false; };
        if host.parse::<std::net::Ipv6Addr>().is_err() { return false; }
        port.strip_prefix(':').map(|port| (host, port))
    } else {
        let Some((host, port)) = authority.rsplit_once(':') else { return false; };
        if host.is_empty() || host.contains(':')
            || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        { return false; }
        Some((host, port))
    };
    host_port.is_some_and(|(_, port)| !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u8,
    sealed: Sealed,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed {
    backend: String,
    key: String,
    iv: String,
    data: String,
}
fn parse_envelope(text: &str) -> Result<Envelope, ErrorCode> {
    if text.len() > ENVELOPE_BYTES {
        return Err(ErrorCode::Invalid);
    }
    let value: Envelope = serde_json::from_str(text).map_err(|_| ErrorCode::Invalid)?;
    let s = &value.sealed;
    if value.version != 1
        || s.backend != "keymanager3"
        || s.key.is_empty()
        || s.key.len() > 256
        || !s
            .key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || s.iv.len() > 256
        || !base64_shape(&s.iv)
        || !base64_shape(&s.data)
    {
        return Err(ErrorCode::Invalid);
    }
    Ok(value)
}
pub(super) fn base64_shape(text: &str) -> bool {
    !text.is_empty() && text.len() % 4 == 0 && {
        let body = text.trim_end_matches('=');
        text.len() - body.len() <= 2
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    }
}
/// Structural validation only: the app retains authenticated decryption authority.
pub fn validate_envelope(text: &str) -> Result<(), ErrorCode> {
    parse_envelope(text).map(|_| ())
}
pub(crate) fn envelope_app(text: &str, namespace: Namespace) -> Result<String, ErrorCode> {
    validate_envelope(text)?;
    let value: Envelope = serde_json::from_str(text).map_err(|_| ErrorCode::Invalid)?;
    let suffix = match namespace {
        Namespace::Control => ".ott.control.v1",
        Namespace::State => ".ott.state.v1",
    };
    value
        .sealed
        .key
        .strip_suffix(suffix)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or(ErrorCode::Invalid)
}

#[cfg(test)]
#[path = "ott_bookmark_tests.rs"]
mod bookmark_tests;

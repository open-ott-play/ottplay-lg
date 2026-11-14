//! Private compositor protocol. This is not the public controller protocol.
//! No request can supply a provider URL, path, credential, or owner identity.
use plx_base::task::{time::Deadline, OffFrame};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::os::raw::c_long;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const VERSION: u8 = 1;
pub const AUDIENCE: &str = "ottplay-multiview-v1";
pub const MAX_CONTROL_BYTES: usize = 16_384;
pub const MAX_RESULT_BYTES: usize = 262_144;

const MAX_SOURCES_PAGE: usize = 128;
const SERVICE_BUDGET_MS: u32 = 30_000;
const MEDIA_LEASE_MAX_MS: u32 = 90_000;

/// Closed errors: callers may display these names, never server bodies or transport diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceError {
    InvalidConfig,
    UnpairedInstallation,
    InvalidRequest,
    InvalidResponse,
    ResponseTooLarge,
    Cancelled,
    Deadline,
    Transport,
    Unauthorized,
    Forbidden,
    RevisionConflict,
    ResourceLimit,
    AuthorityRevoked,
    BackendUnavailable,
    StaleServer,
    Unsupported,
    InvalidGeometry,
    InvalidOutput,
    HttpStatus(u16),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProtectedBinding {
    server_id: String,
    origin: String,
    audience: String,
    installation_id: String,
    control_bearer: String,
}

/// Dedicated compositor credential stored only in the protected sensitive settings value.
/// It is not OTT remote-control auth and cannot be serialized into profiles.
pub(super) struct ServiceBinding {
    server_id: String,
    origin: String,
    installation_id: String,
    bearer: String,
}
impl std::fmt::Debug for ServiceBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiviewServiceBinding")
            .field("enabled", &true)
            .field("server_id", &"<paired>")
            .field("origin", &"<paired HTTPS origin>")
            .field("credential", &"<redacted>")
            .finish()
    }
}
impl ServiceBinding {
    pub fn from_protected_json(raw: &str) -> Result<Option<Self>, ServiceError> {
        if raw.is_empty() {
            return Ok(None);
        }
        if raw.len() > 4096 {
            return Err(ServiceError::InvalidConfig);
        }
        let stored: ProtectedBinding =
            serde_json::from_str(raw).map_err(|_| ServiceError::InvalidConfig)?;
        if stored.audience != AUDIENCE
            || !server_id(&stored.server_id)
            || !opaque(&stored.installation_id)
            || !control_secret(&stored.control_bearer)
        {
            return Err(ServiceError::InvalidConfig);
        }
        let origin = canonical_https_origin(&stored.origin)?;
        Ok(Some(Self {
            server_id: stored.server_id,
            origin,
            installation_id: stored.installation_id,
            bearer: stored.control_bearer,
        }))
    }

    /// Call with the actual persisted remote-control device ID, never a temporary runtime UUID.
    pub fn bind_for_installation(&self, persisted_device_id: &str) -> Result<(), ServiceError> {
        if !opaque(persisted_device_id) || self.installation_id != persisted_device_id {
            return Err(ServiceError::UnpairedInstallation);
        }
        Ok(())
    }

    /// The caller must write this string only through protected sensitive settings storage.
    pub fn protected_json(&self) -> Result<String, ServiceError> {
        let stored = ProtectedBinding {
            server_id: self.server_id.clone(),
            origin: self.origin.clone(),
            audience: AUDIENCE.into(),
            installation_id: self.installation_id.clone(),
            control_bearer: self.bearer.clone(),
        };
        serde_json::to_string(&stored).map_err(|_| ServiceError::InvalidConfig)
    }

    fn url(&self, endpoint: Endpoint) -> String {
        format!("{}{}", self.origin, endpoint.path())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Capabilities,
    Sessions,
    Status,
    Sources,
    InputGrants,
    Control,
}
impl Endpoint {
    const fn path(self) -> &'static str {
        match self {
            Self::Capabilities => "/multiview/v1/capabilities",
            Self::Sessions => "/multiview/v1/sessions",
            Self::Status => "/multiview/v1/status",
            Self::Sources => "/multiview/v1/sources",
            Self::InputGrants => "/multiview/v1/input-grants",
            Self::Control => "/multiview/v1/control",
        }
    }
    const fn method(self) -> &'static str {
        if matches!(self, Self::Capabilities) {
            "GET"
        } else {
            "POST"
        }
    }
    const fn mutating(self) -> bool {
        matches!(self, Self::Sessions | Self::InputGrants | Self::Control)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RequestId(String);
impl RequestId {
    pub fn parse(value: &str) -> Result<Self, ServiceError> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ServiceError::InvalidRequest);
        }
        Ok(Self(value.to_owned()))
    }
}

pub(super) struct ServiceRequest {
    endpoint: Endpoint,
    request_id: Option<RequestId>,
    body: Option<Vec<u8>>,
}
impl ServiceRequest {
    fn get_capabilities() -> Self {
        Self {
            endpoint: Endpoint::Capabilities,
            request_id: None,
            body: None,
        }
    }
    fn post_read<T: Serialize>(endpoint: Endpoint, value: &T) -> Result<Self, ServiceError> {
        if !matches!(endpoint, Endpoint::Status | Endpoint::Sources) {
            return Err(ServiceError::InvalidRequest);
        }
        Self::encode(endpoint, None, value)
    }
    fn post<T: Serialize>(
        endpoint: Endpoint,
        request_id: RequestId,
        value: &T,
    ) -> Result<Self, ServiceError> {
        if !endpoint.mutating() {
            return Err(ServiceError::InvalidRequest);
        }
        Self::encode(endpoint, Some(request_id), value)
    }
    fn encode<T: Serialize>(
        endpoint: Endpoint,
        request_id: Option<RequestId>,
        value: &T,
    ) -> Result<Self, ServiceError> {
        let body = serde_json::to_vec(value).map_err(|_| ServiceError::InvalidRequest)?;
        if body.is_empty() || body.len() > MAX_CONTROL_BYTES {
            return Err(ServiceError::InvalidRequest);
        }
        Ok(Self {
            endpoint,
            request_id,
            body: Some(body),
        })
    }
    pub fn url(&self, binding: &ServiceBinding) -> String {
        binding.url(self.endpoint)
    }
    pub fn method(&self) -> &'static str {
        self.endpoint.method()
    }
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_ref().map(|id| id.0.as_str())
    }
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }
}

pub(super) trait Transport {
    fn send(
        &self,
        worker: &OffFrame,
        binding: &ServiceBinding,
        request: &ServiceRequest,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, ServiceError>;
}

// libcurl uses C long: 32 bits on webOS ARMv7 and 64 bits on desktop hosts.
// Saturate before conversion so an oversized duration cannot disable the timeout.
fn service_timeouts(remaining: Duration) -> plx_net::net::Timeouts {
    plx_net::net::Timeouts {
        connect_s: c_long::try_from(remaining.as_secs().saturating_add(1))
            .unwrap_or(4)
            .clamp(1, 4),
        total_s: 0,
        total_ms: c_long::try_from(remaining.as_millis())
            .unwrap_or(c_long::MAX)
            .max(1),
        low_speed_bps: 0,
        low_speed_s: 0,
    }
}

pub(super) struct VerifiedHttpsTransport;
impl Transport for VerifiedHttpsTransport {
    fn send(
        &self,
        worker: &OffFrame,
        binding: &ServiceBinding,
        request: &ServiceRequest,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, ServiceError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ServiceError::Cancelled);
        }
        let remaining = deadline.remaining(worker);
        if remaining.is_zero() {
            return Err(ServiceError::Deadline);
        }
        let mut headers = vec![format!("Authorization: Bearer {}", binding.bearer)];
        if request.body.is_some() {
            headers.push("Content-Type: application/json".into());
        }
        if let Some(id) = request.request_id() {
            headers.push(format!("X-Request-Id: {id}"));
        }
        let mut body = Vec::new();
        let mut too_large = false;
        let mut on_bytes = |chunk: &[u8]| {
            if body.len().saturating_add(chunk.len()) > MAX_RESULT_BYTES {
                too_large = true;
                return false;
            }
            body.extend_from_slice(chunk);
            true
        };
        let end = plx_net::net::stream_request_tls_with_timeouts(
            &request.url(binding),
            &headers,
            request.method(),
            request.body(),
            plx_net::net::Tls::Ca,
            None,
            service_timeouts(remaining),
            Duration::from_secs(3),
            cancelled,
            &mut on_bytes,
        );
        if too_large {
            return Err(ServiceError::ResponseTooLarge);
        }
        if cancelled.load(Ordering::Acquire) || end == plx_net::net::StreamEnd::Cancelled {
            return Err(ServiceError::Cancelled);
        }
        match end {
            plx_net::net::StreamEnd::Closed => Ok(body),
            plx_net::net::StreamEnd::Status(code) => Err(http_status_error(code)),
            plx_net::net::StreamEnd::Cancelled => Err(ServiceError::Cancelled),
            plx_net::net::StreamEnd::Transport(_) => Err(ServiceError::Transport),
            plx_net::net::StreamEnd::Consumer if too_large => Err(ServiceError::ResponseTooLarge),
            plx_net::net::StreamEnd::Consumer => Err(ServiceError::InvalidResponse),
            plx_net::net::StreamEnd::Idle => Err(ServiceError::Deadline),
            plx_net::net::StreamEnd::Unsupported | plx_net::net::StreamEnd::Refused => {
                Err(ServiceError::Unsupported)
            }
        }
    }
}

fn http_status_error(status: u16) -> ServiceError {
    match status {
        401 => ServiceError::Unauthorized,
        403 => ServiceError::Forbidden,
        409 => ServiceError::RevisionConflict,
        410 => ServiceError::AuthorityRevoked,
        413 | 429 => ServiceError::ResourceLimit,
        502..=504 => ServiceError::BackendUnavailable,
        code => ServiceError::HttpStatus(code),
    }
}

fn server_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 254
        && value
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !b"\\/?#%@".contains(&b))
}
fn control_secret(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn canonical_https_origin(value: &str) -> Result<String, ServiceError> {
    if value.len() > 2048
        || value.trim() != value
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || value.contains(['\\', '?', '#', '@', '%'])
    {
        return Err(ServiceError::InvalidConfig);
    }
    let rest = value
        .strip_prefix("https://")
        .ok_or(ServiceError::InvalidConfig)?;
    if rest.is_empty() || rest.contains('/') || rest.contains('\\') {
        return Err(ServiceError::InvalidConfig);
    }
    let (host, explicit_port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let (raw_host, suffix) = bracketed
            .split_once(']')
            .ok_or(ServiceError::InvalidConfig)?;
        let parsed = raw_host
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| ServiceError::InvalidConfig)?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(parse_explicit_port(
                suffix
                    .strip_prefix(':')
                    .ok_or(ServiceError::InvalidConfig)?,
            )?)
        };
        (format!("[{parsed}]"), port)
    } else {
        let (raw_host, raw_port) = match rest.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return Err(ServiceError::InvalidConfig),
            None => (rest, None),
        };
        let host = canonical_host(raw_host)?;
        let port = raw_port.map(parse_explicit_port).transpose()?;
        (host, port)
    };
    Ok(match explicit_port.filter(|port| *port != 443) {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

fn parse_explicit_port(value: &str) -> Result<u16, ServiceError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ServiceError::InvalidConfig);
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(ServiceError::InvalidConfig)
}

fn canonical_host(value: &str) -> Result<String, ServiceError> {
    if value.is_empty() || value.len() > 253 || value.starts_with('.') || value.ends_with('.') {
        return Err(ServiceError::InvalidConfig);
    }
    if let Ok(ipv4) = value.parse::<std::net::Ipv4Addr>() {
        return Ok(ipv4.to_string());
    }
    let host = value.to_ascii_lowercase();
    if host.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err(ServiceError::InvalidConfig);
    }
    Ok(host)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendExpected {
    pub runtime: u64,
    pub scene: String,
    pub geometry_revision: u64,
    pub focus_revision: u64,
    pub audio_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRevisions {
    pub scene: String,
    pub layout_revision: String,
    pub focus_revision: String,
    pub audio_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeExpected {
    pub runtime: String,
    pub expected: NativeRevisions,
    pub slot_id: u8,
    pub slot_generation: String,
    pub source_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub server_epoch: String,
    pub session_id: String,
    pub backend_expected: BackendExpected,
    pub native_expected: NativeExpected,
    pub backend_slot_revision: u64,
}
impl Binding {
    pub fn valid(&self) -> bool {
        let n = &self.native_expected;
        let e = &n.expected;
        [
            self.server_epoch.as_str(),
            self.session_id.as_str(),
            self.backend_expected.scene.as_str(),
            n.runtime.as_str(),
            e.scene.as_str(),
            e.layout_revision.as_str(),
            e.focus_revision.as_str(),
            e.audio_revision.as_str(),
            n.slot_generation.as_str(),
            n.source_revision.as_str(),
        ]
        .iter()
        .all(|s| opaque(s))
            && n.slot_id < 9
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Protection {
    pub known: bool,
    pub restricted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub title: String,
    pub provider: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogItem {
    pub source_ref: String,
    pub item_ref: String,
    pub item_revision: String,
    pub identity_version: u8,
    pub favorite_sha256: String,
    pub kind: String,
    pub input_adapter: String,
    pub protection: Protection,
    pub descriptor: Descriptor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcesRequest {
    pub version: u8,
    pub context: Binding,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcesResponse {
    pub version: u8,
    pub server_id: String,
    pub context: Binding,
    pub catalog_revision: String,
    pub items: Vec<CatalogItem>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    pub version: u8,
    pub context: Binding,
    /// Optional source-only transition. A returned grant is bound to the exact
    /// committed context, never to a client-invented revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_native: Option<NativeExpected>,
    pub source_ref: String,
    pub item_ref: String,
    pub item_revision: String,
    pub identity_version: u8,
    pub favorite_sha256: String,
    pub catalog_revision: String,
    /// Narrows the server's 30-second maximum; never extends an existing budget.
    pub remaining_budget_ms: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantResponse {
    pub version: u8,
    pub server_id: String,
    pub context: Binding,
    pub item: CatalogItem,
    pub catalog_revision: String,
    pub policy_revision: String,
    pub input_grant: String,
    pub expires_in_ms: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub version: u8,
    pub context: Binding,
    /// Optional operation-scoped transition validated against the closed op union.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_native: Option<NativeExpected>,
    pub op: ControlOp,
}

/// Internally tagged closed variants avoid accepting ignored authority fields.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlOp {
    Play { input_grant: String },
    Renew {},
    Focus {},
    Audio { policy: AudioPolicy },
    Layout { geometry: Geometry },
    Stop {},
    Off {},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioPolicy {
    FollowsFocus,
    Muted,
    Slot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryMode {
    Disjoint,
    Pip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub slot_id: u8,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub z: u8,
}

/// Native supplies exact raster geometry; server validates, never recomputes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub canvas: Canvas,
    pub revision: u64,
    pub mode: GeometryMode,
    pub rects: Vec<Rect>,
}

/// Bootstrap has no prior backend identity. The server allocates it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    pub version: u8,
    pub server_epoch: String,
    pub native_expected: NativeExpected,
    pub output_profile_id: String,
    pub geometry: Geometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProfile {
    pub profile_id: String,
    pub canvas: Canvas,
    pub max_inputs: u8,
    pub geometry_modes: Vec<GeometryMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub version: u8,
    pub server_id: String,
    pub server_epoch: String,
    pub max_inputs: u8,
    pub qualified_input_adapters: Vec<String>,
    pub output_profiles: Vec<OutputProfile>,
    pub output_auth: String,
    pub catalog_revision: String,
}

/// Owner-bound read can recover fresh backend revisions after a lost response.
/// The returned snapshot is not an execution grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusRequest {
    pub version: u8,
    pub server_epoch: String,
    pub session_id: String,
    pub native_expected: NativeExpected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Preparing,
    Ready,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    Empty,
    Preparing,
    Playing,
    Stalled,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotSnapshot {
    pub slot_id: u8,
    pub source_revision: u64,
    pub state: SlotState,
    pub error: Option<String>,
    pub frames: u64,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub source_ref: Option<String>,
    pub item_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioState {
    pub policy: AudioPolicy,
    pub slot_id: Option<u8>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResponse {
    pub version: u8,
    pub server_id: String,
    pub server_epoch: String,
    pub session_id: String,
    pub native_expected: NativeExpected,
    pub backend_expected: BackendExpected,
    pub state: SessionState,
    pub geometry: Geometry,
    pub slots: Vec<SlotSnapshot>,
    pub audio: AudioState,
    pub output: Option<Output>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlResponse {
    pub version: u8,
    pub context: Binding,
    pub accepted: bool,
    pub operation_id: String,
    pub state: SessionState,
}

/// Private response only. Never copy this path into public controller receipts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub session_id: String,
    pub server_epoch: String,
    pub output_generation: String,
    pub playlist_path: String,
    pub lease_expires_in_ms: u32,
}

pub fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
pub fn favorite_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, ServiceError> {
    if body.len() > MAX_RESULT_BYTES {
        return Err(ServiceError::ResponseTooLarge);
    }
    serde_json::from_slice(body).map_err(|_| ServiceError::InvalidResponse)
}

fn safe_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(|c| c.is_control())
}

fn valid_geometry(geometry: &Geometry) -> bool {
    let c = &geometry.canvas;
    if c.width == 0
        || c.width > 3840
        || c.height == 0
        || c.height > 2160
        || c.fps == 0
        || c.fps > 60
        || geometry.revision == 0
        || geometry.rects.is_empty()
        || geometry.rects.len() > 9
    {
        return false;
    }
    for (i, rect) in geometry.rects.iter().enumerate() {
        if rect.slot_id >= 9
            || rect.width == 0
            || rect.height == 0
            || rect.x.checked_add(rect.width).is_none_or(|x| x > c.width)
            || rect.y.checked_add(rect.height).is_none_or(|y| y > c.height)
            || geometry.rects[..i]
                .iter()
                .any(|other| other.slot_id == rect.slot_id || other.z == rect.z)
        {
            return false;
        }
        if geometry.mode == GeometryMode::Disjoint
            && geometry.rects[..i].iter().any(|other| {
                let x_overlap = rect.x < other.x.saturating_add(other.width)
                    && other.x < rect.x.saturating_add(rect.width);
                let y_overlap = rect.y < other.y.saturating_add(other.height)
                    && other.y < rect.y.saturating_add(rect.height);
                x_overlap && y_overlap
            })
        {
            return false;
        }
    }
    true
}

fn valid_capabilities(c: &Capabilities, paired_server: &str) -> Result<(), ServiceError> {
    if c.version != VERSION
        || c.server_id != paired_server
        || !opaque(&c.server_epoch)
        || !opaque(&c.catalog_revision)
        || c.max_inputs > 9
        || c.output_auth != "hmac_path_v1"
        || c.qualified_input_adapters.len() > 16
        || c.qualified_input_adapters
            .iter()
            .any(|a| !safe_token(a, 48))
        || c.qualified_input_adapters
            .iter()
            .enumerate()
            .any(|(i, a)| c.qualified_input_adapters[..i].contains(a))
        || c.output_profiles.len() > 16
    {
        return Err(ServiceError::InvalidResponse);
    }
    let mut ids = std::collections::BTreeSet::new();
    for profile in &c.output_profiles {
        if !opaque(&profile.profile_id)
            || !ids.insert(&profile.profile_id)
            || profile.max_inputs == 0
            || profile.max_inputs > 9
            || profile.max_inputs > c.max_inputs
            || profile.geometry_modes.is_empty()
            || profile.geometry_modes.len() > 2
            || profile.canvas.width == 0
            || profile.canvas.width > 3840
            || profile.canvas.height == 0
            || profile.canvas.height > 2160
            || profile.canvas.fps == 0
            || profile.canvas.fps > 60
        {
            return Err(ServiceError::InvalidResponse);
        }
    }
    if (c.max_inputs == 0) != c.output_profiles.is_empty()
        || (c.max_inputs > 0
            && c.output_profiles
                .iter()
                .map(|p| usize::from(p.max_inputs))
                .max()
                .unwrap_or(0)
                != usize::from(c.max_inputs))
    {
        return Err(ServiceError::InvalidResponse);
    }
    Ok(())
}

fn safe_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

fn valid_item(item: &CatalogItem, adapters: &[String]) -> bool {
    opaque(&item.source_ref)
        && opaque(&item.item_ref)
        && opaque(&item.item_revision)
        && item.identity_version == 1
        && favorite_digest(&item.favorite_sha256)
        && matches!(item.kind.as_str(), "live" | "vod" | "file")
        && safe_token(&item.input_adapter, 48)
        && adapters.iter().any(|a| a == &item.input_adapter)
        && item.protection.known
        && safe_text(&item.descriptor.title, 256)
        && safe_token(&item.descriptor.provider, 48)
}

fn valid_context(context: &Binding, server_epoch: &str, session_id: Option<&str>) -> bool {
    context.valid()
        && context.server_epoch == server_epoch
        && session_id.is_none_or(|session| context.session_id == session)
}

/// A paired private endpoint client. All bodies are closed serde schemas and all
/// URLs are fixed endpoint paths under the exact paired HTTPS origin.
pub(super) struct ServiceClient<T> {
    binding: ServiceBinding,
    transport: T,
}

impl<T: Transport> ServiceClient<T> {
    pub fn new(
        binding: ServiceBinding,
        persisted_device_id: &str,
        transport: T,
    ) -> Result<Self, ServiceError> {
        binding.bind_for_installation(persisted_device_id)?;
        Ok(Self { binding, transport })
    }

    fn call<R: DeserializeOwned>(
        &self,
        worker: &OffFrame,
        request: &ServiceRequest,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<R, ServiceError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ServiceError::Cancelled);
        }
        let remaining = deadline.remaining(worker);
        if remaining.is_zero() {
            return Err(ServiceError::Deadline);
        }
        let bounded = Deadline::after(
            worker,
            remaining.min(Duration::from_millis(u64::from(SERVICE_BUDGET_MS))),
        );
        let bytes = self
            .transport
            .send(worker, &self.binding, request, bounded, cancelled)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(ServiceError::Cancelled);
        }
        if bounded.expired(worker) {
            return Err(ServiceError::Deadline);
        }
        decode(&bytes)
    }

    pub fn capabilities(
        &self,
        worker: &OffFrame,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Capabilities, ServiceError> {
        let request = ServiceRequest::get_capabilities();
        let response: Capabilities = self.call(worker, &request, deadline, cancelled)?;
        valid_capabilities(&response, &self.binding.server_id)?;
        Ok(response)
    }

    pub fn create_session(
        &self,
        worker: &OffFrame,
        capabilities: &Capabilities,
        native_expected: NativeExpected,
        output_profile_id: &str,
        geometry: Geometry,
        request_id: RequestId,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<StatusResponse, ServiceError> {
        valid_capabilities(capabilities, &self.binding.server_id)?;
        let profile = capabilities
            .output_profiles
            .iter()
            .find(|p| p.profile_id == output_profile_id)
            .ok_or(ServiceError::Unsupported)?;
        if profile.canvas != geometry.canvas
            || !profile.geometry_modes.contains(&geometry.mode)
            || !valid_geometry(&geometry)
        {
            return Err(ServiceError::InvalidGeometry);
        }
        if !valid_native_expected(&native_expected) {
            return Err(ServiceError::InvalidRequest);
        }
        let request = SessionCreateRequest {
            version: VERSION,
            server_epoch: capabilities.server_epoch.clone(),
            native_expected: native_expected.clone(),
            output_profile_id: output_profile_id.into(),
            geometry: geometry.clone(),
        };
        let request = ServiceRequest::post(Endpoint::Sessions, request_id, &request)?;
        let response: StatusResponse = self.call(worker, &request, deadline, cancelled)?;
        validate_status(
            &response,
            &self.binding.server_id,
            &capabilities.server_epoch,
            None,
            &native_expected,
            Some(&geometry),
        )?;
        Ok(response)
    }

    pub fn status(
        &self,
        worker: &OffFrame,
        server_epoch: &str,
        session_id: &str,
        native_expected: NativeExpected,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<StatusResponse, ServiceError> {
        if !opaque(server_epoch) || !opaque(session_id) || !valid_native_expected(&native_expected)
        {
            return Err(ServiceError::InvalidRequest);
        }
        let request = StatusRequest {
            version: VERSION,
            server_epoch: server_epoch.into(),
            session_id: session_id.into(),
            native_expected: native_expected.clone(),
        };
        let request = ServiceRequest::post_read(Endpoint::Status, &request)?;
        let response: StatusResponse = self.call(worker, &request, deadline, cancelled)?;
        if !same_native_session(&response.native_expected, &native_expected) {
            return Err(ServiceError::InvalidResponse);
        }
        validate_status(
            &response,
            &self.binding.server_id,
            server_epoch,
            Some(session_id),
            &response.native_expected,
            None,
        )?;
        Ok(response)
    }

    pub fn sources(
        &self,
        worker: &OffFrame,
        context: Binding,
        cursor: Option<String>,
        capabilities: &Capabilities,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<SourcesResponse, ServiceError> {
        valid_capabilities(capabilities, &self.binding.server_id)?;
        validate_binding(
            &context,
            &self.binding.server_id,
            &capabilities.server_epoch,
        )?;
        let request = SourcesRequest {
            version: VERSION,
            context: context.clone(),
            cursor,
        };
        let request = ServiceRequest::post_read(Endpoint::Sources, &request)?;
        let response: SourcesResponse = self.call(worker, &request, deadline, cancelled)?;
        if response.version != VERSION
            || response.server_id != self.binding.server_id
            || response.context != context
            || !opaque(&response.catalog_revision)
            || response.items.len() > MAX_SOURCES_PAGE
            || response.next_cursor.as_deref().is_some_and(|c| !opaque(c))
        {
            return Err(ServiceError::InvalidResponse);
        }
        let mut refs = std::collections::BTreeSet::new();
        if response.items.iter().any(|item| {
            !valid_item(item, &capabilities.qualified_input_adapters)
                || !refs.insert((&item.source_ref, &item.item_ref))
        }) {
            return Err(ServiceError::InvalidResponse);
        }
        Ok(response)
    }

    pub fn input_grant(
        &self,
        worker: &OffFrame,
        context: Binding,
        item: &CatalogItem,
        next_native: Option<NativeExpected>,
        catalog_revision: &str,
        remaining_budget_ms: u32,
        capabilities: &Capabilities,
        request_id: RequestId,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<GrantResponse, ServiceError> {
        valid_capabilities(capabilities, &self.binding.server_id)?;
        if capabilities.max_inputs == 0 {
            return Err(ServiceError::Unsupported);
        }
        validate_binding(
            &context,
            &self.binding.server_id,
            &capabilities.server_epoch,
        )?;
        if !valid_item(item, &capabilities.qualified_input_adapters)
            || !opaque(catalog_revision)
            || remaining_budget_ms == 0
            || remaining_budget_ms > SERVICE_BUDGET_MS
            || next_native
                .as_ref()
                .is_some_and(|next| !valid_grant_transition(&context.native_expected, next))
        {
            return Err(ServiceError::InvalidRequest);
        }
        let mut committed_context = context.clone();
        if let Some(next) = &next_native {
            committed_context.native_expected = next.clone();
        }
        let request = GrantRequest {
            version: VERSION,
            context: context.clone(),
            next_native,
            source_ref: item.source_ref.clone(),
            item_ref: item.item_ref.clone(),
            item_revision: item.item_revision.clone(),
            identity_version: item.identity_version,
            favorite_sha256: item.favorite_sha256.clone(),
            catalog_revision: catalog_revision.into(),
            remaining_budget_ms,
        };
        let request = ServiceRequest::post(Endpoint::InputGrants, request_id, &request)?;
        let response: GrantResponse = self.call(worker, &request, deadline, cancelled)?;
        if response.version != VERSION
            || response.server_id != self.binding.server_id
            || response.context != committed_context
            || response.item != *item
            || response.catalog_revision != catalog_revision
            || !opaque(&response.policy_revision)
            || !opaque(&response.input_grant)
            || response.expires_in_ms == 0
            || response.expires_in_ms > remaining_budget_ms.min(SERVICE_BUDGET_MS)
        {
            return Err(ServiceError::InvalidResponse);
        }
        Ok(response)
    }

    pub fn control(
        &self,
        worker: &OffFrame,
        context: Binding,
        op: ControlOp,
        next_native: Option<NativeExpected>,
        capabilities: &Capabilities,
        request_id: RequestId,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<ControlResponse, ServiceError> {
        valid_capabilities(capabilities, &self.binding.server_id)?;
        if capabilities.max_inputs == 0 && matches!(&op, ControlOp::Play { .. }) {
            return Err(ServiceError::Unsupported);
        }
        validate_binding(
            &context,
            &self.binding.server_id,
            &capabilities.server_epoch,
        )?;
        validate_control_op(&op)?;
        if next_native
            .as_ref()
            .is_some_and(|next| !valid_control_transition(&context.native_expected, next, &op))
        {
            return Err(ServiceError::InvalidRequest);
        }
        let operation = op.clone();
        let request = ControlRequest {
            version: VERSION,
            context: context.clone(),
            next_native: next_native.clone(),
            op,
        };
        let request = ServiceRequest::post(Endpoint::Control, request_id, &request)?;
        let response: ControlResponse = self.call(worker, &request, deadline, cancelled)?;
        let expected_native = if response.accepted {
            next_native.as_ref().unwrap_or(&context.native_expected)
        } else {
            &context.native_expected
        };
        if response.version != VERSION
            || validate_binding(
                &response.context,
                &self.binding.server_id,
                &context.server_epoch,
            )
            .is_err()
            || response.context.server_epoch != context.server_epoch
            || response.context.session_id != context.session_id
            || if response.accepted && matches!(operation, ControlOp::Play { .. }) {
                !valid_accepted_play_context(&context, &response.context)
            } else {
                response.context.native_expected != *expected_native
            }
            || !valid_control_result_backend(
                &context,
                &response.context,
                &operation,
                response.accepted,
            )
            || !opaque(&response.operation_id)
        {
            return Err(ServiceError::InvalidResponse);
        }
        Ok(response)
    }

    /// Convert an authenticated ready status into private media-worker authority without
    /// exposing the paired bearer or putting the signed lease in a public receipt.
    pub fn private_output(
        &self,
        capabilities: &Capabilities,
        status: &StatusResponse,
    ) -> Result<PrivateOutput, ServiceError> {
        PrivateOutput::from_ready_status(&self.binding, capabilities, status)
    }
}

fn valid_native_expected(n: &NativeExpected) -> bool {
    n.slot_id < 9
        && opaque(&n.runtime)
        && opaque(&n.expected.scene)
        && opaque(&n.expected.layout_revision)
        && opaque(&n.expected.focus_revision)
        && opaque(&n.expected.audio_revision)
        && opaque(&n.slot_generation)
        && opaque(&n.source_revision)
}

fn same_native_session(a: &NativeExpected, b: &NativeExpected) -> bool {
    a.runtime == b.runtime && a.expected.scene == b.expected.scene && a.slot_id == b.slot_id
}

fn valid_grant_transition(prior: &NativeExpected, next: &NativeExpected) -> bool {
    same_native_session(prior, next)
        && prior.expected == next.expected
        && valid_native_expected(next)
}

fn valid_control_transition(prior: &NativeExpected, next: &NativeExpected, op: &ControlOp) -> bool {
    if !same_native_session(prior, next) || !valid_native_expected(next) {
        return false;
    }
    let same_source = prior.slot_generation == next.slot_generation
        && prior.source_revision == next.source_revision;
    match op {
        ControlOp::Play { .. } => false,
        ControlOp::Focus {} => {
            prior.expected.layout_revision == next.expected.layout_revision
                && same_source
                && (prior.expected.focus_revision != next.expected.focus_revision
                    || prior.expected.audio_revision == next.expected.audio_revision)
        }
        ControlOp::Audio { .. } => {
            prior.expected.layout_revision == next.expected.layout_revision
                && prior.expected.focus_revision == next.expected.focus_revision
                && same_source
        }
        ControlOp::Layout { .. } => {
            prior.expected.focus_revision == next.expected.focus_revision
                && prior.expected.audio_revision == next.expected.audio_revision
                && same_source
        }
        ControlOp::Stop {} => prior.expected == next.expected,
        ControlOp::Renew {} | ControlOp::Off {} => prior == next,
    }
}

/// Only the acknowledgement of an already exactly redeemed source grant may
/// reflect concurrent focus/audio changes. The source and raster remain fixed.
fn valid_accepted_play_context(prior: &Binding, next: &Binding) -> bool {
    let a = &prior.native_expected;
    let b = &next.native_expected;
    let old = &prior.backend_expected;
    let new = &next.backend_expected;
    let revision_pair = |before: u64, after: u64, old_token: &str, new_token: &str| {
        after >= before && (after != before || old_token == new_token)
    };
    prior.server_epoch == next.server_epoch
        && prior.session_id == next.session_id
        && old.runtime == new.runtime
        && old.scene == new.scene
        && old.geometry_revision == new.geometry_revision
        && same_native_session(a, b)
        && a.expected.layout_revision == b.expected.layout_revision
        && a.slot_generation == b.slot_generation
        && a.source_revision == b.source_revision
        && prior.backend_slot_revision.checked_add(1) == Some(next.backend_slot_revision)
        && revision_pair(
            old.focus_revision,
            new.focus_revision,
            &a.expected.focus_revision,
            &b.expected.focus_revision,
        )
        && revision_pair(
            old.audio_revision,
            new.audio_revision,
            &a.expected.audio_revision,
            &b.expected.audio_revision,
        )
}

fn valid_control_result_backend(
    prior: &Binding,
    next: &Binding,
    op: &ControlOp,
    accepted: bool,
) -> bool {
    if !accepted {
        return prior == next;
    }
    let a = &prior.backend_expected;
    let b = &next.backend_expected;
    let same_session = a.runtime == b.runtime && a.scene == b.scene;
    if !same_session {
        return false;
    }
    match op {
        ControlOp::Focus {} => {
            a.geometry_revision == b.geometry_revision
                && prior.backend_slot_revision == next.backend_slot_revision
        }
        ControlOp::Audio { .. } => {
            a.geometry_revision == b.geometry_revision
                && a.focus_revision == b.focus_revision
                && prior.backend_slot_revision == next.backend_slot_revision
        }
        ControlOp::Layout { .. } => {
            a.focus_revision == b.focus_revision
                && a.audio_revision == b.audio_revision
                && prior.backend_slot_revision == next.backend_slot_revision
        }
        ControlOp::Play { .. } => valid_accepted_play_context(prior, next),
        ControlOp::Stop {} => {
            a.geometry_revision == b.geometry_revision
                && a.focus_revision == b.focus_revision
                && a.audio_revision == b.audio_revision
        }
        ControlOp::Renew {} | ControlOp::Off {} => {
            prior.backend_expected == next.backend_expected
                && prior.backend_slot_revision == next.backend_slot_revision
        }
    }
}

fn validate_binding(
    binding: &Binding,
    paired_server_id: &str,
    epoch: &str,
) -> Result<(), ServiceError> {
    if !valid_context(binding, epoch, None) || !server_id(paired_server_id) {
        return Err(ServiceError::StaleServer);
    }
    Ok(())
}

fn validate_status(
    status: &StatusResponse,
    server_id: &str,
    epoch: &str,
    session: Option<&str>,
    native: &NativeExpected,
    requested_geometry: Option<&Geometry>,
) -> Result<(), ServiceError> {
    if status.version != VERSION
        || status.server_id != server_id
        || status.server_epoch != epoch
        || !opaque(&status.session_id)
        || session.is_some_and(|s| s != status.session_id)
        || status.native_expected != *native
        || !valid_geometry(&status.geometry)
        || requested_geometry.is_some_and(|g| g != &status.geometry)
        || status.slots.len() > 9
        || status.slots.iter().any(|s| s.slot_id >= 9)
        || status
            .slots
            .iter()
            .enumerate()
            .any(|(i, s)| status.slots[..i].iter().any(|p| p.slot_id == s.slot_id))
        || status.slots.iter().any(|s| {
            s.source_ref.as_deref().is_some_and(|v| !opaque(v))
                || s.item_ref.as_deref().is_some_and(|v| !opaque(v))
                || (s.source_ref.is_some() != s.item_ref.is_some())
                || (s.state == SlotState::Playing && s.frames == 0)
                || s.error.as_deref().is_some_and(|v| !safe_token(v, 48))
        })
        || status.audio.slot_id.is_some_and(|slot| slot >= 9)
        || (status.audio.policy == AudioPolicy::Slot && status.audio.slot_id.is_none())
        || (status.state == SessionState::Ready && status.output.is_none())
    {
        return Err(ServiceError::InvalidResponse);
    }
    if let Some(output) = &status.output {
        if output.session_id != status.session_id
            || output.server_epoch != epoch
            || !opaque(&output.output_generation)
            || output.lease_expires_in_ms == 0
            || output.lease_expires_in_ms > MEDIA_LEASE_MAX_MS
            || !valid_output_path(&output.playlist_path)
        {
            return Err(ServiceError::InvalidOutput);
        }
    }
    if !opaque(&status.backend_expected.scene) {
        return Err(ServiceError::InvalidResponse);
    }
    Ok(())
}

fn valid_output_path(path: &str) -> bool {
    let Some(tail) = path.strip_prefix("/multiview/v1/media/") else {
        return false;
    };
    let Some(lease) = tail.strip_suffix("/index.m3u8") else {
        return false;
    };
    let bytes = lease.as_bytes();
    bytes.len() == 97
        && bytes[32] == b'.'
        && bytes[..32].iter().all(u8::is_ascii_hexdigit)
        && bytes[33..].iter().all(u8::is_ascii_hexdigit)
        && path == format!("/multiview/v1/media/{lease}/index.m3u8")
}

fn validate_control_op(op: &ControlOp) -> Result<(), ServiceError> {
    match op {
        ControlOp::Play { input_grant } if !opaque(input_grant) => {
            Err(ServiceError::InvalidRequest)
        }
        ControlOp::Layout { geometry } if !valid_geometry(geometry) => {
            Err(ServiceError::InvalidGeometry)
        }
        _ => Ok(()),
    }
}

/// Private media output authority. Debug intentionally never prints its HMAC path.
pub(super) struct PrivateOutput {
    origin: String,
    lease: String,
    session_id: String,
    server_epoch: String,
    output_generation: String,
}
impl std::fmt::Debug for PrivateOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateMultiviewOutput")
            .field("session_id", &self.session_id)
            .field("server_epoch", &self.server_epoch)
            .field("output_generation", &self.output_generation)
            .field("playlist", &"<private HMAC lease>")
            .finish()
    }
}
impl PrivateOutput {
    pub fn from_ready_status(
        binding: &ServiceBinding,
        capabilities: &Capabilities,
        status: &StatusResponse,
    ) -> Result<Self, ServiceError> {
        valid_capabilities(capabilities, &binding.server_id)?;
        if capabilities.output_auth != "hmac_path_v1" || status.state != SessionState::Ready {
            return Err(ServiceError::InvalidOutput);
        }
        validate_status(
            status,
            &binding.server_id,
            &capabilities.server_epoch,
            Some(&status.session_id),
            &status.native_expected,
            None,
        )?;
        let output = status.output.as_ref().ok_or(ServiceError::InvalidOutput)?;
        if !valid_output_path(&output.playlist_path) {
            return Err(ServiceError::InvalidOutput);
        }
        let tail = output
            .playlist_path
            .strip_prefix("/multiview/v1/media/")
            .ok_or(ServiceError::InvalidOutput)?;
        let lease = tail
            .strip_suffix("/index.m3u8")
            .ok_or(ServiceError::InvalidOutput)?;
        Ok(Self {
            origin: binding.origin.clone(),
            lease: lease.into(),
            session_id: status.session_id.clone(),
            server_epoch: capabilities.server_epoch.clone(),
            output_generation: output.output_generation.clone(),
        })
    }

    /// Use only inside the media worker; do not place the returned URL in logs, UI, or RPC receipts.
    pub fn playlist_url_for_media_worker(&self) -> String {
        format!(
            "{}/multiview/v1/media/{}/index.m3u8",
            self.origin, self.lease
        )
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_multiview_service_tests.rs"]
mod service_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_timeouts_preserve_deadlines_in_the_target_c_long_range() {
        for (remaining, connect_s, total_ms) in [
            (Duration::ZERO, 1, 1),
            (Duration::from_nanos(1), 1, 1),
            (Duration::from_millis(1500), 2, 1500),
            (Duration::from_secs(30), 4, 30_000),
            (Duration::MAX, 4, c_long::MAX),
        ] {
            let timeouts = service_timeouts(remaining);
            assert_eq!(timeouts.connect_s, connect_s);
            assert_eq!(timeouts.total_ms, total_ms);
            assert_eq!(timeouts.total_s, 0);
            assert_eq!(timeouts.low_speed_bps, 0);
            assert_eq!(timeouts.low_speed_s, 0);
        }
    }

    #[test]
    fn operation_variants_reject_unrecognized_authority_fields() {
        for raw in [
            r#"{"name":"renew","url":"https://example.test"}"#,
            r#"{"name":"play","input_grant":"g","owner":"other"}"#,
            r#"{"name":"play","input_grant":"g","input_grant":"h"}"#,
        ] {
            assert!(serde_json::from_str::<ControlOp>(raw).is_err());
        }
        assert!(matches!(
            serde_json::from_str::<ControlOp>(r#"{"name":"renew"}"#),
            Ok(ControlOp::Renew {})
        ));
    }
    #[test]
    fn identity_and_opaque_encodings_are_canonical_and_bounded() {
        assert!(favorite_digest(&"a".repeat(64)));
        assert!(!favorite_digest(&"A".repeat(64)));
        assert!(!opaque("../secret"));
        assert!(!opaque(&"a".repeat(129)));
        assert!(opaque("source_A.1"));
    }
}

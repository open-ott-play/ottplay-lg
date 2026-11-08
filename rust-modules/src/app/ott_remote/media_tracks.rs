//! Pure current-playback track protocol. No capability, I/O, intent reducer or
//! backend confirmation lives here. Native choices and all identities come from
//! the application owner; decoded IDs are never interpreted as native indices.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

const MAX_REQUEST_BYTES: usize = 2048;
const MAX_RESULT_BYTES: usize = 65_536;
pub(crate) const MAX_TRACKS: usize = 128;
pub(crate) const MAX_TRACKS_PER_KIND: usize = 64;
pub(crate) const MAX_LABEL_BYTES: usize = 128;
pub(crate) const MAX_METADATA_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 64;
const CATALOG_TTL_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Audio,
    Subtitle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum Target {
    Track { kind: Kind, id: String },
    Off { kind: Kind },
}
impl Target {
    fn valid(&self) -> bool {
        match self {
            Self::Track { id, .. } => opaque(id),
            Self::Off { kind } => *kind == Kind::Subtitle,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Params {
    List {
        runtime: String,
    },
    Select {
        runtime: String,
        playback: String,
        source_generation: String,
        catalog: String,
        target: Target,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    action: String,
    params: Params,
}

/// Opaque parsed request; callers cannot construct unchecked wire fields.
#[derive(Clone, Debug)]
pub struct Request(Params);
impl Request {
    /// Decode the complete action/params object so byte limits and duplicate
    /// keys are checked before any Value parser could discard that information.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Error::InvalidRequest);
        }
        let envelope: Envelope =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidRequest)?;
        if envelope.action != "media_tracks" {
            return Err(Error::InvalidRequest);
        }
        let request = Self(envelope.params);
        if !runtime_id(request.runtime()) {
            return Err(Error::InvalidRequest);
        }
        if let Params::Select {
            playback,
            source_generation,
            catalog,
            target,
            ..
        } = &request.0
        {
            if !opaque(playback)
                || !opaque(source_generation)
                || !opaque(catalog)
                || !target.valid()
            {
                return Err(Error::InvalidRequest);
            }
        }
        Ok(request)
    }
    pub fn operation(&self) -> &'static str {
        match self.0 {
            Params::List { .. } => "list",
            Params::Select { .. } => "select",
        }
    }
    pub fn runtime(&self) -> &str {
        match &self.0 {
            Params::List { runtime } | Params::Select { runtime, .. } => runtime,
        }
    }
    pub fn error_response(&self, error: Error) -> Response {
        let mut data = json!({"version":1,"runtime":self.runtime(),"op":self.operation(),
            "state":"error","error":error.code()});
        if let Params::Select {
            playback,
            source_generation,
            catalog,
            target,
            ..
        } = &self.0
        {
            data["playback"] = json!(playback);
            data["source_generation"] = json!(source_generation);
            data["catalog"] = json!(catalog);
            data["target"] = json!(target);
        }
        // Validated fixed-size fields make error responses strictly < 2 KiB.
        Response {
            status: if error == Error::Unsupported {
                "unsupported"
            } else {
                "rejected"
            },
            data,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    RuntimeMismatch,
    NoPlayback,
    PlaybackChanged,
    CatalogChanged,
    CatalogIncomplete,
    TrackNotFound,
    NotSelectable,
    Restricted,
    Unsupported,
    Busy,
    Unavailable,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::RuntimeMismatch => "runtime_mismatch",
            Self::NoPlayback => "no_playback",
            Self::PlaybackChanged => "playback_changed",
            Self::CatalogChanged => "catalog_changed",
            Self::CatalogIncomplete => "catalog_incomplete",
            Self::TrackNotFound => "track_not_found",
            Self::NotSelectable => "not_selectable",
            Self::Restricted => "restricted",
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The status is part of the protocol: unsupported must not masquerade as a
/// rejected response, and arbitrary backend exception text is never projected.
#[derive(Clone, Debug)]
pub struct Response {
    status: &'static str,
    data: Value,
}
impl Response {
    pub fn into_envelope(self) -> Value {
        json!({"status":self.status,"data":self.data})
    }
    #[cfg(test)]
    pub fn data(&self) -> &Value {
        &self.data
    }
}

/// K is the application's actual logical-playback + concrete-source + complete
/// inventory/policy identity. Wire nonces alone must not substitute for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context<K> {
    owner: K,
    runtime: String,
    playback: String,
    source_generation: String,
}
impl<K> Context<K> {
    pub fn new(
        owner: K,
        runtime: String,
        playback: String,
        source_generation: String,
    ) -> Result<Self, Error> {
        if !runtime_id(&runtime) || !opaque(&playback) || !opaque(&source_generation) {
            return Err(Error::Unavailable);
        }
        Ok(Self {
            owner,
            runtime,
            playback,
            source_generation,
        })
    }
}

/// C is an already-resolved native choice. None means a visible but unavailable
/// row; the wire never contains C, a native index, provider ID or resource URL.
pub struct Track<C> {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub choice: Option<C>,
}
pub enum Inventory<C> {
    Incomplete,
    Complete {
        tracks: Vec<Track<C>>,
        subtitle_off: Option<C>,
    },
}

#[derive(Serialize)]
struct WireTrack {
    id: String,
    kind: Kind,
    label: String,
    language: Option<String>,
    codec: Option<String>,
    selectable: bool,
}
struct Row<C> {
    wire: WireTrack,
    choice: Option<C>,
}
struct Snapshot<K, C> {
    context: Context<K>,
    catalog: String,
    tracks: Vec<Row<C>>,
    subtitle_off: Option<C>,
}
struct Current<K, C> {
    snapshot: Arc<Snapshot<K, C>>,
    lease_expires_ms: Option<u64>,
}

/// Exactly one current catalog, with no delivery queue or selection-intent epoch.
/// Old snapshots can only be retained by explicit Selection values held by the
/// existing application intent owner; replacing this slot invalidates them.
pub struct Catalog<K, C> {
    current: Option<Current<K, C>>,
}
impl<K, C> Default for Catalog<K, C> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<K: Eq, C> Catalog<K, C> {
    /// Install a complete passive inventory. Token minting is external. Catalog
    /// and row tokens must never be reused for different mappings. An unchanged
    /// snapshot is renewed by list(), not by reinstalling it. Reporting a new
    /// incomplete/invalid inventory retires the previous receipt even on error.
    pub fn install(
        &mut self,
        live: &Context<K>,
        context: Context<K>,
        catalog: String,
        inventory: Inventory<C>,
    ) -> Result<(), Error> {
        // Results retain the context captured when ordinary discovery started.
        // A stale completion must never retire the current owner's snapshot.
        if &context != live {
            return Err(Error::CatalogChanged);
        }
        let previous = self.current.take();
        if !opaque(&catalog) {
            return Err(Error::Unavailable);
        }
        let Inventory::Complete {
            tracks,
            subtitle_off,
        } = inventory
        else {
            return Err(Error::CatalogIncomplete);
        };
        if tracks.len() > MAX_TRACKS {
            return Err(Error::CatalogIncomplete);
        }
        if previous
            .as_ref()
            .is_some_and(|old| old.snapshot.catalog == catalog)
        {
            return Err(Error::CatalogChanged);
        }
        let mut ids = HashSet::new();
        let mut counts = [0usize; 2];
        let mut rows = Vec::with_capacity(tracks.len());
        for (index, row) in tracks.into_iter().enumerate() {
            if !opaque(&row.id) || !ids.insert(row.id.clone()) {
                return Err(Error::CatalogIncomplete);
            }
            let count = &mut counts[if row.kind == Kind::Audio { 0 } else { 1 }];
            *count += 1;
            if *count > MAX_TRACKS_PER_KIND {
                return Err(Error::CatalogIncomplete);
            }
            rows.push(Row {
                wire: WireTrack {
                    id: row.id,
                    kind: row.kind,
                    label: label(&row.label, index + 1),
                    language: metadata(row.language, false),
                    codec: metadata(row.codec, true),
                    selectable: row.choice.is_some(),
                },
                choice: row.choice,
            });
        }
        let snapshot = Arc::new(Snapshot {
            context,
            catalog,
            tracks: rows,
            subtitle_off,
        });
        // Refuse the whole inventory if the complete wire data cannot fit.
        bounded(list_data(&snapshot))?;
        self.current = Some(Current {
            snapshot,
            lease_expires_ms: None,
        });
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.current = None;
    }

    /// Listing is passive; a successful serialization renews this receipt only.
    pub fn list(
        &mut self,
        request: &Request,
        live: Option<&Context<K>>,
        now_ms: u64,
    ) -> Result<Response, Error> {
        if !matches!(request.0, Params::List { .. }) {
            return Err(Error::InvalidRequest);
        }
        let live = check_runtime(request, live)?;
        let current = self.current.as_mut().ok_or(Error::CatalogIncomplete)?;
        if &current.snapshot.context != live {
            return Err(Error::CatalogIncomplete);
        }
        let expires = now_ms
            .checked_add(CATALOG_TTL_MS)
            .ok_or(Error::Unavailable)?;
        let response = bounded(list_data(&current.snapshot))?;
        current.lease_expires_ms = Some(expires);
        Ok(response)
    }

    /// Resolve the exact immutable mapping without issuing an intent or effect.
    /// The application must admit through its shared manual/remote intent owner,
    /// then recheck immediately before an effect, including an accepted no-op.
    pub fn select(
        &self,
        request: &Request,
        live: Option<&Context<K>>,
        now_ms: u64,
        request_expires_ms: u64,
    ) -> Result<Selection<K, C>, Error> {
        let Params::Select {
            playback,
            source_generation,
            catalog,
            target,
            ..
        } = &request.0
        else {
            return Err(Error::InvalidRequest);
        };
        let live = check_runtime(request, live)?;
        if playback != &live.playback || source_generation != &live.source_generation {
            return Err(Error::PlaybackChanged);
        }
        let current = self.current.as_ref().ok_or(Error::CatalogIncomplete)?;
        if &current.snapshot.context != live || catalog != &current.snapshot.catalog {
            return Err(Error::CatalogChanged);
        }
        let expires = current.lease_expires_ms.ok_or(Error::CatalogChanged)?;
        if now_ms >= expires {
            return Err(Error::CatalogChanged);
        }
        if now_ms >= request_expires_ms {
            return Err(Error::Unavailable);
        }
        let slot = match target {
            Target::Off { .. } => {
                if current.snapshot.subtitle_off.is_none() {
                    return Err(Error::NotSelectable);
                }
                Slot::Off
            }
            Target::Track { kind, id } => {
                let index = current
                    .snapshot
                    .tracks
                    .iter()
                    .position(|row| row.wire.id == *id && row.wire.kind == *kind)
                    .ok_or(Error::TrackNotFound)?;
                if current.snapshot.tracks[index].choice.is_none() {
                    return Err(Error::NotSelectable);
                }
                Slot::Track(index)
            }
        };
        Ok(Selection {
            snapshot: current.snapshot.clone(),
            slot,
            target: target.clone(),
            deadline_ms: expires.min(request_expires_ms),
        })
    }

    /// Revalidate only the captured mapping. Never resolve an old row ID against
    /// a new catalog. Own-rebuild authority transfer is the external intent
    /// owner's responsibility and is deliberately not provided by this method.
    pub fn recheck(
        &self,
        selection: &Selection<K, C>,
        live: Option<&Context<K>>,
        now_ms: u64,
    ) -> Result<(), Error> {
        let live = live.ok_or(Error::NoPlayback)?;
        if selection.snapshot.context.runtime != live.runtime {
            return Err(Error::RuntimeMismatch);
        }
        if selection.snapshot.context.playback != live.playback
            || selection.snapshot.context.source_generation != live.source_generation
        {
            return Err(Error::PlaybackChanged);
        }
        if now_ms >= selection.deadline_ms {
            return Err(Error::Unavailable);
        }
        let current = self.current.as_ref().ok_or(Error::CatalogChanged)?;
        if &selection.snapshot.context != live
            || !Arc::ptr_eq(&current.snapshot, &selection.snapshot)
        {
            return Err(Error::CatalogChanged);
        }
        Ok(())
    }
}

enum Slot {
    Track(usize),
    Off,
}
/// A resolved choice, not an accepted intent. It owns a fixed deadline captured
/// before any later list renewal. This type has no backend or persistence API.
pub struct Selection<K, C> {
    snapshot: Arc<Snapshot<K, C>>,
    slot: Slot,
    target: Target,
    deadline_ms: u64,
}
impl<K, C> Selection<K, C> {
    pub fn choice(&self) -> &C {
        match self.slot {
            Slot::Track(index) => self.snapshot.tracks[index]
                .choice
                .as_ref()
                .expect("validated immutable choice"),
            Slot::Off => self
                .snapshot
                .subtitle_off
                .as_ref()
                .expect("validated immutable off choice"),
        }
    }
    /// The captured deadline also bounds deferred application-side admission.
    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
    /// Call only after the application's shared intent reducer has accepted.
    /// This acknowledges a request, never a decoder/renderer observation.
    pub fn requested_response(&self) -> Response {
        let context = &self.snapshot.context;
        Response {
            status: "ok",
            data: json!({"version":1,"runtime":context.runtime,"op":"select",
            "scope":"current_playback","playback":context.playback,
            "source_generation":context.source_generation,"catalog":self.snapshot.catalog,
            "target":self.target,"accepted":true,"outcome":"requested","observed":null}),
        }
    }
}

fn check_runtime<'a, K>(
    request: &Request,
    live: Option<&'a Context<K>>,
) -> Result<&'a Context<K>, Error> {
    let live = live.ok_or(Error::NoPlayback)?;
    if request.runtime() != live.runtime {
        return Err(Error::RuntimeMismatch);
    }
    Ok(live)
}
fn list_data<K, C>(snapshot: &Snapshot<K, C>) -> Value {
    let context = &snapshot.context;
    json!({"version":1,"runtime":context.runtime,"op":"list","scope":"current_playback",
        "playback":context.playback,"source_generation":context.source_generation,"catalog":snapshot.catalog,
        "tracks":snapshot.tracks.iter().map(|row| &row.wire).collect::<Vec<_>>(),
        "subtitle_off_supported":snapshot.subtitle_off.is_some(),
        "observed":{"audio":{"state":"not_observed","track_id":null,"age_ms":null},
                    "subtitle":{"state":"not_observed","track_id":null,"age_ms":null}}})
}
fn bounded(data: Value) -> Result<Response, Error> {
    if serde_json::to_vec(&data)
        .map_err(|_| Error::Unavailable)?
        .len()
        > MAX_RESULT_BYTES
    {
        return Err(Error::CatalogIncomplete);
    }
    Ok(Response { status: "ok", data })
}
fn runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn metadata(value: Option<String>, codec: bool) -> Option<String> {
    value.filter(|value| {
        !value.is_empty()
            && value.len() <= MAX_METADATA_BYTES
            && value.bytes().all(|b| {
                b.is_ascii_alphanumeric() || b == b'-' || (codec && (b == b'.' || b == b'_'))
            })
    })
}
fn label(raw: &str, ordinal: usize) -> String {
    // Validate the entire input before the shared redactor can trim/truncate it.
    // Track v1 also forbids every Cc/Cf scalar and embedded private syntax.
    // Cs cannot occur in Rust &str. Keep the shared, sometimes stricter redaction.
    if raw.len() > MAX_LABEL_BYTES
        || raw.chars().any(|c| c.is_control() || label_format(c))
        || label_private_syntax(raw)
    {
        return format!("Track {ordinal}");
    }
    if let Some(safe) = super::display::text(raw, MAX_LABEL_BYTES) {
        if !label_private_syntax(&safe) {
            return safe;
        }
    }
    format!("Track {ordinal}")
}

fn label_format(c: char) -> bool {
    // Complete Cf ranges from Unicode 17.0 DerivedGeneralCategory.txt:
    // https://www.unicode.org/Public/17.0.0/ucd/extracted/DerivedGeneralCategory.txt
    // Preserve the previous conservative rejection of unassigned U+2065 too.
    matches!(c,
        '\u{00ad}' | '\u{0600}'..='\u{0605}' | '\u{061c}' | '\u{06dd}' |
        '\u{070f}' | '\u{0890}'..='\u{0891}' | '\u{08e2}' | '\u{180e}' |
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' |
        '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}' |
        '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' |
        '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

fn label_private_syntax(value: &str) -> bool {
    // The CLI uses ASCII case/boundaries for PRIVATE_LABEL. Work on bytes for
    // path delimiters so non-ASCII titles remain intact; callers cap input first.
    let value = value.to_ascii_lowercase();
    let bytes = value.as_bytes();
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
            return true;
        }
    }
    // General :// resources are already rejected by display::text. These
    // schemes also match without //, and Authorization accepts = as well as :.
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
        for (offset, _) in value.match_indices(key) {
            if offset > 0
                && (bytes[offset - 1].is_ascii_alphanumeric() || bytes[offset - 1] == b'_')
            {
                continue;
            }
            if key.ends_with(':')
                || value[offset + key.len()..]
                    .trim_start()
                    .starts_with([':', '='])
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "media_tracks_tests.rs"]
mod tests;

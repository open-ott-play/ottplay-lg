//! Pure Plex wire validation and ordered temporary playback. The common media
//! owner supplies both tokens; this module has no registry, IO, counter, timer,
//! retry cache, persistence, native playback, or independent global media owner.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

// Also the conservative batch bound used by internal restores. The existing
// ReadSession caps a metadata call at this same 500-item public preparation size.
pub(super) const ITEM_LIMIT: usize = 500;
const LIBRARY_LIMIT: usize = 1000;
const METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUE_STATE_BYTES: usize = 16 * 1024;
const MAX_QUEUE_STATE_V2_BYTES: usize = 32 * 1024;
const QUEUE_STATE_OPERATIONS: [&str; 5] = ["play", "status", "next", "previous", "stop"];

/// Read-state format only. Both versions retain the 500-item public write cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StateVersion {
    V1,
    V2,
}
impl StateVersion {
    fn number(self) -> u8 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
    fn item_limit(self) -> usize {
        match self {
            Self::V1 => ITEM_LIMIT,
            Self::V2 => plx_data::ott::favorites::ENTRY_COUNT,
        }
    }
    fn byte_limit(self) -> usize {
        match self {
            Self::V1 => MAX_QUEUE_STATE_BYTES,
            Self::V2 => MAX_QUEUE_STATE_V2_BYTES,
        }
    }
}

/// Absence is legacy v1. The extension is valid only for queue operations that
/// produce state; previews and library responses keep their existing schemas.
pub(super) fn state_version(action: &str, params: &Value) -> Result<StateVersion, WireError> {
    let params = params.as_object().ok_or(WireError::InvalidRequest)?;
    let Some(value) = params.get("state_version") else {
        return Ok(StateVersion::V1);
    };
    if action != "plex_queue"
        || !params.get("op").and_then(Value::as_str)
            .is_some_and(|operation| operation == "repeat" || QUEUE_STATE_OPERATIONS.contains(&operation))
        || value.as_u64() != Some(2)
    {
        return Err(WireError::InvalidRequest);
    }
    Ok(StateVersion::V2)
}

/// Opaque values assigned by the shared Local/Plex umbrella, never minted here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OwnerToken(pub u64);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct PlaybackToken(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DecimalId(String);
impl DecimalId {
    pub fn parse(value: &str) -> Result<Self, WireError> {
        if value.is_empty()
            || value.len() > 20
            || !matches!(value.as_bytes()[0], b'1'..=b'9')
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(WireError::InvalidRequest);
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Internal ordered queue input. It cannot be constructed by the wire parser;
/// the runtime must separately bind its durable kiosk Origin and credentials.
#[derive(Clone)]
pub(super) struct RestoreIds(Vec<DecimalId>);
impl RestoreIds {
    pub fn new(ids: Vec<DecimalId>) -> Result<Self, WireError> {
        if ids.is_empty() || ids.len() > plx_data::ott::favorites::ENTRY_COUNT {
            return Err(WireError::InvalidRequest);
        }
        Ok(Self(ids))
    }
    pub fn as_slice(&self) -> &[DecimalId] {
        &self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrepareKind {
    Play,
    Preview,
}
impl PrepareKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Preview => "preview",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QueueRequest {
    Prepare {
        kind: PrepareKind,
        ids: Vec<DecimalId>,
    },
    Status,
    Next,
    Previous,
    Stop,
    Repeat(RepeatPolicy),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LibrarySelector {
    Id(DecimalId),
    Query(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LibraryRequest {
    List,
    Prepare {
        kind: PrepareKind,
        selector: LibrarySelector,
        shuffle: bool,
        allow_truncate: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlexRequest {
    Queue(QueueRequest),
    Library(LibraryRequest),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueueError {
    Configuration,
    ServerUnavailable,
    MissingItem,
    Unavailable,
    TimedOut,
    Cancelled,
    Busy,
    ContextChanged,
    StartFailed,
    Empty,
    First,
    Last,
    Restricted,
}
impl QueueError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Configuration => "Plex configuration is missing or invalid.",
            Self::ServerUnavailable => "Plex server is unreachable or access was denied.",
            Self::MissingItem => "One or more Plex items are unavailable or not playable.",
            Self::Unavailable => "Plex playback is unavailable on this player.",
            Self::TimedOut => "Plex queue request timed out.",
            Self::Cancelled => "Plex queue request was cancelled.",
            Self::Busy => "Plex queue request is already in progress.",
            Self::ContextChanged => "Player context changed before Plex playback.",
            Self::StartFailed => "Plex playback could not start.",
            Self::Empty => "Plex queue is empty.",
            Self::First => "Plex queue is already at its first item.",
            Self::Last => "Plex queue is already at its last item.",
            Self::Restricted => "Kiosk mode does not allow a Plex queue.",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LibraryError {
    Ambiguous,
    NotFound,
    Unsupported,
    TooManyLibraries,
    TooManyItems,
    Incomplete,
}
impl LibraryError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Ambiguous => "ambiguous_library",
            Self::NotFound => "library_not_found",
            Self::Unsupported => "unsupported_library",
            Self::TooManyLibraries => "too_many_libraries",
            Self::TooManyItems => "too_many_items",
            Self::Incomplete => "incomplete_library",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WireError {
    InvalidRequest,
    InvalidResult,
    Queue(QueueError),
    Library(LibraryError),
}
fn runtime_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn wire_text(value: &str, limit: usize, nonempty: bool) -> bool {
    value.len() <= limit
        && (!nonempty || !value.trim().is_empty())
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
fn ids(value: &Value) -> Result<Vec<DecimalId>, WireError> {
    ids_bounded(value, ITEM_LIMIT)
}
fn ids_bounded(value: &Value, limit: usize) -> Result<Vec<DecimalId>, WireError> {
    let rows = value.as_array().ok_or(WireError::InvalidRequest)?;
    if rows.is_empty() || rows.len() > limit {
        return Err(WireError::InvalidRequest);
    }
    rows.iter()
        .map(|v| DecimalId::parse(v.as_str().ok_or(WireError::InvalidRequest)?))
        .collect()
}
pub(super) fn parse(action: &str, params: &Value, runtime: &str) -> Result<PlexRequest, WireError> {
    state_version(action, params)?;
    let p = params.as_object().ok_or(WireError::InvalidRequest)?;
    let extension_fields = usize::from(p.contains_key("state_version"));
    if !runtime_valid(runtime) || p.get("runtime").and_then(Value::as_str) != Some(runtime) {
        return Err(WireError::InvalidRequest);
    }
    let op = p
        .get("op")
        .and_then(Value::as_str)
        .ok_or(WireError::InvalidRequest)?;
    let kind = match op {
        "play" => Some(PrepareKind::Play),
        "preview" => Some(PrepareKind::Preview),
        _ => None,
    };
    match action {
        "plex_queue" => {
            let request = if let Some(kind) = kind {
                if p.len() != 3 + extension_fields {
                    return Err(WireError::InvalidRequest);
                }
                QueueRequest::Prepare {
                    kind,
                    ids: ids(p.get("ids").ok_or(WireError::InvalidRequest)?)?,
                }
            } else if op == "repeat" {
                if p.len() != 3 + extension_fields {
                    return Err(WireError::InvalidRequest);
                }
                QueueRequest::Repeat(match p.get("repeat").and_then(Value::as_str) {
                    Some("none") => RepeatPolicy::None,
                    Some("all") => RepeatPolicy::All,
                    _ => return Err(WireError::InvalidRequest),
                })
            } else {
                if p.len() != 2 + extension_fields {
                    return Err(WireError::InvalidRequest);
                }
                match op {
                    "status" => QueueRequest::Status,
                    "next" => QueueRequest::Next,
                    "previous" => QueueRequest::Previous,
                    "stop" => QueueRequest::Stop,
                    _ => return Err(WireError::InvalidRequest),
                }
            };
            Ok(PlexRequest::Queue(request))
        }
        "plex_library" => {
            if op == "list" && p.len() == 2 {
                return Ok(PlexRequest::Library(LibraryRequest::List));
            }
            let kind = kind.ok_or(WireError::InvalidRequest)?;
            if p.keys().any(|k| {
                ![
                    "runtime",
                    "op",
                    "library_id",
                    "query",
                    "shuffle",
                    "allow_truncate",
                ]
                .contains(&k.as_str())
            }) {
                return Err(WireError::InvalidRequest);
            }
            let selector = match (p.get("library_id"), p.get("query")) {
                (Some(id), None) => LibrarySelector::Id(DecimalId::parse(
                    id.as_str().ok_or(WireError::InvalidRequest)?,
                )?),
                (None, Some(Value::String(query))) if wire_text(query, 256, true) => {
                    LibrarySelector::Query(query.clone())
                }
                _ => return Err(WireError::InvalidRequest),
            };
            let shuffle = match p.get("shuffle") {
                None => false,
                Some(Value::Bool(b)) => *b,
                _ => return Err(WireError::InvalidRequest),
            };
            let allow_truncate = match p.get("allow_truncate") {
                None => false,
                Some(Value::Bool(true)) => true,
                _ => return Err(WireError::InvalidRequest),
            };
            Ok(PlexRequest::Library(LibraryRequest::Prepare {
                kind,
                selector,
                shuffle,
                allow_truncate,
            }))
        }
        _ => Err(WireError::InvalidRequest),
    }
}

pub(super) fn queue_capability() -> Value {
    let mut operations = QUEUE_STATE_OPERATIONS.to_vec();
    operations.insert(1, "preview");
    json!({"version":1,"operations":operations,"max_items":ITEM_LIMIT})
}

pub(super) fn queue_repeat_capability() -> Value {
    json!({"version":1,"modes":["none","all"]})
}
/// Additive read-state format; public writes retain the legacy item bound.
pub(super) fn queue_state_v2_capability() -> Value {
    let state = StateVersion::V2;
    json!({"version":state.number(),"operations":QUEUE_STATE_OPERATIONS,
        "max_write_items":ITEM_LIMIT,"max_state_items":state.item_limit(),
        "max_state_bytes":state.byte_limit()})
}
pub(super) fn library_capability() -> Value {
    json!({"version":1,"operations":["list","preview","play"],"max_items":ITEM_LIMIT})
}
fn bounded(value: Value, limit: usize) -> Result<Value, WireError> {
    if serde_json::to_vec(&value)
        .map_err(|_| WireError::InvalidResult)?
        .len()
        > limit
    {
        Err(WireError::InvalidResult)
    } else {
        Ok(value)
    }
}
fn checked_runtime(runtime: &str) -> Result<(), WireError> {
    if runtime_valid(runtime) {
        Ok(())
    } else {
        Err(WireError::InvalidResult)
    }
}
fn public_title(value: &str, limit: usize) -> String {
    let mut result = String::new();
    for c in value.trim().chars() {
        let c = if c <= '\u{1f}' || c == '\u{7f}' {
            ' '
        } else {
            c
        };
        if result.len() + c.len_utf8() > limit {
            break;
        }
        result.push(c);
    }
    if result.trim().is_empty() {
        "Plex".into()
    } else {
        result
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordKind {
    Movie,
    Episode,
}
/// Receiver-owned validated playability projection; never a stream URL or token.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct PlexRecord {
    pub id: String,
    pub title: String,
    pub kind: RecordKind,
    pub playable: bool,
}
#[derive(Clone)]
pub(super) struct PreparedQueue {
    records: Vec<PlexRecord>,
}
impl PreparedQueue {
    pub fn records(&self) -> &[PlexRecord] {
        &self.records
    }
    pub fn ids(&self) -> Vec<&str> {
        self.records.iter().map(|r| r.id.as_str()).collect()
    }
    pub fn apply_permutation(&mut self, order: &[usize]) -> Result<(), WireError> {
        if order.len() != self.records.len() {
            return Err(WireError::InvalidRequest);
        }
        let mut seen = HashSet::new();
        if order
            .iter()
            .any(|i| *i >= self.records.len() || !seen.insert(*i))
        {
            return Err(WireError::InvalidRequest);
        }
        self.records = order.iter().map(|i| self.records[*i].clone()).collect();
        Ok(())
    }
    pub fn preview_value(&self, runtime: &str) -> Result<Value, WireError> {
        checked_runtime(runtime)?;
        bounded(
            json!({"version":1,"runtime":runtime,"state":"ready","ids":self.ids(),
            "titles":self.records.iter().map(|r|r.title.as_str()).collect::<Vec<_>>(),"order":"listed"}),
            1024 * 1024,
        )
    }
}
pub(super) fn prepare(
    ids: &[DecimalId],
    records: &[PlexRecord],
) -> Result<PreparedQueue, WireError> {
    prepare_bounded(ids, records, ITEM_LIMIT)
}
pub(super) fn prepare_restore(
    ids: &RestoreIds,
    records: &[PlexRecord],
) -> Result<PreparedQueue, WireError> {
    prepare_bounded(
        ids.as_slice(),
        records,
        plx_data::ott::favorites::ENTRY_COUNT,
    )
}
fn prepare_bounded(
    ids: &[DecimalId],
    records: &[PlexRecord],
    limit: usize,
) -> Result<PreparedQueue, WireError> {
    if ids.is_empty() || ids.len() > limit || records.len() > limit {
        return Err(WireError::InvalidRequest);
    }
    let mut total = 0usize;
    let mut found = HashMap::new();
    for record in records {
        total = total
            .saturating_add(record.title.len())
            .saturating_add(record.id.len());
        if total > METADATA_BYTES || DecimalId::parse(&record.id).is_err() {
            return Err(WireError::InvalidResult);
        }
        if let Some(previous) = found.insert(record.id.as_str(), record) {
            if previous != record {
                return Err(WireError::Queue(QueueError::MissingItem));
            }
        }
    }
    let ordered: Result<Vec<_>, _> = ids
        .iter()
        .map(|id| {
            let record = found
                .get(id.as_str())
                .filter(|r| r.playable)
                .ok_or(WireError::Queue(QueueError::MissingItem))?;
            Ok(PlexRecord {
                id: id.0.clone(),
                title: public_title(&record.title, 512),
                kind: record.kind,
                playable: true,
            })
        })
        .collect();
    Ok(PreparedQueue { records: ordered? })
}
pub(super) fn preview_error(
    runtime: &str,
    ids: &[DecimalId],
    error: QueueError,
) -> Result<Value, WireError> {
    checked_runtime(runtime)?;
    if ids.is_empty() || ids.len() > ITEM_LIMIT {
        return Err(WireError::InvalidRequest);
    }
    bounded(
        json!({"version":1,"runtime":runtime,"state":"error","ids":ids.iter().map(DecimalId::as_str).collect::<Vec<_>>(),
        "titles":[],"order":"listed","error":error.message()}),
        1024 * 1024,
    )
}
pub(super) fn idle_snapshot(runtime: &str) -> Result<Value, WireError> {
    idle_snapshot_version(runtime, StateVersion::V1)
}
pub(super) fn idle_snapshot_version(
    runtime: &str,
    version: StateVersion,
) -> Result<Value, WireError> {
    checked_runtime(runtime)?;
    Ok(
        json!({"version":version.number(),"runtime":runtime,"active":false,"state":"idle","ids":[],"index":null,"repeat":"none","order":"listed"}),
    )
}

pub(super) fn empty_error_snapshot(runtime: &str, error: QueueError) -> Result<Value, WireError> {
    empty_error_snapshot_version(runtime, error, StateVersion::V1)
}
pub(super) fn empty_error_snapshot_version(
    runtime: &str,
    error: QueueError,
    version: StateVersion,
) -> Result<Value, WireError> {
    let mut value = idle_snapshot_version(runtime, version)?;
    value["state"] = json!("error");
    value["error"] = json!(error.message());
    bounded(value, version.byte_limit())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RepeatPolicy {
    None,
    All,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlaybackPhase {
    Playing,
    Paused,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Stopped,
    Preparing,
    Playing,
    Paused,
    Ended,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlexDecision {
    Start { index: usize },
    Ended,
    Ignore,
}
/// One payload of the common media owner's Local/Plex enum, never a singleton.
pub(super) struct PlexQueue {
    owner: OwnerToken,
    prepared: PreparedQueue,
    index: usize,
    playback: Option<PlaybackToken>,
    state: State,
    requested_repeat: RepeatPolicy,
    kiosk_repeat: bool,
    error: Option<QueueError>,
}
impl PlexQueue {
    pub fn owner(&self) -> OwnerToken {
        self.owner
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn playback(&self) -> Option<PlaybackToken> {
        self.playback
    }
    pub fn stopped(&self) -> bool {
        self.state == State::Stopped
    }
    pub fn park(&mut self, owner: OwnerToken) -> Result<(), QueueError> {
        self.check(owner)?;
        self.playback = None;
        if matches!(
            self.state,
            State::Preparing | State::Playing | State::Paused
        ) {
            self.state = State::Paused;
        }
        Ok(())
    }
    pub fn stop(&mut self, owner: OwnerToken) -> Result<(), QueueError> {
        self.check(owner)?;
        self.playback = None;
        self.state = State::Stopped;
        Ok(())
    }
    pub fn new(owner: OwnerToken, prepared: PreparedQueue) -> Self {
        Self {
            owner,
            prepared,
            index: 0,
            playback: None,
            state: State::Preparing,
            requested_repeat: RepeatPolicy::None,
            kiosk_repeat: false,
            error: None,
        }
    }
    pub fn records(&self) -> &[PlexRecord] {
        self.prepared.records()
    }
    fn check(&self, owner: OwnerToken) -> Result<(), QueueError> {
        if owner == self.owner {
            Ok(())
        } else {
            Err(QueueError::ContextChanged)
        }
    }
    /// The umbrella supplies a fresh playback ticket and executes the returned
    /// start. Its accepted callback owns the RPC reply; this is no playback proof.
    pub fn begin(
        &mut self,
        owner: OwnerToken,
        playback: PlaybackToken,
        index: usize,
    ) -> Result<PlexDecision, QueueError> {
        self.check(owner)?;
        if index >= self.prepared.records.len() {
            return Err(QueueError::MissingItem);
        }
        self.index = index;
        self.playback = Some(playback);
        self.state = State::Preparing;
        self.error = None;
        Ok(PlexDecision::Start { index })
    }
    /// Choose a destination without changing the admitted cursor or ticket.
    pub fn target_index(&self, owner: OwnerToken, offset: i64) -> Result<usize, QueueError> {
        self.check(owner)?;
        if self.stopped() {
            return Err(QueueError::Unavailable);
        }
        let index = (self.index as i64)
            .checked_add(offset)
            .ok_or(if offset < 0 {
                QueueError::First
            } else {
                QueueError::Last
            })?;
        if index < 0 {
            return Err(QueueError::First);
        }
        if index as usize >= self.prepared.records.len() {
            return Err(QueueError::Last);
        }
        Ok(index as usize)
    }
    pub fn observe(
        &mut self,
        owner: OwnerToken,
        playback: PlaybackToken,
        phase: PlaybackPhase,
    ) -> bool {
        if owner != self.owner
            || self.playback != Some(playback)
            || matches!(self.state, State::Stopped | State::Ended | State::Error)
        {
            return false;
        }
        self.state = match phase {
            PlaybackPhase::Playing => State::Playing,
            PlaybackPhase::Paused => State::Paused,
        };
        true
    }
    pub fn fail(&mut self, owner: OwnerToken, playback: PlaybackToken, error: QueueError) -> bool {
        if owner != self.owner || self.playback != Some(playback) || self.state == State::Ended {
            return false;
        }
        self.state = State::Error;
        self.error = Some(error);
        true
    }
    /// A consumed EOF has no live ticket to fail. Keep its cursor and report a
    /// terminal error if its one successor could not be admitted.
    pub fn fail_pending(&mut self, owner: OwnerToken, error: QueueError) -> bool {
        if owner != self.owner || self.playback.is_some() || self.state != State::Preparing {
            return false;
        }
        self.state = State::Error;
        self.error = Some(error);
        true
    }
    pub fn set_repeat(
        &mut self,
        owner: OwnerToken,
        repeat: RepeatPolicy,
    ) -> Result<(), QueueError> {
        self.check(owner)?;
        self.requested_repeat = repeat;
        Ok(())
    }

    /// The durable kiosk owner can force All without overwriting ordinary intent.
    pub fn set_kiosk_repeat(&mut self, owner: OwnerToken, forced: bool) -> Result<(), QueueError> {
        self.check(owner)?;
        self.kiosk_repeat = forced;
        Ok(())
    }

    fn effective_repeat(&self, requested: RepeatPolicy) -> RepeatPolicy {
        if self.kiosk_repeat { RepeatPolicy::All } else { requested }
    }
    pub fn ended(&mut self, owner: OwnerToken, playback: PlaybackToken) -> PlexDecision {
        if owner != self.owner || self.playback != Some(playback) || self.state != State::Playing {
            return PlexDecision::Ignore;
        }
        let next = if self.index + 1 < self.prepared.records.len() {
            Some(self.index + 1)
        } else if self.effective_repeat(self.requested_repeat) == RepeatPolicy::All {
            Some(0)
        } else {
            None
        };
        if let Some(index) = next {
            // Consume this EOF exactly once while the caller admits the next
            // ticket. Re-observing the old ticket must not re-arm its EOF.
            self.playback = None;
            self.state = State::Preparing;
            PlexDecision::Start { index }
        } else {
            self.state = State::Ended;
            PlexDecision::Ended
        }
    }
    pub fn state_fits(&self, version: StateVersion) -> bool {
        self.stopped() || self.records().len() <= version.item_limit()
    }
    pub fn snapshot(&self, runtime: &str) -> Result<Value, WireError> {
        self.snapshot_version(runtime, StateVersion::V1)
    }
    pub fn snapshot_version(
        &self,
        runtime: &str,
        version: StateVersion,
    ) -> Result<Value, WireError> {
        self.snapshot_with_repeat(runtime, version, self.requested_repeat)
    }

    /// Preflight the exact proposed response before the caller commits the setter.
    pub fn snapshot_for_repeat(
        &self,
        runtime: &str,
        version: StateVersion,
        repeat: RepeatPolicy,
    ) -> Result<Value, WireError> {
        self.snapshot_with_repeat(runtime, version, repeat)
    }

    fn snapshot_with_repeat(
        &self,
        runtime: &str,
        version: StateVersion,
        requested: RepeatPolicy,
    ) -> Result<Value, WireError> {
        checked_runtime(runtime)?;
        if !self.state_fits(version) {
            return Err(WireError::InvalidResult);
        }
        if self.stopped() {
            return idle_snapshot_version(runtime, version);
        }
        let mut data = json!({"version":version.number(),"runtime":runtime,"active":true,"ids":self.prepared.ids(),"index":self.index,
            "state":match self.state {State::Stopped=>"idle",State::Preparing=>"preparing",State::Playing=>"playing",State::Paused=>"paused",State::Ended=>"ended",State::Error=>"error"},
            "repeat":if self.effective_repeat(requested)==RepeatPolicy::All {"all"} else {"none"},"order":"listed",
            "title":self.prepared.records[self.index].title});
        if let Some(error) = self.error {
            data["error"] = json!(error.message());
        }
        bounded(data, version.byte_limit())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LibraryKind {
    Movie,
    Show,
    Other,
}
#[derive(Clone)]
pub(super) struct LibraryRow {
    pub id: String,
    pub title: String,
    pub kind: LibraryKind,
}
fn validate_libraries(rows: &[LibraryRow]) -> Result<(), WireError> {
    if rows.len() > LIBRARY_LIMIT {
        return Err(WireError::Library(LibraryError::TooManyLibraries));
    }
    let mut ids = HashSet::new();
    let mut bytes = 0usize;
    for row in rows {
        bytes = bytes
            .saturating_add(row.title.len())
            .saturating_add(row.id.len());
        if bytes > METADATA_BYTES
            || DecimalId::parse(&row.id).is_err()
            || row.title.trim().is_empty()
            || !ids.insert(row.id.as_str())
        {
            return Err(WireError::Library(LibraryError::Incomplete));
        }
    }
    Ok(())
}
fn summary(row: &LibraryRow) -> Result<Value, WireError> {
    validate_libraries(std::slice::from_ref(row))?;
    let kind = match row.kind {
        LibraryKind::Movie => "movie",
        LibraryKind::Show => "show",
        LibraryKind::Other => return Err(WireError::Library(LibraryError::Unsupported)),
    };
    Ok(json!({"id":row.id,"title":public_title(&row.title,160),"type":kind}))
}
/// The umbrella supplies shared caseless_key, not ASCII lowercase. Whitespace
/// folding matches the current library selector without changing stored titles.
pub(super) fn select_library(
    rows: &[LibraryRow],
    selector: &LibrarySelector,
    normalize: fn(&str) -> String,
) -> Result<LibraryRow, WireError> {
    validate_libraries(rows)?;
    let indices: Vec<usize> = match selector {
        LibrarySelector::Id(id) => rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.id == id.as_str())
            .map(|(i, _)| i)
            .collect(),
        LibrarySelector::Query(query) => {
            let key = normalize(query)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if key.is_empty() {
                return Err(WireError::InvalidRequest);
            }
            let keys: Vec<_> = rows
                .iter()
                .map(|r| {
                    normalize(&r.title)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            let exact: Vec<_> = keys
                .iter()
                .enumerate()
                .filter(|(_, k)| **k == key)
                .map(|(i, _)| i)
                .collect();
            if exact.is_empty() {
                keys.iter()
                    .enumerate()
                    .filter(|(_, k)| k.contains(&key))
                    .map(|(i, _)| i)
                    .collect()
            } else {
                exact
            }
        }
    };
    match indices.as_slice() {
        [] => Err(WireError::Library(LibraryError::NotFound)),
        [index] if rows[*index].kind == LibraryKind::Other => {
            Err(WireError::Library(LibraryError::Unsupported))
        }
        [index] => Ok(rows[*index].clone()),
        _ => Err(WireError::Library(LibraryError::Ambiguous)),
    }
}
pub(super) fn library_list_value(runtime: &str, rows: &[LibraryRow]) -> Result<Value, WireError> {
    checked_runtime(runtime)?;
    validate_libraries(rows)?;
    let libraries: Result<Vec<_>, _> = rows
        .iter()
        .filter(|r| r.kind != LibraryKind::Other)
        .map(summary)
        .collect();
    bounded(
        json!({"version":1,"runtime":runtime,"op":"list","state":"ready","libraries":libraries?}),
        1024 * 1024,
    )
}
pub(super) fn library_error_value(
    runtime: &str,
    request: &LibraryRequest,
    error: WireError,
) -> Result<Value, WireError> {
    checked_runtime(runtime)?;
    let op = match request {
        LibraryRequest::List => "list",
        LibraryRequest::Prepare { kind, .. } => kind.name(),
    };
    let text = match error {
        WireError::Queue(e) => e.message(),
        WireError::Library(e) => e.code(),
        _ => return Err(WireError::InvalidResult),
    };
    Ok(json!({"version":1,"runtime":runtime,"op":op,"state":"error","error":text}))
}
/// A complete or proven-truncated collection is supplied by the bounded typed
/// reader. Page completeness remains that reader's prerequisite, never inferred
/// here merely because 500 items happened to arrive.
pub(super) fn prepare_library(
    request: &LibraryRequest,
    library: &LibraryRow,
    records: &[PlexRecord],
    truncated: bool,
    permutation: Option<&[usize]>,
) -> Result<PreparedQueue, WireError> {
    let LibraryRequest::Prepare {
        selector,
        shuffle,
        allow_truncate,
        ..
    } = request
    else {
        return Err(WireError::InvalidRequest);
    };
    summary(library)?;
    if let LibrarySelector::Id(id) = selector {
        if id.as_str() != library.id {
            return Err(WireError::InvalidResult);
        }
    }
    if records.len() > ITEM_LIMIT || (truncated && !*allow_truncate) {
        return Err(WireError::Library(LibraryError::TooManyItems));
    }
    if truncated && records.len() != ITEM_LIMIT {
        return Err(WireError::Library(LibraryError::Incomplete));
    }
    if records.is_empty() {
        return Err(WireError::Queue(QueueError::Empty));
    }
    let kind = if library.kind == LibraryKind::Movie {
        RecordKind::Movie
    } else {
        RecordKind::Episode
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for record in records {
        if record.kind != kind || !seen.insert(record.id.as_str()) {
            return Err(WireError::Library(LibraryError::Incomplete));
        }
        ids.push(
            DecimalId::parse(&record.id)
                .map_err(|_| WireError::Library(LibraryError::Incomplete))?,
        );
    }
    let mut result = prepare(&ids, records)?;
    match (*shuffle, permutation) {
        (true, Some(order)) => result.apply_permutation(order)?,
        (false, None) => {}
        _ => return Err(WireError::InvalidRequest),
    }
    Ok(result)
}
pub(super) fn library_prepared_value(
    runtime: &str,
    request: &LibraryRequest,
    library: &LibraryRow,
    nested: Value,
    truncated: bool,
) -> Result<Value, WireError> {
    checked_runtime(runtime)?;
    let LibraryRequest::Prepare {
        kind,
        selector,
        shuffle,
        allow_truncate,
    } = request
    else {
        return Err(WireError::InvalidRequest);
    };
    if let LibrarySelector::Id(id) = selector {
        if id.as_str() != library.id {
            return Err(WireError::InvalidResult);
        }
    }
    // Callers use PreparedQueue::preview_value or PlexQueue::snapshot. Still
    // validate this boundary, rather than trusting arbitrary JSON from adapters.
    validate_nested(&nested, runtime, *kind)?;
    let count = nested["ids"]
        .as_array()
        .ok_or(WireError::InvalidResult)?
        .len();
    if truncated && (!*allow_truncate || count != ITEM_LIMIT) {
        return Err(WireError::InvalidResult);
    }
    let mut value = json!({"version":1,"runtime":runtime,"op":kind.name(),"state":"ready","library":summary(library)?,"shuffled":shuffle,"queue":nested});
    if truncated {
        value["truncated"] = json!(true);
    }
    bounded(value, 1024 * 1024)
}
fn validate_nested(value: &Value, runtime: &str, kind: PrepareKind) -> Result<(), WireError> {
    let p = value.as_object().ok_or(WireError::InvalidResult)?;
    let ids = ids_bounded(p.get("ids").ok_or(WireError::InvalidResult)?, ITEM_LIMIT)
        .map_err(|_| WireError::InvalidResult)?;
    if p.get("version").and_then(Value::as_u64) != Some(1)
        || p.get("runtime").and_then(Value::as_str) != Some(runtime)
        || p.get("order").and_then(Value::as_str) != Some("listed")
    {
        return Err(WireError::InvalidResult);
    }
    match kind {
        PrepareKind::Preview => {
            if p.len() != 6 || p.get("state").and_then(Value::as_str) != Some("ready") {
                return Err(WireError::InvalidResult);
            }
            let titles = p
                .get("titles")
                .and_then(Value::as_array)
                .ok_or(WireError::InvalidResult)?;
            if titles.len() != ids.len()
                || titles
                    .iter()
                    .any(|v| !v.as_str().is_some_and(|s| wire_text(s, 512, false)))
            {
                return Err(WireError::InvalidResult);
            }
        }
        PrepareKind::Play => {
            let state = p.get("state").and_then(Value::as_str);
            let valid_state = matches!(state, Some("preparing" | "playing" | "paused"))
                || (state == Some("ended") && ids.len() == 1);
            if p.keys().any(|k| {
                ![
                    "version", "runtime", "active", "state", "ids", "index", "repeat", "order",
                    "title",
                ]
                .contains(&k.as_str())
            }) || !matches!(p.len(), 8 | 9)
                || p.get("active") != Some(&Value::Bool(true))
                || p.get("index").and_then(Value::as_u64) != Some(0)
                || !valid_state
                || !matches!(
                    p.get("repeat").and_then(Value::as_str),
                    Some("none" | "all")
                )
                || p.get("title")
                    .is_some_and(|v| !v.as_str().is_some_and(|s| wire_text(s, 512, false)))
            {
                return Err(WireError::InvalidResult);
            }
        }
    }
    if serde_json::to_vec(value)
        .map_err(|_| WireError::InvalidResult)?
        .len()
        > if kind == PrepareKind::Play {
            MAX_QUEUE_STATE_BYTES
        } else {
            1024 * 1024
        }
    {
        return Err(WireError::InvalidResult);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queue_contract() -> Value {
        serde_json::from_str(include_str!("fixtures/plex-queue-v1.json")).unwrap()
    }
    fn library_contract() -> Value {
        serde_json::from_str(include_str!("fixtures/plex-library-v1.json")).unwrap()
    }
    fn decimal(values: &[&str]) -> Vec<DecimalId> {
        values
            .iter()
            .map(|v| DecimalId::parse(v).unwrap())
            .collect()
    }
    fn record(id: &str) -> PlexRecord {
        PlexRecord {
            id: id.into(),
            title: format!("Item {id}"),
            kind: RecordKind::Episode,
            playable: true,
        }
    }
    fn ready(values: &[&str]) -> PreparedQueue {
        prepare(
            &decimal(values),
            &values.iter().map(|v| record(v)).collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn identity(value: &str) -> String {
        value.into()
    }
    fn library() -> LibraryRow {
        LibraryRow {
            id: "16".into(),
            title: "Series".into(),
            kind: LibraryKind::Show,
        }
    }
    fn request(kind: PrepareKind, shuffle: bool, allow_truncate: bool) -> LibraryRequest {
        LibraryRequest::Prepare {
            kind,
            selector: LibrarySelector::Id(DecimalId::parse("16").unwrap()),
            shuffle,
            allow_truncate,
        }
    }

    #[test]
    fn canonical_queue_contract_and_capability_remain_unchanged() {
        let spec = queue_contract();
        assert_eq!(queue_capability(), spec["capability"]);
        for field in ["play_request", "preview_request"] {
            let req = &spec[field];
            assert!(parse(req["action"].as_str().unwrap(), &req["params"], "page-123").is_ok());
        }
        let q = PlexQueue::new(OwnerToken(1), ready(&["78777", "78776", "78775"]));
        let mut value = q.snapshot("page-123").unwrap();
        value.as_object_mut().unwrap().remove("title");
        assert_eq!(value, spec["queue_result"]);
        assert_eq!(idle_snapshot("page-123").unwrap(), spec["stop_result"]);
        let mut rows = vec![record("78777"), record("78776"), record("78775")];
        for (row, title) in rows
            .iter_mut()
            .zip(["First film", "Second film", "Third film"])
        {
            row.title = title.into();
        }
        let preview = prepare(&decimal(&["78777", "78776", "78775"]), &rows)
            .unwrap()
            .preview_value("page-123")
            .unwrap();
        assert_eq!(preview, spec["preview_result"]);
    }

    #[test]
    fn restore_limits_do_not_widen_public_requests_or_preparation() {
        for count in [500, 501, 1000, 1001] {
            let values: Vec<_> = (1..=count).map(|n| n.to_string()).collect();
            let ids: Vec<_> = values
                .iter()
                .map(|id| DecimalId::parse(id).unwrap())
                .collect();
            let rows: Vec<_> = values.iter().map(|id| record(id)).collect();
            assert_eq!(
                parse(
                    "plex_queue",
                    &json!({"runtime":"page-123","op":"play","ids":values}),
                    "page-123"
                )
                .is_ok(),
                count <= 500
            );
            assert_eq!(prepare(&ids, &rows).is_ok(), count <= 500);
            let restore = RestoreIds::new(ids);
            assert_eq!(restore.is_ok(), count <= 1000);
            if let Ok(restore) = restore {
                assert_eq!(
                    prepare_restore(&restore, &rows).unwrap().records().len(),
                    count
                );
            }
        }
        assert!(RestoreIds::new(Vec::new()).is_err());
    }

    #[test]
    fn restore_projection_preserves_duplicates_and_rejects_a_missing_middle() {
        let ids = RestoreIds::new(decimal(&["42", "42", "7"])).unwrap();
        let prepared = prepare_restore(&ids, &[record("7"), record("42")]).unwrap();
        assert_eq!(prepared.ids(), ["42", "42", "7"]);
        assert_eq!(
            prepare_restore(&ids, &[record("42")]).err(),
            Some(WireError::Queue(QueueError::MissingItem))
        );
    }

    #[test]
    fn queue_status_has_a_separate_one_thousand_id_and_32kib_envelope() {
        let values: Vec<_> = (0..1000)
            .map(|n| format!("1234567890123456{n:04}"))
            .collect();
        let ids = RestoreIds::new(
            values
                .iter()
                .map(|id| DecimalId::parse(id).unwrap())
                .collect(),
        )
        .unwrap();
        let rows: Vec<_> = values.iter().map(|id| record(id)).collect();
        let q = PlexQueue::new(OwnerToken(1), prepare_restore(&ids, &rows).unwrap());
        assert!(q.snapshot("page-123").is_err());
        let snapshot = q.snapshot_version("page-123", StateVersion::V2).unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap().len();
        assert!(bytes > 16 * 1024 && bytes <= MAX_QUEUE_STATE_V2_BYTES);
        assert_eq!(snapshot["version"], 2);
        assert!(validate_nested(&snapshot, "page-123", PrepareKind::Play).is_err());
        for count in [500, 501, 1000, 1001] {
            let mut value = snapshot.clone();
            value["version"] = json!(1);
            value["ids"] = json!(vec!["1"; count]);
            assert_eq!(
                validate_nested(&value, "page-123", PrepareKind::Play).is_ok(),
                count <= 500
            );
            let request = json!({"runtime":"page-123","op":"play","ids":vec!["1";count]});
            assert_eq!(
                parse("plex_queue", &request, "page-123").is_ok(),
                count <= 500
            );
        }
    }

    #[test]
    fn state_version_negotiation_is_explicit_and_excludes_previews_and_libraries() {
        for op in ["play", "status", "next", "previous", "stop"] {
            let mut params = json!({"runtime":"page-123","op":op,"state_version":2});
            if op == "play" {
                params["ids"] = json!(["42"]);
            }
            assert_eq!(state_version("plex_queue", &params), Ok(StateVersion::V2));
            assert!(parse("plex_queue", &params, "page-123").is_ok());
            params.as_object_mut().unwrap().remove("state_version");
            assert_eq!(state_version("plex_queue", &params), Ok(StateVersion::V1));
        }
        for value in [
            Value::Null,
            json!(false),
            json!(1),
            json!(3),
            json!(2.0),
            json!("2"),
        ] {
            let params = json!({"runtime":"page-123","op":"status","state_version":value});
            assert!(parse("plex_queue", &params, "page-123").is_err());
        }
        for (action, params) in [
            (
                "plex_queue",
                json!({"runtime":"page-123","op":"preview","ids":["42"],"state_version":2}),
            ),
            (
                "plex_library",
                json!({"runtime":"page-123","op":"play","library_id":"17","state_version":2}),
            ),
            (
                "plex_library",
                json!({"runtime":"page-123","op":"list","state_version":2}),
            ),
        ] {
            assert!(parse(action, &params, "page-123").is_err());
        }
        for count in [500, 501] {
            let params =
                json!({"runtime":"page-123","op":"play","ids":vec!["42";count],"state_version":2});
            assert_eq!(
                parse("plex_queue", &params, "page-123").is_ok(),
                count == 500
            );
        }
    }

    #[test]
    fn negotiated_state_envelopes_keep_separate_exact_utf8_byte_limits() {
        for version in [StateVersion::V1, StateVersion::V2] {
            let limit = version.byte_limit();
            let exact = json!("x".repeat(limit - 2));
            assert_eq!(serde_json::to_vec(&exact).unwrap().len(), limit);
            assert!(bounded(exact, limit).is_ok());
            assert!(bounded(json!("x".repeat(limit - 1)), limit).is_err());
            assert_eq!(
                idle_snapshot_version("page-123", version).unwrap()["version"],
                version.number()
            );
            assert_eq!(
                empty_error_snapshot_version("page-123", QueueError::StartFailed, version).unwrap()
                    ["version"],
                version.number()
            );
        }
        assert_eq!(StateVersion::V1.byte_limit(), 16384);
        assert_eq!(StateVersion::V2.byte_limit(), 32768);
    }

    #[test]
    fn version_two_contract_fixture_keeps_write_and_state_limits_distinct() {
        let spec: Value =
            serde_json::from_str(include_str!("fixtures/plex-queue-state-v2.json")).unwrap();
        assert_eq!(spec["capability_name"], "plex_queue_state_v2");
        assert_eq!(spec["capability"]["max_write_items"], ITEM_LIMIT);
        assert_eq!(
            spec["capability"]["max_state_items"],
            StateVersion::V2.item_limit()
        );
        assert_eq!(
            spec["capability"]["max_state_bytes"],
            StateVersion::V2.byte_limit()
        );
        assert!(parse("plex_queue", &spec["status_request"], "page-123").is_ok());
        let idle = idle_snapshot_version("page-123", StateVersion::V2).unwrap();
        assert_eq!(idle, spec["idle_response"]);
    }

    #[test]
    fn pinned_request_matrix_rejects_unknown_fields_nulls_and_runtime_mismatch() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("fixtures/plex-request-cases.json")).unwrap();
        assert_eq!(cases.len(), 34);
        for case in cases {
            let actual = parse(
                case["action"].as_str().unwrap(),
                &case["params"],
                "page-123",
            )
            .is_ok();
            assert_eq!(
                actual,
                case["accept"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn canonical_library_contract_shapes_preview_play_and_errors() {
        let spec = library_contract();
        let meta = &spec["preview_result"]["library"];
        let row = LibraryRow {
            id: meta["id"].as_str().unwrap().into(),
            title: meta["title"].as_str().unwrap().into(),
            kind: LibraryKind::Show,
        };
        let list = library_list_value("page-123", std::slice::from_ref(&row)).unwrap();
        assert_eq!(list, spec["list_result"]);
        for key in ["preview_request", "play_request", "truncation_request"] {
            let req = &spec[key];
            assert!(parse("plex_library", &req["params"], "page-123").is_ok());
        }
        let mut records = vec![record("78777"), record("78776"), record("78775")];
        for (r, t) in records.iter_mut().zip(["One", "Two", "Three"]) {
            r.title = t.into();
        }
        let preview_req = request(PrepareKind::Preview, true, false);
        let prepared =
            prepare_library(&preview_req, &row, &records, false, Some(&[0, 1, 2])).unwrap();
        let result = library_prepared_value(
            "page-123",
            &preview_req,
            &row,
            prepared.preview_value("page-123").unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(result, spec["preview_result"]);
        let q = PlexQueue::new(OwnerToken(1), prepared);
        let mut result = library_prepared_value(
            "page-123",
            &request(PrepareKind::Play, true, false),
            &row,
            q.snapshot("page-123").unwrap(),
            false,
        )
        .unwrap();
        result["queue"].as_object_mut().unwrap().remove("title");
        assert_eq!(result, spec["play_result"]);
        assert_eq!(
            library_error_value(
                "page-123",
                &request(PrepareKind::Play, false, false),
                WireError::Library(LibraryError::Ambiguous)
            )
            .unwrap(),
            spec["error_result"]
        );
    }

    #[test]
    fn metadata_reordering_keeps_every_requested_occurrence() {
        let p = prepare(&decimal(&["11", "11", "12"]), &[record("12"), record("11")]).unwrap();
        assert_eq!(p.ids(), vec!["11", "11", "12"]);
        assert_eq!(
            p.preview_value("page-123").unwrap()["titles"],
            json!(["Item 11", "Item 11", "Item 12"])
        );
        let identical = prepare(&decimal(&["11", "11"]), &[record("11"), record("11")]).unwrap();
        assert_eq!(identical.ids(), vec!["11", "11"]);
    }

    #[test]
    fn missing_unplayable_or_conflicting_metadata_never_becomes_a_partial_queue() {
        let ids = decimal(&["11", "12"]);
        assert!(matches!(
            prepare(&ids, &[record("11")]),
            Err(WireError::Queue(QueueError::MissingItem))
        ));
        let mut invalid = record("12");
        invalid.playable = false;
        assert!(matches!(
            prepare(&ids, &[record("11"), invalid]),
            Err(WireError::Queue(QueueError::MissingItem))
        ));
        let mut conflict = record("11");
        conflict.title = "Different".into();
        assert!(matches!(
            prepare(&decimal(&["11"]), &[record("11"), conflict]),
            Err(WireError::Queue(QueueError::MissingItem))
        ));
    }

    #[test]
    fn twenty_digit_decimal_identity_is_never_narrowed_to_an_integer() {
        let id = "99999999999999999999";
        let p = prepare(&decimal(&[id]), &[record(id)]).unwrap();
        assert_eq!(p.ids(), vec![id]);
        let mut row = library();
        row.id = id.into();
        assert_eq!(
            select_library(
                &[row],
                &LibrarySelector::Id(DecimalId::parse(id).unwrap()),
                identity
            )
            .unwrap()
            .id,
            id
        );
        for bad in ["0", "01", "-1", "1/2", "１", "100000000000000000000"] {
            assert!(DecimalId::parse(bad).is_err());
        }
    }

    #[test]
    fn title_shaping_is_utf8_bounded_and_never_emits_control_text() {
        let mut row = record("11");
        row.title = format!("\n{}\u{7f}tail", "🟢".repeat(200));
        let p = prepare(&decimal(&["11"]), &[row]).unwrap();
        let value = p.preview_value("page-123").unwrap();
        let title = value["titles"][0].as_str().unwrap();
        assert_eq!(title.len(), 512);
        assert!(wire_text(title, 512, false));
        let mut row = record("11");
        row.title = "x".repeat(METADATA_BYTES + 1);
        assert!(matches!(
            prepare(&decimal(&["11"]), &[row]),
            Err(WireError::InvalidResult)
        ));
    }

    #[test]
    fn duplicate_occurrences_advance_once_and_old_eof_cannot_rearm() {
        let owner = OwnerToken(7);
        let mut q = PlexQueue::new(owner, ready(&["11", "11", "12"]));
        q.begin(owner, PlaybackToken(1), 0).unwrap();
        q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing);
        assert_eq!(
            q.ended(owner, PlaybackToken(1)),
            PlexDecision::Start { index: 1 }
        );
        assert!(!q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing));
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
        q.begin(owner, PlaybackToken(2), 1).unwrap();
        q.observe(owner, PlaybackToken(2), PlaybackPhase::Playing);
        assert_eq!(q.snapshot("page-123").unwrap()["index"], 1);
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
        assert_eq!(
            q.ended(owner, PlaybackToken(2)),
            PlexDecision::Start { index: 2 }
        );
    }

    #[test]
    fn default_terminal_eof_is_active_ended_without_wrapping() {
        let owner = OwnerToken(7);
        let mut q = PlexQueue::new(owner, ready(&["11", "12"]));
        q.begin(owner, PlaybackToken(1), 1).unwrap();
        q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing);
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ended);
        let value = q.snapshot("page-123").unwrap();
        assert_eq!(value["active"], true);
        assert_eq!(value["state"], "ended");
        assert_eq!(value["index"], 1);
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
    }

    #[test]
    fn policy_loop_retains_order_but_explicit_step_boundaries_reject() {
        let owner = OwnerToken(7);
        let mut p = ready(&["11", "12", "13"]);
        p.apply_permutation(&[2, 0, 1]).unwrap();
        let mut q = PlexQueue::new(owner, p);
        q.set_repeat(owner, RepeatPolicy::All).unwrap();
        assert_eq!(q.target_index(owner, -1), Err(QueueError::First));
        q.begin(owner, PlaybackToken(3), 2).unwrap();
        q.observe(owner, PlaybackToken(3), PlaybackPhase::Playing);
        assert_eq!(q.target_index(owner, 1), Err(QueueError::Last));
        assert_eq!(
            q.ended(owner, PlaybackToken(3)),
            PlexDecision::Start { index: 0 }
        );
        assert_eq!(
            q.snapshot("page-123").unwrap()["ids"],
            json!(["13", "11", "12"])
        );
    }

    #[test]
    fn old_owner_and_ticket_cannot_change_current_playback() {
        let owner = OwnerToken(7);
        let mut q = PlexQueue::new(owner, ready(&["11", "12"]));
        q.begin(owner, PlaybackToken(2), 0).unwrap();
        assert_eq!(
            q.target_index(OwnerToken(8), 1),
            Err(QueueError::ContextChanged)
        );
        assert!(!q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing));
        assert!(!q.fail(OwnerToken(8), PlaybackToken(2), QueueError::StartFailed));
        assert_eq!(
            q.ended(OwnerToken(8), PlaybackToken(2)),
            PlexDecision::Ignore
        );
        assert_eq!(q.snapshot("page-123").unwrap()["state"], "preparing");
    }

    #[test]
    fn paused_and_preparing_sources_do_not_consume_eof() {
        let owner = OwnerToken(7);
        let mut q = PlexQueue::new(owner, ready(&["11", "12"]));
        q.begin(owner, PlaybackToken(1), 0).unwrap();
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
        q.observe(owner, PlaybackToken(1), PlaybackPhase::Paused);
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
        q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing);
        assert_eq!(
            q.ended(owner, PlaybackToken(1)),
            PlexDecision::Start { index: 1 }
        );
    }

    #[test]
    fn failure_does_not_skip_items_and_fresh_explicit_start_clears_error() {
        let owner = OwnerToken(7);
        let mut q = PlexQueue::new(owner, ready(&["11", "12"]));
        q.begin(owner, PlaybackToken(1), 0).unwrap();
        assert!(q.fail(owner, PlaybackToken(1), QueueError::StartFailed));
        assert_eq!(q.ended(owner, PlaybackToken(1)), PlexDecision::Ignore);
        assert!(!q.observe(owner, PlaybackToken(1), PlaybackPhase::Playing));
        assert_eq!(q.snapshot("page-123").unwrap()["index"], 0);
        q.begin(owner, PlaybackToken(2), 0).unwrap();
        assert!(q.snapshot("page-123").unwrap().get("error").is_none());
    }

    #[test]
    fn invalid_permutation_is_rejected_without_partial_reordering() {
        let mut p = ready(&["11", "12", "13"]);
        for bad in [&[0, 0, 2][..], &[0, 1][..], &[0, 1, 3][..]] {
            assert_eq!(p.apply_permutation(bad), Err(WireError::InvalidRequest));
            assert_eq!(p.ids(), vec!["11", "12", "13"]);
        }
    }

    #[test]
    fn lookup_prefers_exact_full_title_then_unique_substring_and_injected_keys() {
        let rows = vec![
            LibraryRow {
                id: "1".into(),
                title: "Series".into(),
                kind: LibraryKind::Show,
            },
            LibraryRow {
                id: "2".into(),
                title: "Series extra".into(),
                kind: LibraryKind::Show,
            },
        ];
        assert_eq!(
            select_library(&rows, &LibrarySelector::Query("Series".into()), identity)
                .unwrap()
                .id,
            "1"
        );
        assert!(matches!(
            select_library(&rows, &LibrarySelector::Query("Ser".into()), identity),
            Err(WireError::Library(LibraryError::Ambiguous))
        ));
        fn keys(value: &str) -> String {
            match value {
                "Éxample" | "EXAMPLE" => "shared-normalized-key".into(),
                other => other.into(),
            }
        }
        let mut row = library();
        row.title = "Éxample".into();
        assert_eq!(
            select_library(&[row], &LibrarySelector::Query("EXAMPLE".into()), keys)
                .unwrap()
                .id,
            "16"
        );
        let prefix = "a".repeat(170);
        let mut first = library();
        first.title = format!("{prefix} first");
        let mut second = library();
        second.id = "17".into();
        second.title = format!("{prefix} second");
        assert_eq!(
            select_library(
                &[first, second],
                &LibrarySelector::Query(format!("{prefix} second")),
                identity
            )
            .unwrap()
            .id,
            "17"
        );
    }

    #[test]
    fn library_list_filters_unsupported_kinds_but_selection_reports_them() {
        let mut unsupported = library();
        unsupported.id = "17".into();
        unsupported.title = "Music".into();
        unsupported.kind = LibraryKind::Other;
        let rows = [library(), unsupported];
        assert_eq!(
            library_list_value("page-123", &rows).unwrap()["libraries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            select_library(
                &rows,
                &LibrarySelector::Id(DecimalId::parse("17").unwrap()),
                identity
            ),
            Err(WireError::Library(LibraryError::Unsupported))
        ));
        assert!(library_list_value("page-123", &[library(), library()]).is_err());
    }

    #[test]
    fn library_preparation_enforces_type_unique_items_and_proven_truncation() {
        let row = library();
        let req = request(PrepareKind::Play, false, false);
        assert!(matches!(
            prepare_library(&req, &row, &[record("11"), record("11")], false, None),
            Err(WireError::Library(LibraryError::Incomplete))
        ));
        let mut wrong = record("11");
        wrong.kind = RecordKind::Movie;
        assert!(prepare_library(&req, &row, &[wrong], false, None).is_err());
        let records: Vec<_> = (1..=500).map(|i| record(&i.to_string())).collect();
        assert!(matches!(
            prepare_library(&req, &row, &records, true, None),
            Err(WireError::Library(LibraryError::TooManyItems))
        ));
        let allowed = request(PrepareKind::Play, true, true);
        let permutation: Vec<_> = (0..500).rev().collect();
        let p = prepare_library(&allowed, &row, &records, true, Some(&permutation)).unwrap();
        assert_eq!(p.ids()[0], "500");
        assert_eq!(p.ids()[499], "1");
        let q = PlexQueue::new(OwnerToken(1), p);
        assert_eq!(
            library_prepared_value(
                "page-123",
                &allowed,
                &row,
                q.snapshot("page-123").unwrap(),
                true
            )
            .unwrap()["truncated"],
            true
        );
        assert!(prepare_library(
            &allowed,
            &row,
            &records[..499],
            true,
            Some(&permutation[..499])
        )
        .is_err());
        let exact = prepare_library(&req, &row, &records, false, None).unwrap();
        let q = PlexQueue::new(OwnerToken(2), exact);
        assert!(library_prepared_value(
            "page-123",
            &req,
            &row,
            q.snapshot("page-123").unwrap(),
            false
        )
        .unwrap()
        .get("truncated")
        .is_none());
    }

    #[test]
    fn shaped_library_receipts_reject_untrusted_nested_extras_and_wrong_variant() {
        let row = library();
        let req = request(PrepareKind::Play, false, false);
        let p = ready(&["11"]);
        assert!(library_prepared_value(
            "page-123",
            &req,
            &row,
            p.preview_value("page-123").unwrap(),
            false
        )
        .is_err());
        let q = PlexQueue::new(OwnerToken(1), p);
        let mut value = q.snapshot("page-123").unwrap();
        value["url"] = json!("https://fixture.invalid/private");
        assert!(library_prepared_value("page-123", &req, &row, value, false).is_err());
        assert!(library_error_value("page-123", &req, WireError::InvalidRequest).is_err());
        let failure =
            preview_error("page-123", &decimal(&["11"]), QueueError::MissingItem).unwrap();
        assert_eq!(failure["titles"], json!([]));
        assert_eq!(failure["state"], "error");
        assert!(queue_contract()["errors"]
            .as_array()
            .unwrap()
            .contains(&failure["error"]));
    }

    #[test]
    fn empty_failed_preparation_has_bounded_error_status_without_a_fake_queue() {
        let value = empty_error_snapshot("page-123", QueueError::TimedOut).unwrap();
        assert_eq!(value["active"], false);
        assert_eq!(value["ids"], json!([]));
        assert!(value["index"].is_null());
        assert_eq!(value["state"], "error");
        assert_eq!(value["error"], "Plex queue request timed out.");
        assert_eq!(value.as_object().unwrap().len(), 9);
        assert!(empty_error_snapshot("Bad Runtime", QueueError::TimedOut).is_err());
    }

    #[test]
    fn one_item_ended_play_receipt_still_satisfies_library_contract() {
        let owner = OwnerToken(1);
        let mut queue = PlexQueue::new(owner, ready(&["11"]));
        queue.begin(owner, PlaybackToken(2), 0).unwrap();
        queue.observe(owner, PlaybackToken(2), PlaybackPhase::Playing);
        assert_eq!(queue.ended(owner, PlaybackToken(2)), PlexDecision::Ended);
        let value = library_prepared_value(
            "page-123",
            &request(PrepareKind::Play, false, false),
            &library(),
            queue.snapshot("page-123").unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(value["queue"]["state"], "ended");
        assert_eq!(value["queue"]["index"], 0);
        let mut bad = value["queue"].clone();
        bad["ids"] = json!(["11", "12"]);
        assert!(library_prepared_value(
            "page-123",
            &request(PrepareKind::Play, false, false),
            &library(),
            bad,
            false
        )
        .is_err());
    }
}

#[cfg(test)]
#[path = "ott_plex_repeat_tests.rs"]
mod repeat_proposal_tests;

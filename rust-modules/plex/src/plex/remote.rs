//! Bounded reads on one already-authorized PMS client. These methods neither
//! select a server nor create playback queues or change saved preferences.
//!
//! Hold one `ReadSession` through library selection and item reads: its deadline,
//! response budget and captured credential identity apply to the whole operation.
//! Cancellation is checked before/after each HTTP request; an in-flight request
//! is bounded by the lesser of eight seconds and the remaining transaction time.

use super::client::QueryBuilder;
use super::models::Metadata;
use super::paging::{paged_path, PageReq};
use super::Client;
use plx_base::task::time::{Budget, Deadline};
use plx_base::task::OffFrame;
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEMS: usize = 500;
const MAX_LIBRARIES: usize = 1000;
const MAX_METADATA_PATH_BYTES: usize = 2048;
const PAGE_SIZE: usize = 200;

/// Errors contain no server URLs, tokens, response bodies or user metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    InvalidIds,
    InvalidLibrary,
    UnsupportedLibrary,
    Cancelled,
    Deadline,
    SourceChanged,
    Transport,
    Unauthorized,
    NotFound,
    Http,
    Malformed,
    Incomplete,
    TooManyItems,
    TooManyLibraries,
    ResponseTooLarge,
    Unplayable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryKind {
    Movie,
    Show,
    Other,
}

/// A complete section row. Keep the full title for Unicode-aware selection in
/// the caller; clipping/sanitizing public response labels happens after matching.
#[derive(Deserialize)]
pub struct Library {
    #[serde(rename = "key")]
    id: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
}

impl Library {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn kind(&self) -> LibraryKind {
        match self.kind.as_str() {
            "movie" => LibraryKind::Movie,
            "show" => LibraryKind::Show,
            _ => LibraryKind::Other,
        }
    }
}

/// Only this reader constructs the result. `truncated` means a unique,
/// type-compatible 501st item was actually observed, never merely claimed by a
/// total. The extra item's media payload is not retained or used for playback.
pub struct LibraryItems {
    items: Vec<Metadata>,
    truncated: bool,
}

impl LibraryItems {
    pub fn items(&self) -> &[Metadata] {
        &self.items
    }
    pub fn truncated(&self) -> bool {
        self.truncated
    }
    pub fn into_items(self) -> Vec<Metadata> {
        self.items
    }
}

#[derive(Clone, Copy)]
struct Identity {
    instance: u32,
    token: u32,
    grant: u32,
}

impl Identity {
    fn capture(client: &Client) -> Self {
        Self {
            instance: client.instance_gen(),
            token: client.token_gen(),
            grant: client.grant_epoch(),
        }
    }
    fn current(self, client: &Client) -> bool {
        !client.data_io_disabled()
            && self.instance == client.instance_gen()
            && self.token == client.token_gen()
            && self.grant == client.grant_epoch()
    }
}

/// Worker-owned, single-authority read transaction. No registry fallback, cache,
/// owner counter or profile mutation is performed here. The runtime still owns
/// commit admission and must reject a result whose playback authority retired.
pub struct ReadSession<'a> {
    client: &'a Client,
    identity: Identity,
    state: ReadState<'a>,
}

impl Client {
    pub fn remote_read<'a>(
        &'a self,
        off: &'a OffFrame,
        deadline: Deadline,
        cancelled: &'a dyn Fn() -> bool,
    ) -> ReadSession<'a> {
        ReadSession {
            client: self,
            identity: Identity::capture(self),
            state: ReadState::new(off, deadline, cancelled),
        }
    }
}

impl ReadSession<'_> {
    fn check(&self) -> Result<(), ReadError> {
        self.state.check(self.identity.current(self.client))
    }

    fn fetch(&mut self, path: &str) -> Result<Vec<u8>, ReadError> {
        let client = self.client;
        let identity = self.identity;
        self.state.read(
            || identity.current(client),
            |limit, allowance| {
                let reply = client
                    .get_json_reply_bounded(path, limit, allowance)
                    .ok_or(ReadError::Transport)?;
                match reply.status {
                    200..=299 => Ok(reply.body),
                    401 | 403 => Err(ReadError::Unauthorized),
                    404 => Err(ReadError::NotFound),
                    _ => Err(ReadError::Http),
                }
            },
        )
    }

    /// Return one full record per distinct requested ID. The caller reconstructs
    /// duplicate occurrences in its requested order. No `excludeElements=Media`
    /// query is used. A missing/duplicate/unexpected record rejects the batch.
    pub fn metadata(&mut self, ids: &[&str]) -> Result<Vec<Metadata>, ReadError> {
        self.check()?;
        let result = metadata_with(ids, |path| self.fetch(path));
        self.check()?;
        result
    }

    pub fn libraries(&mut self) -> Result<Vec<Library>, ReadError> {
        self.check()?;
        let result = libraries_with(|path| self.fetch(path));
        self.check()?;
        result
    }

    /// Read movies, or episodes of a show library, in PMS order. No client sort or
    /// shuffle is applied. Truncation, when allowed, keeps exactly the first 500.
    pub fn library_items(
        &mut self,
        id: &str,
        kind: LibraryKind,
        allow_truncate: bool,
    ) -> Result<LibraryItems, ReadError> {
        self.check()?;
        let result = library_items_with(id, kind, allow_truncate, |path| self.fetch(path));
        self.check()?;
        result
    }
}

struct ReadState<'a> {
    off: &'a OffFrame,
    deadline: Deadline,
    budget: Budget,
    cancelled: &'a dyn Fn() -> bool,
    bytes_left: usize,
}

impl<'a> ReadState<'a> {
    fn new(off: &'a OffFrame, deadline: Deadline, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            off,
            deadline,
            budget: Budget::start(off, Duration::from_secs(35)),
            cancelled,
            bytes_left: MAX_BODY_BYTES,
        }
    }

    fn remaining(&self) -> Duration {
        self.deadline
            .remaining(self.off)
            .min(self.budget.remaining(self.off))
    }

    fn check(&self, current: bool) -> Result<(), ReadError> {
        if (self.cancelled)() {
            return Err(ReadError::Cancelled);
        }
        if !current {
            return Err(ReadError::SourceChanged);
        }
        if self.remaining().is_zero() {
            return Err(ReadError::Deadline);
        }
        Ok(())
    }

    /// Inject only the actual request for deterministic budget/cancellation
    /// tests. Production always calls the Client's authenticated request door.
    fn read(
        &mut self,
        current: impl Fn() -> bool,
        send: impl FnOnce(usize, Duration) -> Result<Vec<u8>, ReadError>,
    ) -> Result<Vec<u8>, ReadError> {
        self.check(current())?;
        if self.bytes_left == 0 {
            return Err(ReadError::ResponseTooLarge);
        }
        let result = send(
            self.bytes_left,
            self.remaining().min(Duration::from_secs(8)),
        );
        self.check(current())?;
        let body = result?;
        self.bytes_left = self
            .bytes_left
            .checked_sub(body.len())
            .ok_or(ReadError::ResponseTooLarge)?;
        Ok(body)
    }
}

fn decimal_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|c| c.is_ascii_digit())
}

// Paging counters are NOT identifiers. Accept PMS's exact unsigned decimal
// strings, never bool/null/fraction/exponent or lossy numeric coercion. Derive
// rejects duplicate count keys before a generic JSON map could erase them.
fn count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    struct Counter;
    impl<'de> serde::de::Visitor<'de> for Counter {
        type Value = Option<u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a nonnegative integer counter")
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
            Ok(Some(value))
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
            u64::try_from(value).map(Some).map_err(E::custom)
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
                return Err(E::custom("invalid counter"));
            }
            value.parse::<u64>().map(Some).map_err(E::custom)
        }
    }
    deserializer.deserialize_any(Counter)
}

fn optional_rows<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Vec<T>>, D::Error> {
    Vec::<T>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "MediaContainer")]
    container: Container,
}

#[derive(Deserialize)]
struct Container {
    #[serde(default, deserialize_with = "count")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "count")]
    offset: Option<u64>,
    #[serde(rename = "totalSize", default, deserialize_with = "count")]
    total: Option<u64>,
    #[serde(rename = "Metadata", default, deserialize_with = "optional_rows")]
    metadata: Option<Vec<Metadata>>,
    #[serde(rename = "Directory", default, deserialize_with = "optional_rows")]
    directory: Option<Vec<Library>>,
}

struct Page<T> {
    rows: Vec<T>,
    size: Option<u64>,
    offset: Option<u64>,
    total: Option<u64>,
}

fn container(body: &[u8]) -> Result<Container, ReadError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ReadError::ResponseTooLarge);
    }
    serde_json::from_slice::<Envelope>(body)
        .map(|v| v.container)
        .map_err(|_| ReadError::Malformed)
}

fn item_page(body: &[u8]) -> Result<Page<Metadata>, ReadError> {
    let value = container(body)?;
    let rows = match value.metadata {
        Some(rows) => rows,
        None if value.size == Some(0) => Vec::new(),
        None => return Err(ReadError::Malformed),
    };
    Ok(Page {
        rows,
        size: value.size,
        offset: value.offset,
        total: value.total,
    })
}

fn library_page(body: &[u8]) -> Result<Page<Library>, ReadError> {
    let value = container(body)?;
    let rows = match value.directory {
        Some(rows) => rows,
        None if value.size == Some(0) => Vec::new(),
        None => return Err(ReadError::Malformed),
    };
    Ok(Page {
        rows,
        size: value.size,
        offset: value.offset,
        total: value.total,
    })
}

fn item_identity(item: &Metadata, expected: Option<LibraryKind>) -> Result<(), ReadError> {
    if !decimal_id(&item.rating_key) {
        return Err(ReadError::Malformed);
    }
    match (expected, item.kind.as_str()) {
        (None, "movie" | "episode")
        | (Some(LibraryKind::Movie), "movie")
        | (Some(LibraryKind::Show), "episode") => Ok(()),
        _ => Err(ReadError::Unplayable),
    }
}

/// This is a readiness check, not a new playback URL builder. The existing
/// playback path still resolves the selected record through its captured client.
fn playable(item: &Metadata) -> bool {
    item.media
        .first()
        .and_then(|m| m.part.first())
        .is_some_and(|p| valid_part_key(&p.key))
}

fn valid_part_key(key: &str) -> bool {
    if key
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '#'))
    {
        return false;
    }
    let (raw_path, query) = key.split_once('?').unwrap_or((key, ""));
    if !raw_path.starts_with("/library/parts/") || raw_path.len() <= "/library/parts/".len() {
        return false;
    }
    let Some(path) = decode_component(raw_path) else {
        return false;
    };
    if path.split('/').any(|s| s == "." || s == "..") {
        return false;
    }
    for param in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = param.split_once('=').unwrap_or((param, ""));
        let Some(name) = decode_component(name) else {
            return false;
        };
        if name.chars().any(char::is_whitespace)
            || name.to_ascii_lowercase().starts_with("x-plex-")
            || decode_component(value).is_none()
        {
            return false;
        }
    }
    true
}

fn decode_component(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = (bytes.next()? as char).to_digit(16)?;
            let lo = (bytes.next()? as char).to_digit(16)?;
            decoded.push((hi * 16 + lo) as u8);
        } else {
            decoded.push(byte);
        }
    }
    let result = String::from_utf8(decoded).ok()?;
    if result.chars().any(|c| c.is_control() || c == '\\') {
        None
    } else {
        Some(result)
    }
}

fn metadata_with(
    ids: &[&str],
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, ReadError>,
) -> Result<Vec<Metadata>, ReadError> {
    if ids.is_empty() || ids.len() > MAX_ITEMS || !ids.iter().all(|id| decimal_id(id)) {
        return Err(ReadError::InvalidIds);
    }
    let mut unique = HashSet::new();
    let ids: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| unique.insert(*id))
        .collect();
    let mut result = Vec::with_capacity(ids.len());
    let mut cursor = 0;
    while cursor < ids.len() {
        let mut path = String::from("/library/metadata/");
        let start = cursor;
        while cursor < ids.len()
            && path.len() + ids[cursor].len() + usize::from(cursor != start)
                <= MAX_METADATA_PATH_BYTES
        {
            if cursor != start {
                path.push(',');
            }
            path.push_str(ids[cursor]);
            cursor += 1;
        }
        let page = item_page(&fetch(&path)?)?;
        let expected: HashSet<&str> = ids[start..cursor].iter().copied().collect();
        let mut found = HashSet::new();
        if page.rows.len() != expected.len()
            || page.size.is_some_and(|n| n != expected.len() as u64)
            || page.offset.is_some_and(|n| n != 0)
            || page.total.is_some_and(|n| n != expected.len() as u64)
        {
            return Err(ReadError::Incomplete);
        }
        for item in page.rows {
            item_identity(&item, None)?;
            if !expected.contains(item.rating_key.as_str())
                || !found.insert(item.rating_key.clone())
            {
                return Err(ReadError::Incomplete);
            }
            if !playable(&item) {
                return Err(ReadError::Unplayable);
            }
            result.push(item);
        }
    }
    Ok(result)
}

fn libraries_with(
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, ReadError>,
) -> Result<Vec<Library>, ReadError> {
    // This endpoint is a whole section directory, not a section's item list.
    // Re-requesting offsets on a server that ignores them repeats the same rows.
    let page = library_page(&fetch("/library/sections")?)?;
    if page.rows.len() > MAX_LIBRARIES || page.total.is_some_and(|n| n > MAX_LIBRARIES as u64) {
        return Err(ReadError::TooManyLibraries);
    }
    if page.size.is_some_and(|n| n != page.rows.len() as u64)
        || page.offset.is_some_and(|n| n != 0)
        || page.total.is_some_and(|n| n != page.rows.len() as u64)
    {
        return Err(ReadError::Incomplete);
    }
    let mut seen = HashSet::new();
    for row in &page.rows {
        if !decimal_id(&row.id) || row.title.trim().is_empty() || row.kind.is_empty() {
            return Err(ReadError::Malformed);
        }
        if !seen.insert(&row.id) {
            return Err(ReadError::Incomplete);
        }
    }
    Ok(page.rows)
}

fn library_items_with(
    id: &str,
    kind: LibraryKind,
    allow_truncate: bool,
    fetch: impl FnMut(&str) -> Result<Vec<u8>, ReadError>,
) -> Result<LibraryItems, ReadError> {
    if !decimal_id(id) {
        return Err(ReadError::InvalidLibrary);
    }
    let item_type = match kind {
        LibraryKind::Movie => 1,
        LibraryKind::Show => 4,
        LibraryKind::Other => return Err(ReadError::UnsupportedLibrary),
    };
    let path = QueryBuilder::new(format!("/library/sections/{id}/all"))
        .int("type", item_type)
        .build();
    let (items, truncated) = collect_pages(
        &path,
        MAX_ITEMS,
        allow_truncate,
        ReadError::TooManyItems,
        fetch,
        item_page,
        |row| row.rating_key.as_str(),
        |row, retained| {
            item_identity(row, Some(kind))?;
            if retained && !playable(row) {
                return Err(ReadError::Unplayable);
            }
            Ok(())
        },
    )?;
    Ok(LibraryItems { items, truncated })
}

/// Collection is deterministic given request replies. One lookahead proves
/// overflow; counts are checked before converting them to native indexes (ARM32).
fn collect_pages<T>(
    path: &str,
    limit: usize,
    truncate: bool,
    too_many: ReadError,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, ReadError>,
    decode: impl Fn(&[u8]) -> Result<Page<T>, ReadError>,
    id: impl Fn(&T) -> &str,
    validate: impl Fn(&T, bool) -> Result<(), ReadError>,
) -> Result<(Vec<T>, bool), ReadError> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0usize;
    let mut total = None;
    loop {
        let requested = PAGE_SIZE.min(limit + 1 - offset);
        let request = paged_path(
            path,
            PageReq {
                start: offset,
                size: requested,
            },
        );
        let page = decode(&fetch(&request)?)?;
        let count = page.rows.len();
        let end = offset + count;
        if count > requested
            || page.size.is_some_and(|n| n != count as u64)
            || page.offset.is_some_and(|n| n != offset as u64)
            || total.is_some_and(|n| n < end as u64)
            || page
                .total
                .is_some_and(|n| n < end as u64 || total.is_some_and(|t| t != n))
        {
            return Err(ReadError::Incomplete);
        }
        if let Some(reported) = page.total {
            total = Some(reported);
        }
        if !truncate && total.is_some_and(|n| n > limit as u64) {
            return Err(too_many);
        }
        for row in page.rows {
            let retained = rows.len() < limit;
            validate(&row, retained)?;
            if !seen.insert(id(&row).to_owned()) {
                return Err(ReadError::Incomplete);
            }
            if !retained {
                return if truncate {
                    Ok((rows, true))
                } else {
                    Err(too_many)
                };
            }
            rows.push(row);
        }
        offset = end;
        if total == Some(offset as u64) || (total.is_none() && count == 0) {
            return Ok((rows, false));
        }
        if count == 0 {
            return Err(ReadError::Incomplete);
        }
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

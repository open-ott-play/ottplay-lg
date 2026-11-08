//! Plex preparation inside the existing resolver worker. No worker, owner,
//! registry, catalog, playback effect or persistence mechanism is created here.
//! The returned proposal still requires the common runtime's admission check.

use super::ott_plex_queue::{
    self as queue, LibraryError, LibraryRequest, PlexRequest, PrepareKind, PreparedQueue,
    QueueError, QueueRequest, WireError,
};
use plx_base::task::time::{Budget, Deadline};
use plx_base::task::OffFrame;
use plx_plex::plex::{remote, Client, ClientKey, Metadata, ServerId};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Values supplied by the common owner. The callback must be bounded, perform
/// no IO and return one permutation of 0..count. No RNG is introduced here.
pub(super) struct PrepareOptions<'a> {
    /// The host binds the shared Unicode caseless_key implementation.
    pub normalize: fn(&str) -> String,
    pub permutation: Option<&'a mut dyn FnMut(usize) -> Result<Vec<usize>, WireError>>,
}

/// A receipt of the Client actually used, not an independently supplied server
/// selector. Deliberately not Serialize/Debug: it is not a wire response.
pub(super) struct CapturedAuthority {
    pub server: ServerId,
    pub machine_id: String,
    pub client_key: ClientKey,
    pub grant_epoch: u32,
}

impl CapturedAuthority {
    pub(super) fn capture(client: &Client) -> Result<Self, WireError> {
        if !client.id().is_set() || client.machine_id().is_empty() {
            return Err(WireError::Queue(QueueError::Configuration));
        }
        Ok(Self {
            server: client.id(),
            machine_id: client.machine_id().to_owned(),
            client_key: ClientKey::of(client),
            grant_epoch: client.grant_epoch(),
        })
    }

    pub(super) fn matches(&self, client: &Client) -> bool {
        self.server == client.id()
            && self.machine_id == client.machine_id()
            && self.client_key == ClientKey::of(client)
            && self.grant_epoch == client.grant_epoch()
            && !client.data_io_disabled()
    }
}

/// Each full Metadata appears once. Occurrences preserve PreparedQueue order,
/// including duplicates, and index this table. The runtime creates display IDs
/// from its own owner token + occurrence index; ratingKey remains the media ID.
pub(super) struct Selection {
    pub prepared: PreparedQueue,
    pub metadata: Vec<Metadata>,
    pub occurrences: Vec<usize>,
}

pub(super) enum Prepared {
    Libraries {
        rows: Vec<queue::LibraryRow>,
    },
    Queue {
        kind: PrepareKind,
        selection: Selection,
    },
    Library {
        request: LibraryRequest,
        library: queue::LibraryRow,
        truncated: bool,
        selection: Selection,
    },
}

/// No native start/stop or owner transition is represented by this data alone.
/// In particular, list and preview must not be installed as active playback.
pub(super) struct Proposal {
    pub owner: queue::OwnerToken,
    pub authority: CapturedAuthority,
    pub prepared: Prepared,
}

impl Prepared {
    /// The only ready wire responses available before playback admission.
    /// Play responses are produced by the runtime after a real native start.
    pub fn preview_value(&self, runtime: &str) -> Result<Value, WireError> {
        match self {
            Self::Libraries { rows } => queue::library_list_value(runtime, rows),
            Self::Queue {
                kind: PrepareKind::Preview,
                selection,
            } => selection.prepared.preview_value(runtime),
            Self::Library {
                request:
                    request @ LibraryRequest::Prepare {
                        kind: PrepareKind::Preview,
                        ..
                    },
                library,
                truncated,
                selection,
            } => {
                let nested = selection.prepared.preview_value(runtime)?;
                queue::library_prepared_value(runtime, request, library, nested, *truncated)
            }
            _ => Err(WireError::InvalidRequest),
        }
    }
}

/// Call only from the existing resolver worker. Both the original deadline and
/// a single 35-second ceiling cover reads AND all projection/user callbacks.
pub(super) fn prepare(
    off: &OffFrame,
    client: &Client,
    deadline: Deadline,
    cancelled: &dyn Fn() -> bool,
    request: &PlexRequest,
    owner: queue::OwnerToken,
    options: &mut PrepareOptions<'_>,
) -> Result<Proposal, WireError> {
    supported(request, options)?;
    transaction(off, client, deadline, cancelled, owner, |reader, check| {
        prepare_with(reader, request, options, check)
    })
}

/// Internal exact queue restore; public parsing and preparation remain capped
/// at 500. All chunks share this one authority, reader and transaction budget.
pub(super) fn prepare_restore(
    off: &OffFrame,
    client: &Client,
    deadline: Deadline,
    cancelled: &dyn Fn() -> bool,
    ids: &queue::RestoreIds,
    owner: queue::OwnerToken,
) -> Result<Proposal, WireError> {
    transaction(off, client, deadline, cancelled, owner, |reader, check| {
        prepare_restore_with(reader, ids, check)
    })
}

fn transaction(
    off: &OffFrame,
    client: &Client,
    deadline: Deadline,
    cancelled: &dyn Fn() -> bool,
    owner: queue::OwnerToken,
    execute: impl FnOnce(
        &mut remote::ReadSession<'_>,
        &dyn Fn() -> Result<(), WireError>,
    ) -> Result<Prepared, WireError>,
) -> Result<Proposal, WireError> {
    let authority = CapturedAuthority::capture(client)?;
    let budget = Budget::start(off, Duration::from_secs(35));
    let check = || {
        if cancelled() {
            return Err(WireError::Queue(QueueError::Cancelled));
        }
        if !authority.matches(client) {
            return Err(WireError::Queue(QueueError::ContextChanged));
        }
        if deadline.expired(off) || budget.expired(off) {
            return Err(WireError::Queue(QueueError::TimedOut));
        }
        Ok(())
    };
    check()?;
    let mut reader = client.remote_read(off, deadline, cancelled);
    let result = execute(&mut reader, &check);
    // A normalizer, permutation producer or projection can outlive a read's
    // fence. Retire such a result before any proposal leaves the worker.
    check()?;
    Ok(Proposal {
        owner,
        authority,
        prepared: result?,
    })
}

fn prepare_restore_with(
    reader: &mut impl Reader,
    ids: &queue::RestoreIds,
    check: &dyn Fn() -> Result<(), WireError>,
) -> Result<Prepared, WireError> {
    if ids.as_slice().is_empty() || ids.as_slice().len() > plx_data::ott::favorites::ENTRY_COUNT {
        return Err(WireError::InvalidRequest);
    }
    check()?;
    let mut seen = HashSet::new();
    let unique: Vec<_> = ids
        .as_slice()
        .iter()
        .map(queue::DecimalId::as_str)
        .filter(|id| seen.insert(*id))
        .collect();
    let mut metadata = Vec::new();
    // Existing public-preparation limit is also the current ReadSession batch
    // limit. Never recreate the reader here: its byte/time budget spans chunks.
    for chunk in unique.chunks(queue::ITEM_LIMIT) {
        check()?;
        let rows = reader.metadata(chunk).map_err(map_queue_read)?;
        check()?;
        if rows.len() != chunk.len() {
            return Err(WireError::Queue(QueueError::MissingItem));
        }
        metadata.extend(rows);
    }
    let records = project_verified(&metadata).map_err(map_queue_read)?;
    let prepared = queue::prepare_restore(ids, &records)?;
    let selection = selection(prepared, metadata)?;
    check()?;
    Ok(Prepared::Queue {
        kind: PrepareKind::Play,
        selection,
    })
}

fn supported(request: &PlexRequest, options: &PrepareOptions<'_>) -> Result<(), WireError> {
    match request {
        PlexRequest::Queue(QueueRequest::Prepare { .. }) => Ok(()),
        PlexRequest::Library(LibraryRequest::List) => Ok(()),
        PlexRequest::Library(LibraryRequest::Prepare { shuffle: true, .. })
            if options.permutation.is_none() =>
        {
            Err(WireError::InvalidRequest)
        }
        PlexRequest::Library(LibraryRequest::Prepare { .. }) => Ok(()),
        _ => Err(WireError::InvalidRequest),
    }
}

// A private seam over the already bounded reader, not a second PMS client.
// Only the actual ReadSession supplies production values. Tests supply replies
// whose admission is assumed; wire/path/page validation belongs to the reader.
struct ReadItems {
    metadata: Vec<Metadata>,
    truncated: bool,
}

trait Reader {
    fn metadata(&mut self, ids: &[&str]) -> Result<Vec<Metadata>, remote::ReadError>;
    fn libraries(&mut self) -> Result<Vec<remote::Library>, remote::ReadError>;
    fn items(
        &mut self,
        id: &str,
        kind: remote::LibraryKind,
        truncate: bool,
    ) -> Result<ReadItems, remote::ReadError>;
}

impl Reader for remote::ReadSession<'_> {
    fn metadata(&mut self, ids: &[&str]) -> Result<Vec<Metadata>, remote::ReadError> {
        remote::ReadSession::metadata(self, ids)
    }
    fn libraries(&mut self) -> Result<Vec<remote::Library>, remote::ReadError> {
        remote::ReadSession::libraries(self)
    }
    fn items(
        &mut self,
        id: &str,
        kind: remote::LibraryKind,
        truncate: bool,
    ) -> Result<ReadItems, remote::ReadError> {
        let result = remote::ReadSession::library_items(self, id, kind, truncate)?;
        let truncated = result.truncated();
        Ok(ReadItems {
            metadata: result.into_items(),
            truncated,
        })
    }
}

fn prepare_with(
    reader: &mut impl Reader,
    request: &PlexRequest,
    options: &mut PrepareOptions<'_>,
    check: &dyn Fn() -> Result<(), WireError>,
) -> Result<Prepared, WireError> {
    supported(request, options)?;
    check()?;
    let result = match request {
        PlexRequest::Queue(QueueRequest::Prepare { kind, ids }) => {
            let keys: Vec<_> = ids.iter().map(queue::DecimalId::as_str).collect();
            let metadata = reader.metadata(&keys).map_err(map_queue_read)?;
            check()?;
            let records = project_verified(&metadata).map_err(map_queue_read)?;
            let prepared = queue::prepare(ids, &records)?;
            Prepared::Queue {
                kind: *kind,
                selection: selection(prepared, metadata)?,
            }
        }
        PlexRequest::Library(request) => {
            let libraries = reader.libraries().map_err(map_library_read)?;
            check()?;
            let rows: Vec<_> = libraries.iter().map(project_library).collect();
            match request {
                LibraryRequest::List => Prepared::Libraries { rows },
                LibraryRequest::Prepare {
                    selector,
                    shuffle,
                    allow_truncate,
                    ..
                } => {
                    let library = queue::select_library(&rows, selector, options.normalize)?;
                    // Invalidate before another request if normalization retired
                    // the owner or consumed the transaction's allowance.
                    check()?;
                    let read_kind = match library.kind {
                        queue::LibraryKind::Movie => remote::LibraryKind::Movie,
                        queue::LibraryKind::Show => remote::LibraryKind::Show,
                        queue::LibraryKind::Other => {
                            return Err(WireError::Library(LibraryError::Unsupported))
                        }
                    };
                    let items = reader
                        .items(&library.id, read_kind, *allow_truncate)
                        .map_err(map_library_read)?;
                    check()?;
                    let records = project_verified(&items.metadata).map_err(map_library_read)?;
                    let permutation = if *shuffle {
                        let producer = options
                            .permutation
                            .as_deref_mut()
                            .ok_or(WireError::InvalidRequest)?;
                        let result = producer(records.len());
                        check()?;
                        Some(result?)
                    } else {
                        None
                    };
                    let prepared = queue::prepare_library(
                        request,
                        &library,
                        &records,
                        items.truncated,
                        permutation.as_deref(),
                    )?;
                    Prepared::Library {
                        request: request.clone(),
                        library,
                        truncated: items.truncated,
                        selection: selection(prepared, items.metadata)?,
                    }
                }
            }
        }
        _ => return Err(WireError::InvalidRequest),
    };
    check()?;
    Ok(result)
}

fn project_library(library: &remote::Library) -> queue::LibraryRow {
    queue::LibraryRow {
        id: library.id().to_owned(),
        title: library.title().to_owned(),
        kind: match library.kind() {
            remote::LibraryKind::Movie => queue::LibraryKind::Movie,
            remote::LibraryKind::Show => queue::LibraryKind::Show,
            remote::LibraryKind::Other => queue::LibraryKind::Other,
        },
    }
}

/// The private Reader has already checked the full first Media/Part path and
/// the captured authority. Do not clone that path validator or turn arbitrary
/// external Metadata into proof of admission. Retain the full row separately.
fn project_verified(metadata: &[Metadata]) -> Result<Vec<queue::PlexRecord>, remote::ReadError> {
    let mut seen = HashSet::new();
    metadata
        .iter()
        .map(|row| {
            if !seen.insert(row.rating_key.as_str()) {
                return Err(remote::ReadError::Malformed);
            }
            let kind = match row.kind.as_str() {
                "movie" => queue::RecordKind::Movie,
                "episode" => queue::RecordKind::Episode,
                _ => return Err(remote::ReadError::Unplayable),
            };
            if !row
                .media
                .first()
                .and_then(|m| m.part.first())
                .is_some_and(|part| !part.key.is_empty())
            {
                return Err(remote::ReadError::Unplayable);
            }
            Ok(queue::PlexRecord {
                id: row.rating_key.clone(),
                title: row.title.clone(),
                kind,
                playable: true,
            })
        })
        .collect()
}

fn selection(prepared: PreparedQueue, metadata: Vec<Metadata>) -> Result<Selection, WireError> {
    let mut indices = HashMap::new();
    for (index, row) in metadata.iter().enumerate() {
        if indices.insert(row.rating_key.as_str(), index).is_some() {
            return Err(WireError::Queue(QueueError::MissingItem));
        }
    }
    let occurrences: Vec<usize> = prepared
        .records()
        .iter()
        .map(|record| {
            indices
                .get(record.id.as_str())
                .copied()
                .ok_or(WireError::Queue(QueueError::MissingItem))
        })
        .collect::<Result<_, _>>()?;
    // No unrequested metadata survives into a proposal.
    if occurrences.iter().copied().collect::<HashSet<_>>().len() != metadata.len() {
        return Err(WireError::Queue(QueueError::MissingItem));
    }
    Ok(Selection {
        prepared,
        metadata,
        occurrences,
    })
}

fn map_queue_read(error: remote::ReadError) -> WireError {
    use remote::ReadError as E;
    match error {
        E::InvalidIds | E::InvalidLibrary => WireError::InvalidRequest,
        E::Cancelled => WireError::Queue(QueueError::Cancelled),
        E::Deadline => WireError::Queue(QueueError::TimedOut),
        E::SourceChanged => WireError::Queue(QueueError::ContextChanged),
        E::Transport | E::Unauthorized | E::Http => WireError::Queue(QueueError::ServerUnavailable),
        E::NotFound
        | E::Malformed
        | E::Incomplete
        | E::ResponseTooLarge
        | E::Unplayable
        | E::UnsupportedLibrary
        | E::TooManyItems
        | E::TooManyLibraries => WireError::Queue(QueueError::MissingItem),
    }
}

fn map_library_read(error: remote::ReadError) -> WireError {
    use remote::ReadError as E;
    match error {
        E::TooManyLibraries => WireError::Library(LibraryError::TooManyLibraries),
        E::TooManyItems => WireError::Library(LibraryError::TooManyItems),
        E::UnsupportedLibrary => WireError::Library(LibraryError::Unsupported),
        E::NotFound => WireError::Library(LibraryError::NotFound),
        E::Malformed | E::Incomplete | E::ResponseTooLarge | E::Unplayable => {
            WireError::Library(LibraryError::Incomplete)
        }
        other => map_queue_read(other),
    }
}

#[cfg(test)]
#[path = "ott_plex_prepare_tests.rs"]
mod tests;

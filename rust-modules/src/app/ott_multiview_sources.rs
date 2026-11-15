//! Installation-private, read-only multiview source snapshots.
//!
//! No provider switching, native playback admission or global resolver ownership occurs here.
//! The owner authorizes preparation and resolved completion through the ordinary policy stack.
//! Plex requires a server broker; device-local media requires a relay. Neither is a URL grant.
use super::*;
use plx_data::ott::multiview::{MediaRef, ResolveTicket, SourceRef};
use std::num::NonZeroU64;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, Weak};
#[path = "ott_multiview_sources_local.rs"]
mod local;
pub(super) use local::LocalResolved;

const MAX_ROWS: usize = 256;
const MAX_LOCAL_ROWS: usize = 1152;
static NEXT_SOURCE: AtomicU64 = AtomicU64::new(1);
fn next_id(counter: &AtomicU64) -> Result<NonZeroU64, Error> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let next = current.checked_add(1).ok_or(Error::Exhausted)?;
        let id = NonZeroU64::new(current).ok_or(Error::Exhausted)?;
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(id),
            Err(observed) => current = observed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Exhausted,
    UnknownSource,
    Stale,
    Cancelled,
    Unsupported,
    Policy,
    Resolve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Support {
    HttpCandidate,
    PlexBrokerRequired,
    LocalRelayRequired,
    Unavailable,
}

/// Presentation only. This ref is scoped to this process and snapshot, never persisted or sent
/// as a credential. `HttpCandidate` is not a backend, network or policy admission certificate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceHandle {
    source: SourceRef,
    media: MediaRef,
    epoch: u64,
}
impl SourceHandle {
    pub fn source_ref(&self) -> SourceRef {
        self.source
    }
    pub fn media_ref(&self) -> MediaRef {
        self.media
    }
}
impl std::fmt::Debug for SourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceHandle(<opaque>)")
    }
}
/// Credential-free display hints. Nominal kind is not a playback, probe or policy grant.
pub(super) struct SourceDescriptor {
    pub provider: &'static str,
    pub profile: String,
    pub kind: &'static str,
}
pub(super) struct SourceRow {
    pub handle: SourceHandle,
    pub title: String,
    pub support: Support,
    descriptor: Option<SourceDescriptor>,
    identity_digest: String,
}
impl SourceRow {
    pub fn descriptor(&self) -> Option<&SourceDescriptor> {
        self.descriptor.as_ref()
    }
    pub fn identity_digest(&self) -> &str {
        &self.identity_digest
    }
}
struct Row {
    view: SourceRow,
    binding: Option<Binding>,
}
#[derive(Clone)]
struct Binding {
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Option<Favorite>,
    catalog_revision: Option<u64>,
    attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
}
impl Binding {
    fn capture(runtime: &Runtime, favorite: &Favorite) -> Result<Self, Error> {
        let expected_id = format!("favorite:{}", favorite.identity());
        let (request, profile, entry, selected) = runtime
            .prepare_play_request(PlaybackRequest::ordinary(&expected_id, 0))
            .map_err(|_| Error::Stale)?;
        // Kiosk binding may intentionally substitute its pinned request. That must never relabel
        // an unrelated list row as an authorized source.
        if request.id != expected_id || selected.as_ref() != Some(favorite) {
            return Err(Error::Stale);
        }
        let attachment = runtime.attachment_for(favorite).map_err(|_| Error::Stale)?;
        Ok(Self {
            request,
            profile,
            entry,
            favorite: Some(favorite.clone()),
            catalog_revision: None,
            attachment,
        })
    }
    fn current(&self, runtime: &Runtime) -> bool {
        if self.catalog_revision.is_some() {
            // A captured device input is independent of which provider the UI
            // browses next. Definition edits are separately ABA-fenced by the
            // work's profile epoch; physical roots also retain their live read.
            return runtime
                .saved
                .profiles
                .profiles
                .iter()
                .any(|p| p == &self.profile)
                || (self.profile.kind == ProviderKind::Local
                    && runtime.local_profile_selection(&self.profile).is_some())
                || runtime.effective_profile() == Some(&self.profile);
        }
        self.favorite.as_ref().is_some_and(|favorite| {
            Self::capture(runtime, favorite).is_ok_and(|next| {
                self.request == next.request
                    && self.profile == next.profile
                    && self.entry == next.entry
                    && self.favorite == next.favorite
                    && self.attachment == next.attachment
            })
        })
    }
    fn descriptor(&self) -> Option<SourceDescriptor> {
        let id = &self.profile.id;
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        {
            return None;
        }
        use plx_data::ott::favorites::Locator;
        let kind = if let Some(entry) = &self.entry {
            if entry.live.is_some() {
                "live"
            } else if matches!(
                entry.media,
                MediaLocator::Local { .. } | MediaLocator::ServerLibrary { .. }
            ) {
                "file"
            } else {
                "vod"
            }
        } else {
            match self.favorite.as_ref()?.locator {
                Locator::Local { .. } => "file",
                Locator::Plex { .. } | Locator::VPortal { .. } => "vod",
                Locator::Channel { .. } | Locator::ChannelName { .. } => "live",
            }
        };
        Some(SourceDescriptor {
            provider: self.profile.kind.canonical_id(),
            profile: id.clone(),
            kind,
        })
    }
    fn support(&self) -> Support {
        if self.profile.kind == ProviderKind::Plex {
            Support::PlexBrokerRequired
        } else if self.profile.kind == ProviderKind::Local
            && plx_data::ott::http_url(&self.profile.source).is_none()
        {
            Support::LocalRelayRequired
        } else {
            Support::HttpCandidate
        }
    }
}

/// A caller-owned immutable list. No Clone: handles cannot accidentally be copied into a
/// second mutable owner with an independent retirement lifetime.
pub(super) struct Snapshot {
    local: bool,
    epoch: u64,
    pending: Mutex<Vec<Weak<AtomicBool>>>,
    runtime: String,
    generation: u64,
    profile_epoch: u64,
    authority_epoch: u64,
    rows: Vec<Row>,
    retired: Arc<AtomicBool>,
}
impl Snapshot {
    pub fn capture(runtime: &Runtime) -> Result<Self, Error> {
        Self::capture_with_limit(runtime, MAX_ROWS)
    }
    fn capture_with_limit(runtime: &Runtime, limit: usize) -> Result<Self, Error> {
        let epoch = next_id(&NEXT_SOURCE)?.get();
        let mut rows = Vec::new();
        for favorite in runtime.saved.favorites.entries().iter().take(limit) {
            let token = next_id(&NEXT_SOURCE)?;
            let source = SourceRef::from_opaque(token);
            let binding = Binding::capture(runtime, favorite).ok();
            let support = binding
                .as_ref()
                .map_or(Support::Unavailable, Binding::support);
            rows.push(Row {
                view: SourceRow {
                    handle: SourceHandle {
                        source,
                        media: MediaRef::from_opaque(token),
                        epoch,
                    },
                    title: favorite.title.clone(),
                    support,
                    descriptor: binding.as_ref().and_then(Binding::descriptor),
                    identity_digest: plx_data::ott::favorites_wire::hash(
                        favorite.identity().as_bytes(),
                    ),
                },
                binding,
            });
        }
        Ok(Self {
            local: false,
            epoch,
            pending: Mutex::new(Vec::new()),
            runtime: runtime.remote.runtime().into(),
            generation: runtime.generation,
            profile_epoch: runtime.profile_edit_epoch,
            authority_epoch: runtime.multiview_authority_epoch,
            rows,
            retired: Arc::new(AtomicBool::new(false)),
        })
    }
    /// Direct current-catalogue rows carry their real prepared request and
    /// catalogue revision. They are never inserted into persisted Favorites.
    pub fn capture_local(runtime: &Runtime) -> Result<Self, Error> {
        if runtime.multiview_local_profile_epoch == 0 || runtime.multiview_authority_epoch == 0 {
            return Err(Error::Stale);
        }
        let mut snapshot = Self::capture_with_limit(runtime, MAX_LOCAL_ROWS)?;
        snapshot.local = true;
        snapshot.profile_epoch = runtime.multiview_local_profile_epoch;
        if !runtime.catalog.entries.is_empty() {
            snapshot.rows.truncate(MAX_LOCAL_ROWS / 2);
        }
        for entry in runtime
            .catalog
            .entries
            .iter()
            .take(MAX_LOCAL_ROWS.saturating_sub(snapshot.rows.len()))
        {
            let Ok((request, profile, Some(bound), favorite)) =
                runtime.prepare_play_request(PlaybackRequest::ordinary(&entry.id, 0))
            else {
                continue;
            };
            if favorite.is_some() || bound != *entry {
                continue;
            }
            let binding = Binding {
                request,
                profile,
                entry: Some(bound),
                favorite: None,
                catalog_revision: Some(runtime.catalog_revision),
                attachment: None,
            };
            let token = next_id(&NEXT_SOURCE)?;
            let digest = plx_data::ott::favorites_wire::hash(
                serde_json::to_string(&(&binding.profile, entry))
                    .map_err(|_| Error::Stale)?
                    .as_bytes(),
            );
            snapshot.rows.push(Row {
                view: SourceRow {
                    handle: SourceHandle {
                        source: SourceRef::from_opaque(token),
                        media: MediaRef::from_opaque(token),
                        epoch: snapshot.epoch,
                    },
                    title: entry.title.clone(),
                    support: binding.support(),
                    descriptor: binding.descriptor(),
                    identity_digest: digest,
                },
                binding: Some(binding),
            });
        }
        Ok(snapshot)
    }
    pub fn refresh_local(&mut self, runtime: &Runtime) -> Result<(), Error> {
        if !self.local || !self.current(runtime) {
            return Err(Error::Stale);
        }
        let mut fresh = Self::capture_local(runtime)?;
        for mut row in fresh.rows.drain(..) {
            if self.rows.len() >= MAX_LOCAL_ROWS {
                break;
            }
            let duplicate = self
                .rows
                .iter()
                .any(|old| match (&old.binding, &row.binding) {
                    (Some(a), Some(b)) if a.profile == b.profile => {
                        match (&a.favorite, &b.favorite) {
                            (Some(a), Some(b)) => a == b,
                            (None, None) => a
                                .entry
                                .as_ref()
                                .zip(b.entry.as_ref())
                                .is_some_and(|(a, b)| a.id == b.id && a.media == b.media),
                            _ => false,
                        }
                    }
                    _ => false,
                });
            if !duplicate {
                row.view.handle.epoch = self.epoch;
                self.rows.push(row);
            }
        }
        Ok(())
    }
    pub fn rows(&self) -> impl Iterator<Item = &SourceRow> {
        self.rows.iter().map(|r| &r.view)
    }
    pub fn step_local(&self, from: SourceHandle, offset: i64) -> Option<SourceHandle> {
        if !self.local || !self.contains(&from) {
            return None;
        }
        let anchor = self
            .rows
            .iter()
            .find(|r| r.view.handle == from)?
            .binding
            .as_ref()?;
        let candidates = self
            .rows
            .iter()
            .filter(|r| {
                r.binding.as_ref().is_some_and(|binding| {
                    if anchor.favorite.is_some() {
                        binding.favorite.is_some()
                    } else {
                        binding.favorite.is_none() && binding.profile == anchor.profile
                    }
                })
            })
            .map(|r| r.view.handle)
            .collect::<Vec<_>>();
        let at = candidates.iter().position(|h| *h == from)? as i64;
        let next =
            (at + offset.rem_euclid(candidates.len() as i64)).rem_euclid(candidates.len() as i64);
        candidates.get(next as usize).copied()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn contains(&self, handle: &SourceHandle) -> bool {
        !self.retired.load(Ordering::Acquire)
            && handle.epoch == self.epoch
            && self.rows.iter().any(|r| r.view.handle == *handle)
    }
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        let pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        for cancel in pending.iter().filter_map(Weak::upgrade) {
            cancel.store(true, Ordering::Release);
        }
    }
    pub fn current_for(&self, runtime: &Runtime) -> bool {
        self.current(runtime)
    }
    fn current(&self, runtime: &Runtime) -> bool {
        !self.retired.load(Ordering::Acquire)
            && self.runtime == runtime.remote.runtime()
            && (self.local || self.generation == runtime.generation)
            && self.profile_epoch != 0
            && self.profile_epoch
                == if self.local {
                    runtime.multiview_local_profile_epoch
                } else {
                    runtime.profile_edit_epoch
                }
            && self.authority_epoch != 0
            && self.authority_epoch == runtime.multiview_authority_epoch
    }
    pub fn capture_work(
        &self,
        runtime: &mut Runtime,
        app: &App,
        handle: &SourceHandle,
        ticket: ResolveTicket,
    ) -> Result<SourceWork, Error> {
        self.capture_work_using(runtime, app, handle, ticket, false)
    }
    pub fn capture_local_work(
        &self,
        runtime: &mut Runtime,
        app: &App,
        handle: &SourceHandle,
        ticket: ResolveTicket,
    ) -> Result<SourceWork, Error> {
        self.capture_work_using(runtime, app, handle, ticket, true)
    }
    fn capture_work_using(
        &self,
        runtime: &mut Runtime,
        app: &App,
        handle: &SourceHandle,
        ticket: ResolveTicket,
        local: bool,
    ) -> Result<SourceWork, Error> {
        if !self.current(runtime) {
            return Err(Error::Stale);
        }
        if !self.contains(handle)
            || handle.source_ref() != ticket.source()
            || handle.media_ref() != ticket.media()
        {
            return Err(Error::UnknownSource);
        }
        let row = self
            .rows
            .iter()
            .find(|r| r.view.handle == *handle)
            .ok_or(Error::UnknownSource)?;
        let binding = row.binding.as_ref().ok_or(Error::Stale)?;
        if !binding.current(runtime) {
            return Err(Error::Stale);
        }
        if !local
            && matches!(
                binding.support(),
                Support::LocalRelayRequired | Support::Unavailable
            )
        {
            return Err(Error::Unsupported);
        }
        let plex = if binding.profile.kind == ProviderKind::Plex {
            let session = format!(
                "mv-{}-{}",
                plx_data::ott::favorites_wire::hash(self.runtime.as_bytes()),
                next_id(&NEXT_SOURCE)?.get()
            );
            Some(
                match &binding.favorite {
                    Some(favorite) => {
                        super::multiview_plex::Capture::capture(&binding.profile, favorite, session)
                    }
                    None => super::multiview_plex::Capture::capture_entry(
                        &binding.profile,
                        binding.entry.as_ref().ok_or(Error::Stale)?,
                        session,
                    ),
                }
                .map_err(|_| Error::Unsupported)?,
            )
        } else {
            None
        };
        runtime
            .authorize_multiview_source(
                app,
                &binding.request,
                &binding.profile,
                binding.entry.as_ref(),
                binding.favorite.as_ref(),
                None,
            )
            .map_err(|_| Error::Policy)?;
        let permit = match &binding.favorite {
            Some(favorite) => runtime
                .favorite_permit(&binding.profile, favorite)
                .map_err(|_| Error::Policy)?,
            None => None,
        };
        let local_read = if local {
            transient::capture_video_root(runtime, &binding.request, &binding.profile)
                .map_err(|_| Error::Policy)?
        } else {
            None
        };
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            pending.retain(|c| c.strong_count() > 0);
            if pending.len() >= 9 {
                return Err(Error::Exhausted);
            }
            if self.retired.load(Ordering::Acquire) {
                return Err(Error::Stale);
            }
            pending.push(Arc::downgrade(&cancel));
        }
        Ok(SourceWork {
            local,
            local_read,
            ticket,
            plex,
            binding: binding.clone(),
            permit,
            runtime: self.runtime.clone(),
            generation: self.generation,
            profile_epoch: self.profile_epoch,
            authority_epoch: self.authority_epoch,
            retired: self.retired.clone(),
            cancel,
        })
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        self.retire();
    }
}

/// One slot's worker. Cancellation is allocated per capture and never shares the global player
/// or a sibling slot's cancellation. Dropping the snapshot retires all outstanding work.
pub(super) struct SourceWork {
    local: bool,
    local_read: Option<transient::LocalSourceRead>,
    plex: Option<super::multiview_plex::Capture>,
    ticket: ResolveTicket,
    binding: Binding,
    permit: Option<LocalFavoritePermit>,
    runtime: String,
    generation: u64,
    profile_epoch: u64,
    authority_epoch: u64,
    retired: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}
impl SourceWork {
    pub fn prepare_local_metadata(
        mut self,
        worker: &plx_base::task::OffFrame,
    ) -> Result<PreparedMetadata, Error> {
        self.local = true;
        self.prepare_metadata(worker)
    }
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire) || self.retired.load(Ordering::Acquire)
    }
    /// Metadata lookup only: no native media resolution, lease, stream probe or URL export.
    /// Invoke on the caller-owned bounded off-frame worker, never the render thread.
    pub fn prepare_metadata(
        mut self,
        worker: &plx_base::task::OffFrame,
    ) -> Result<PreparedMetadata, Error> {
        if self.cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(plex) = self.plex.take() {
            let plan = plex.prepare_metadata(worker, &self.cancel).map_err(|_| {
                if self.cancelled() {
                    Error::Cancelled
                } else {
                    Error::Resolve
                }
            })?;
            let (entry, authority) = plan.into_parts().map_err(|_| Error::Stale)?;
            if self.cancelled() {
                return Err(Error::Cancelled);
            }
            return Ok(PreparedMetadata {
                work: self,
                entry,
                plex: Some(authority),
            });
        }
        let entry = match self.binding.entry.clone() {
            Some(entry) => entry,
            None => if let Some(receipt) = &self.binding.attachment {
                ott_catalog::find_attachment_favorite(
                    worker,
                    receipt,
                    self.binding.favorite.as_ref().ok_or(Error::Stale)?,
                    &self.cancel,
                )
            } else {
                ott_catalog::find_favorite(
                    worker,
                    &self.binding.profile,
                    self.binding.favorite.as_ref().ok_or(Error::Stale)?,
                    self.permit.as_ref(),
                    &self.cancel,
                )
            }
            .map_err(|_| {
                if self.cancelled() {
                    Error::Cancelled
                } else {
                    Error::Resolve
                }
            })?,
        };
        if self.cancelled() {
            return Err(Error::Cancelled);
        }
        if !self.local
            && matches!(
                entry.media,
                MediaLocator::Local { .. } | MediaLocator::Plex { .. }
            )
        {
            return Err(Error::Unsupported);
        }
        Ok(PreparedMetadata {
            work: self,
            entry,
            plex: None,
        })
    }
}

/// Private metadata only. CatalogEntry.media may contain private source data; never serialize
/// it into a remote/backend command. Only the identity digest may cross that boundary.
pub(super) struct PreparedMetadata {
    work: SourceWork,
    entry: CatalogEntry,
    plex: Option<super::multiview_plex::InputAuthority>,
}
impl PreparedMetadata {
    pub fn ticket(&self) -> ResolveTicket {
        self.work.ticket
    }
    pub fn authorize_completion(
        self,
        runtime: &mut Runtime,
        app: &App,
    ) -> Result<AuthorizedInput, Error> {
        if self.work.cancelled() {
            return Err(Error::Cancelled);
        }
        if !self.work.current_for(runtime)
            || self.plex.as_ref().is_some_and(|proof| !proof.current())
        {
            return Err(Error::Stale);
        }
        let work = &self.work;
        runtime
            .authorize_multiview_source(
                app,
                &work.binding.request,
                &work.binding.profile,
                work.binding.entry.as_ref(),
                work.binding.favorite.as_ref(),
                Some(&self.entry),
            )
            .map_err(|_| Error::Policy)?;
        if self.work.cancelled()
            || !self.work.current_for(runtime)
            || self.plex.as_ref().is_some_and(|proof| !proof.current())
        {
            return Err(Error::Stale);
        }
        let digest = plx_data::ott::favorites_wire::hash(
            work.binding
                .favorite
                .as_ref()
                .map(Favorite::identity)
                .unwrap_or_else(|| {
                    format!(
                        "catalog:{}:{}",
                        work.binding.profile.id, work.binding.request.id
                    )
                })
                .as_bytes(),
        );
        Ok(AuthorizedInput {
            ticket: work.ticket,
            entry: self.entry,
            plex: self.plex,
            identity_digest: digest,
            work: self.work,
        })
    }
}
impl SourceWork {
    fn current_for(&self, runtime: &Runtime) -> bool {
        !self.cancelled()
            && self
                .local_read
                .as_ref()
                .is_none_or(|read| runtime.local_source_read_is_current(read))
            && self.runtime == runtime.remote.runtime()
            && (self.local || self.generation == runtime.generation)
            && self.profile_epoch != 0
            && self.profile_epoch
                == if self.local {
                    runtime.multiview_local_profile_epoch
                } else {
                    runtime.profile_edit_epoch
                }
            && self.authority_epoch != 0
            && self.authority_epoch == runtime.multiview_authority_epoch
            && self
                .binding
                .favorite
                .as_ref()
                .is_none_or(|favorite| runtime.saved.favorites.entries().contains(favorite))
            && self.binding.current(runtime)
    }
}

/// Keep the whole private receipt with an admitted input. Native metadata/witness does NOT
/// authorize a remote fetch. Server input commands use their independent opaque refs + digest.
pub(super) struct AuthorizedInput {
    pub ticket: ResolveTicket,
    pub entry: CatalogEntry,
    pub plex: Option<super::multiview_plex::InputAuthority>,
    identity_digest: String,
    work: SourceWork,
}
impl AuthorizedInput {
    /// Recheck expiring parental/source policy at the native Load boundary.
    /// A previously prepared descriptor is not a permanent permission grant.
    pub fn reauthorize(&self, runtime: &mut Runtime, app: &App) -> Result<(), Error> {
        if !self.current_for(runtime) {
            return Err(Error::Stale);
        }
        let binding = &self.work.binding;
        runtime
            .authorize_multiview_source(
                app,
                &binding.request,
                &binding.profile,
                binding.entry.as_ref(),
                binding.favorite.as_ref(),
                Some(&self.entry),
            )
            .map_err(|_| Error::Policy)?;
        if !self.current_for(runtime) {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub fn identity_scheme(&self) -> u8 {
        1
    }
    pub fn identity_digest(&self) -> &str {
        &self.identity_digest
    }
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.work.cancel.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.work.cancelled()
    }
    pub fn current_plex_authority(&self) -> bool {
        !self.is_cancelled() && self.plex.as_ref().is_none_or(|proof| proof.current())
    }
    pub fn current_for(&self, runtime: &Runtime) -> bool {
        self.work.current_for(runtime) && self.current_plex_authority()
    }
}

/// Fixture-only substitute for a metadata response after real capture. It cannot grant or open
/// a source and does not exist in production. All currentness/cancellation/policy gates remain.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn prepared_http_fixture_for_test(
    work: SourceWork,
    entry: CatalogEntry,
) -> Result<PreparedMetadata, Error> {
    if work.cancelled() {
        return Err(Error::Cancelled);
    }
    if work.binding.profile.kind != ProviderKind::M3u
        || !matches!(entry.media, MediaLocator::Http { .. })
        || !work
            .binding
            .favorite
            .as_ref()
            .is_some_and(|favorite| selection::matches_entry(favorite, &entry))
    {
        return Err(Error::Unsupported);
    }
    Ok(PreparedMetadata {
        work,
        entry,
        plex: None,
    })
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_multiview_sources_tests.rs"]
mod tests;

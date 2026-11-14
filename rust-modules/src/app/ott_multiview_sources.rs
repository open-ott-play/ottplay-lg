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

const MAX_ROWS: usize = 256;
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
    favorite: Favorite,
    binding: Option<Binding>,
}
#[derive(Clone)]
struct Binding {
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Favorite,
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
            favorite: favorite.clone(),
            attachment,
        })
    }
    fn current(&self, runtime: &Runtime) -> bool {
        Self::capture(runtime, &self.favorite).is_ok_and(|next| {
            self.request == next.request
                && self.profile == next.profile
                && self.entry == next.entry
                && self.favorite == next.favorite
                && self.attachment == next.attachment
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
            match self.favorite.locator {
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
        let epoch = next_id(&NEXT_SOURCE)?.get();
        let mut rows = Vec::new();
        for favorite in runtime.saved.favorites.entries().iter().take(MAX_ROWS) {
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
                favorite: favorite.clone(),
                binding,
            });
        }
        Ok(Self {
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
    pub fn rows(&self) -> impl Iterator<Item = &SourceRow> {
        self.rows.iter().map(|r| &r.view)
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
            && self.generation == runtime.generation
            && self.profile_epoch != 0
            && self.profile_epoch == runtime.profile_edit_epoch
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
        if !runtime.saved.favorites.entries().contains(&row.favorite) || !binding.current(runtime) {
            return Err(Error::Stale);
        }
        if matches!(
            binding.support(),
            Support::LocalRelayRequired | Support::Unavailable
        ) {
            return Err(Error::Unsupported);
        }
        let plex = if binding.profile.kind == ProviderKind::Plex {
            let session = format!(
                "mv-{}-{}",
                plx_data::ott::favorites_wire::hash(self.runtime.as_bytes()),
                next_id(&NEXT_SOURCE)?.get()
            );
            Some(
                super::multiview_plex::Capture::capture(
                    &binding.profile,
                    &binding.favorite,
                    session,
                )
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
                Some(&binding.favorite),
                None,
            )
            .map_err(|_| Error::Policy)?;
        let permit = runtime
            .favorite_permit(&binding.profile, &binding.favorite)
            .map_err(|_| Error::Policy)?;
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
                    &self.binding.favorite,
                    &self.cancel,
                )
            } else {
                ott_catalog::find_favorite(
                    worker,
                    &self.binding.profile,
                    &self.binding.favorite,
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
        if matches!(
            entry.media,
            MediaLocator::Local { .. } | MediaLocator::Plex { .. }
        ) {
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
                Some(&work.binding.favorite),
                Some(&self.entry),
            )
            .map_err(|_| Error::Policy)?;
        if self.work.cancelled()
            || !self.work.current_for(runtime)
            || self.plex.as_ref().is_some_and(|proof| !proof.current())
        {
            return Err(Error::Stale);
        }
        let digest =
            plx_data::ott::favorites_wire::hash(work.binding.favorite.identity().as_bytes());
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
            && self.runtime == runtime.remote.runtime()
            && self.generation == runtime.generation
            && self.profile_epoch != 0
            && self.profile_epoch == runtime.profile_edit_epoch
            && self.authority_epoch != 0
            && self.authority_epoch == runtime.multiview_authority_epoch
            && runtime
                .saved
                .favorites
                .entries()
                .contains(&self.binding.favorite)
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
                Some(&binding.favorite),
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
        || !selection::matches_entry(&work.binding.favorite, &entry)
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

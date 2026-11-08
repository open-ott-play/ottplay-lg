//! Runtime-owned temporary media. The accepted catalog is Runtime::catalog;
//! only an unadmitted candidate has a separate prefix. Nothing here is serialized.
use super::*;
use crate::app::ott_local_root::{LocalSource, RootHandle};
use crate::app::ott_local_session::{self as local, BoundSource, LocalContext, OwnerToken};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LocalSelection {
    pub source_id: String,
    pub authority: local::AuthorityRef,
    pub generation: u64,
    pub receipt: selection::LocalBindingReceipt,
}
impl std::fmt::Debug for LocalSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSelection")
            .field("source_id", &self.source_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
#[derive(Default)]
pub(super) struct Inventory {
    pub usb: super::usb_runtime::State,
    pub sources: Vec<LocalSource>,
    pub phase: InventoryPhase,
    // Status only; revoked roots remain in this same inventory. The existing
    // Saved writer is the sole durable owner, with no second queue or registry.
    pub proof_cleanup: ProofCleanup,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ProofCleanup {
    #[default]
    Clean,
    Pending,
    Writing,
    Failed,
}
#[derive(Clone, Copy, Default)]
pub(super) enum InventoryPhase {
    #[default]
    Unloaded,
    Loading(u64),
    Ready(u64),
    /// Required validation failed; an explicit command or lifecycle retry may re-arm it.
    Failed,
}
impl Inventory {
    pub fn generation(&self) -> u64 {
        match self.phase {
            InventoryPhase::Ready(generation) => generation,
            _ => 0,
        }
    }
    pub fn install(&mut self, mut sources: Vec<LocalSource>, generation: u64) {
        // A delayed candidate may retain a clone of a withdrawn root. Never
        // publish its old available bit, even after a successful Saved reply.
        for source in &mut sources {
            if !source.is_live() {
                *source = source.unavailable_observation();
            }
        }
        for old in &mut self.sources {
            let retained = sources.iter().any(|source|
                source.descriptor().id == old.descriptor().id && source.is_live()
                    && source.same_grant(old));
            if !retained { old.withdraw(); }
        }
        self.sources = sources;
        self.phase = InventoryPhase::Ready(generation);
    }
}
impl Drop for Inventory {
    fn drop(&mut self) {
        // This is the sole live inventory, not a cloned worker observation.
        // Retire cached/worker-held capabilities before releasing our references.
        for source in &mut self.sources { source.withdraw(); }
    }
}
#[derive(Default)]
pub(super) struct State {
    pub accepted: Option<TransientMedia>,
    pub candidate: Option<Candidate>,
}
impl State {
    pub(super) fn owns_playback(&self) -> bool {
        self.candidate.as_ref().is_some_and(Candidate::is_play)
    }
}
pub(super) enum TransientMedia {
    Local(LocalSession),
    Plex(plex_remote::PlexSession),
}
#[derive(Clone)]
pub(super) struct LocalRootRead {
    pub selection: LocalSelection,
    pub root: Arc<RootHandle>,
}
/// An exact live Local capability. A file variant never exposes a directory.
#[derive(Clone)]
pub(super) struct LocalSourceRead {
    pub selection: LocalSelection,
    pub source: LocalSource,
}
impl LocalSourceRead {
    pub(super) fn is_live(&self) -> bool { self.source.is_live() }
    /// Identity only; callers separately validate liveness and selection epochs.
    pub(super) fn same_grant(&self, other: &Self) -> bool {
        self.source.same_grant(&other.source)
    }
    pub(super) fn folder_read(&self) -> Option<LocalRootRead> {
        Some(LocalRootRead { selection: self.selection.clone(),
            root: self.source.root_handle()?.clone() })
    }
}
pub(super) struct LocalSession {
    pub read: LocalSourceRead,
    pub context: LocalContext,
    pub profile: ProviderProfile,
    /// Present only when the currently admitted source is outside this folder.
    pub borrowed: Option<BorrowedBookmark>,
    /// Explicit durable kiosk activation only; ordinary temporary sessions use None.
    pub kiosk_resume: Option<kiosk_resume::Scope>,
}
pub(super) struct BorrowedBookmark {
    pub selection_id: String,
    pub profile_fingerprint: String,
    pub position_ms: u64,
}
pub(super) struct InterruptedScan {
    pub owner: Option<OwnerToken>,
    pub profile: Option<ProviderProfile>,
    pub restart: bool,
}
pub(super) enum Candidate {
    Local(LocalCandidate),
    Plex(plex_remote::PlexCandidate),
}
impl Candidate {
    fn is_play(&self) -> bool {
        match self {
            Self::Local(c) => local_remote::is_play(&c.request),
            Self::Plex(c) => c.is_play(),
        }
    }
}
impl State {
    pub(super) fn local_candidate(&self) -> Option<&LocalCandidate> {
        match self.candidate.as_ref()? {
            Candidate::Local(c) => Some(c),
            _ => None,
        }
    }
    pub(super) fn local_candidate_mut(&mut self) -> Option<&mut LocalCandidate> {
        match self.candidate.as_mut()? {
            Candidate::Local(c) => Some(c),
            _ => None,
        }
    }
    pub(super) fn take_local_candidate(&mut self) -> Option<LocalCandidate> {
        if !matches!(self.candidate, Some(Candidate::Local(_))) {
            return None;
        }
        match self.candidate.take()? {
            Candidate::Local(c) => Some(c),
            _ => None,
        }
    }
    pub(super) fn put_local_candidate(&mut self, candidate: LocalCandidate) {
        self.candidate = Some(Candidate::Local(candidate));
    }
}
pub(super) struct LocalCandidate {
    pub inventory: Vec<LocalSource>,
    pub admission_ticket: plx_media::route::PlayRequestTicket,
    pub owner: OwnerToken,
    pub purpose: local_remote::LocalPurpose,
    pub request: local::LocalRequest,
    pub pending: local::PendingLocal,
    pub interrupted: InterruptedScan,
    pub phase: CandidatePhase,
}
pub(super) enum CandidatePhase {
    Binding(Option<ott_catalog::LocalBindingRequest>),
    Scanning {
        source: BoundSource,
        profile: ProviderProfile,
        catalog: Catalog,
        state: local::ScanState,
        proposed: Option<LocalContext>,
    },
    SavingConnection(BoundSource),
    Failed(local::LocalError),
}
impl TransientMedia {
    pub fn owner(&self) -> OwnerToken {
        match self {
            Self::Local(value) => value.context.owner(),
            Self::Plex(value) => OwnerToken(value.queue.owner().0),
        }
    }
    pub fn profile(&self) -> &ProviderProfile {
        match self {
            Self::Local(value) => &value.profile,
            Self::Plex(value) => &value.profile,
        }
    }
    pub fn selection(&self) -> (&str, bool) {
        match self {
            Self::Local(value) => value.context.view_selection(),
            Self::Plex(value) => (&value.category, value.favorites_selected),
        }
    }
    pub fn root(&self) -> Option<&BoundSource> {
        match self {
            Self::Local(value) => Some(value.context.source()),
            Self::Plex(_) => None,
        }
    }
}

impl Runtime {
    pub(super) fn retire_closed_source(&mut self, app: &mut App) {
        let stale = self.playing.as_ref().is_some_and(|playing| {
            playing.requires_server_lease
                && playing_matches(playing, app)
                && !self
                    .active_lease
                    .as_ref()
                    .is_some_and(HeldLease::is_playback)
        });
        if stale {
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
            // cancel_play retires requests but intentionally preserves an installed
            // source for ordinary suspend. This exact owned lease has ended, so
            // its URL and playback identity must no longer be resumable.
            plx_media::route::forget_external_source(&mut app.player.session);
            self.playing = None;
            self.continuity.stop(self.generation);
        }
    }

    pub(super) fn effective_profile(&self) -> Option<&ProviderProfile> {
        self.transient
            .accepted
            .as_ref()
            .map(TransientMedia::profile)
            .or_else(|| managed::profile(self))
            .or_else(|| classic_source::profile(self))
            .or_else(|| self.active())
    }
    pub(super) fn effective_provider(&self) -> ProviderKind {
        self.effective_profile()
            .map(|p| p.kind)
            .unwrap_or(self.provider)
    }
    pub(super) fn view_selection(&self) -> (&str, bool) {
        let category = self.transient.accepted.as_ref()
            .map(|source| source.selection().0)
            .or_else(|| managed::view_selection(self).map(|selection| selection.0))
            .or_else(|| classic_source::view_selection(self))
            .unwrap_or(&self.saved.category);
        // Favorites is an installation preference, never a private queue flag.
        (category, self.saved.favorites_selected)
    }
    pub(super) fn selection_bindings(
        &self,
    ) -> Result<selection::SelectionBindings<'_>, selection::BindingError> {
        selection::SelectionBindings::new(
            &self.saved.profiles,
            self.local_inventory.sources.iter().map(LocalSource::descriptor),
            self.local_inventory.generation(),
        ).map(|bindings|bindings.with_temporary_profile(managed::binding_profile(self).or_else(|| classic_source::binding_profile(self)))
            .with_attachments(&self.m3u_attachments))
    }
    pub(super) fn resume_waits_for_bindings(&self, value: &SavedResume) -> bool {
        !matches!(self.local_inventory.phase, InventoryPhase::Ready(_))
            && crate::app::ott_local_sources::RememberedFolders::read(&self.saved.settings)
                .is_ok_and(|folders| !folders.folders().is_empty())
            && value.entry_id.strip_prefix("favorite:").is_some_and(|id| {
                self.saved.favorites.entries().iter().any(|favorite| {
                    favorite.identity() == id
                        && favorite.origin.provider == "local"
                        && favorite.origin.profile == "Local videos"
                        && matches!(
                            favorite.locator,
                            plx_data::ott::favorites::Locator::Local { .. }
                        )
                })
            })
    }
    /// Unrelated provider favorites do not depend on Local root validation.
    /// Local entries retain the complete inventory and exact receipt checks.
    pub(super) fn favorite_bindings(
        &self,
        favorite: &Favorite,
    ) -> Result<selection::SelectionBindings<'_>, selection::BindingError> {
        if favorite.origin.provider == "local" {
            self.selection_bindings()
        } else {
            Ok(selection::SelectionBindings::profiles_only(&self.saved.profiles)
                .with_temporary_profile(managed::binding_profile(self).or_else(|| classic_source::binding_profile(self)))
                .with_attachments(&self.m3u_attachments))
        }
    }
    pub(super) fn validate_bound_resume(&mut self) {
        let Some(pending) = self.continuity.pending_restore() else {
            return;
        };
        let resume = self
            .resume_key()
            .and_then(|key| {
                self.checkpoints
                    .get(&key)
                    .or_else(|| self.saved.resume.get(&key))
            })
            .filter(|saved| self.resume_waits_for_bindings(saved) || self.resume_is_current(saved));
        let valid = resume.map_or_else(
            || self.resume_key().is_some()
                && commands::offline_selected_resume(&self.saved).as_ref() == Some(pending),
            |saved| saved.entry_id == pending.entry_id,
        );
        if !valid {
            self.continuity = continuity::Continuity::new(self.generation, None);
        }
    }
    pub(super) fn row_owner(&self) -> Option<selection::SelectionOwner<'_>> {
        match &self.transient.accepted {
            Some(TransientMedia::Local(owner)) => {
                Some(selection::SelectionOwner::LocalRoot(owner.context.source()))
            }
            Some(TransientMedia::Plex(owner)) => {
                Some(selection::SelectionOwner::Profile(&owner.profile))
            }
            None => managed::profile(self).or_else(|| classic_source::profile(self)).or_else(|| self.active()).map(|profile| {
                if profile.kind == ProviderKind::Local {
                    if let Some(source) = self.local_profile_selection(profile)
                        .and_then(|selected| self.current_local_source(&selected))
                        .filter(|source| source.kind == local::SourceKind::File) {
                        return selection::SelectionOwner::LocalRoot(source);
                    }
                }
                selection::SelectionOwner::Profile(profile)
            }),
        }
    }
    pub(super) fn bind_local_selection(
        &self,
        root: &BoundSource,
    ) -> Result<LocalSelection, &'static str> {
        let receipt = self
            .selection_bindings()
            .map_err(|_| "Local source inventory is invalid")?
            .receipt(selection::SelectionOwner::LocalRoot(root))
            .map_err(|_| "Local source binding changed")?
            .ok_or("Local source binding is missing")?;
        Ok(LocalSelection {
            source_id: root.id.clone(),
            authority: root.authority_ref,
            generation: root.generation,
            receipt,
        })
    }
    pub(super) fn local_catalog_task(&self, profile: ProviderProfile) -> Result<ott_catalog::Task, &'static str> {
        if profile.kind != ProviderKind::Local || plx_data::ott::http_url(&profile.source).is_some() {
            return Ok(ott_catalog::Task::Provider(profile));
        }
        let sources = self.transient.local_candidate().and_then(|candidate| {
            if matches!(&candidate.phase, CandidatePhase::Scanning { profile: staged, .. } if staged == &profile) {
                Some(candidate.inventory.as_slice())
            } else { None }
        }).unwrap_or(&self.local_inventory.sources);
        let matches: Vec<_> = sources.iter().filter(|source|
            source.descriptor().canonical_path.as_deref() == Some(profile.source.as_str())).collect();
        if matches.iter().any(|source| source.descriptor().kind == local::SourceKind::File) {
            if matches.len() != 1 { return Err("Local file source is ambiguous"); }
            let source = matches[0];
            if !source.is_live() { return Err("Local file source is unavailable"); }
            return Ok(ott_catalog::Task::LocalFile { profile, source: source.clone() });
        }
        if matches.len() != 1 { return Err("Local folder source is unavailable or ambiguous"); }
        let source = matches[0];
        if !source.is_live() { return Err("Local folder source is unavailable"); }
        Ok(ott_catalog::Task::LocalFolder { profile, source: source.clone() })
    }
    pub(super) fn current_local_source(&self, selected: &LocalSelection) -> Option<&BoundSource> {
        let source = self.local_inventory.sources.iter()
            .find(|source| source.descriptor().id == selected.source_id)?;
        if !source.is_live() { return None; }
        let source = source.descriptor();
        (source.available && source.authority_ref == selected.authority
            && source.generation == selected.generation).then_some(source)
    }
    pub(super) fn current_local_root(&self, selected: &LocalSelection) -> Option<&BoundSource> {
        let source = self.current_local_source(selected)?;
        (source.kind == local::SourceKind::Folder).then_some(source)
    }
    pub(super) fn capture_local_source(&self, selected: &LocalSelection) -> Option<LocalSourceRead> {
        let source = self.current_local_source(selected)?;
        let read = capture_source_read(&self.local_inventory.sources, &self.saved.profiles,
            self.local_inventory.generation(), source)?;
        (read.selection == *selected).then_some(read)
    }
    pub(super) fn local_source_read_is_current(&self, read: &LocalSourceRead) -> bool {
        let Some(source) = self.current_local_source(&read.selection) else { return false; };
        capture_source_read(&self.local_inventory.sources, &self.saved.profiles,
            self.local_inventory.generation(), source)
            .is_some_and(|current| current.is_live() && current.same_grant(read))
    }
    pub(super) fn capture_local_root(&self, selected: &LocalSelection) -> Option<LocalRootRead> {
        let source = self.current_local_root(selected)?;
        let read = capture_root_read(&self.local_inventory.sources, &self.saved.profiles,
            self.local_inventory.generation(), source)?;
        (read.selection == *selected).then_some(read)
    }
    pub(super) fn local_root_read_is_current(&self, read: &LocalRootRead) -> bool {
        // A settled source remains valid across an unrelated inventory refresh.
        // Delayed request admission additionally validates its captured receipt.
        let Some(source) = self.current_local_root(&read.selection) else { return false; };
        capture_root_read(&self.local_inventory.sources, &self.saved.profiles,
            self.local_inventory.generation(), source)
            .is_some_and(|current| Arc::ptr_eq(&current.root, &read.root))
    }
    pub(super) fn folder_row_for(&self, playing: &Playing) -> Option<String> {
        let accepted = self.transient.accepted.as_ref()?;
        let Some(TransientMedia::Local(session)) = self.transient.accepted.as_ref() else { return None; };
        if !self.local_source_read_is_current(&session.read) { return None; }
        if Some(accepted.owner()) != playing.transient_owner {
            return None;
        }
        let binding = playing.local_selection.as_ref()?;
        let root = self.current_local_source(binding)?;
        let accepted_root = accepted.root()?;
        if root.id != accepted_root.id
            || root.authority_ref != accepted_root.authority_ref
            || root.generation != accepted_root.generation
        {
            return None;
        }
        let MediaLocator::Local { relative_path } = &playing.entry.media else {
            return None;
        };
        self.catalog.entries.iter().find(|entry|
            matches!(&entry.media, MediaLocator::Local {relative_path:path} if path == relative_path))
            .map(|entry| entry.id.clone())
    }
    /// A newly accepted selection may replay the same row after Stop. Ordinary
    /// checkpoints cannot grant this intent, nor can an in-place continuation.
    pub(super) fn transient_selection_admitted(&mut self, playing: &Playing) {
        let Some(id) = self.folder_row_for(playing) else { return; };
        let Some(TransientMedia::Local(session)) = self.transient.accepted.as_mut() else { return; };
        let _ = session.context.note_intent(session.context.owner(), local::LocalIntent::Selected(&id));
    }
    /// A stale temporary owner also returns true: it cannot leak a bookmark into
    /// durable state after its session was retired.
    pub(super) fn transient_checkpoint(&mut self, playing: &Playing, position_ms: u64) -> bool {
        // Managed publication is nondurable even after its catalogue was
        // cancelled; never turn its playing item into a Saved resume record.
        if managed::owns_profile(self, &playing.profile)
            || classic_source::temporary_id(self, &playing.profile.id) { return true; }
        let Some(owner) = playing.transient_owner else {
            return false;
        };
        if plex_remote::checkpoint(self, playing, position_ms) {
            return true;
        }
        let folder_id = self.folder_row_for(playing);
        let Some(TransientMedia::Local(session)) = self
            .transient
            .accepted
            .as_mut()
            .filter(|value| value.owner() == owner)
        else {
            return true;
        };
        if let Some(id) = folder_id {
            if session.context.current() != Some(id.as_str()) {
                let _ = session
                    .context
                    .note_intent(owner, local::LocalIntent::Selected(&id));
            }
            let _ = session.context.checkpoint(owner, &id, position_ms);
            session.borrowed = None;
        } else {
            session.borrowed = Some(BorrowedBookmark {
                selection_id: playing.selection_id.clone(),
                profile_fingerprint: profile_fingerprint(&playing.profile),
                position_ms,
            });
        }
        true
    }
}

#[path = "ott_transient_local.rs"]
mod local_remote;
pub(super) use local_remote::*;
pub(in crate::app) use local_remote::LocalKioskPreparation as LocalKioskPreparationReceipt;
pub(super) use crate::app::ott_kiosk_preparation::KioskPreparation;

pub(super) fn kiosk_preparation_is_current(runtime: &Runtime, app: &App, receipt: &KioskPreparation) -> bool {
    match (receipt.local(), receipt.plex()) {
        (Some(local), _) => local_remote::kiosk_preparation_is_current(runtime, app, local),
        (_, Some(plex)) => plex_remote::kiosk_preparation_is_current(runtime, app, plex),
        _ => false,
    }
}
pub(super) fn retry_kiosk_preparation(runtime: &mut Runtime, app: &mut App, receipt: &KioskPreparation)
    -> Result<(), work_lifecycle::RestoreStartError> {
    match (receipt.local(), receipt.plex()) {
        (Some(local), _) => local_remote::retry_kiosk_preparation(runtime, app, local),
        (_, Some(plex)) => plex_remote::retry_kiosk_preparation(runtime, app, plex),
        _ => Err(work_lifecycle::RestoreStartError::Invalid),
    }
}
pub(super) fn kiosk_preparation_resume_policy(runtime: &Runtime, app: &App, receipt: &KioskPreparation)
    -> Option<crate::app::ott_kiosk_stored::Stored> {
    match (receipt.local(), receipt.plex()) {
        (Some(local), _) => local_remote::kiosk_preparation_resume_policy(runtime, app, local),
        (_, Some(plex)) => plex_remote::kiosk_preparation_resume_policy(runtime, app, plex),
        _ => None,
    }
}
pub(super) fn staged_native_binding(runtime: &Runtime, request: &PlaybackRequest)
    -> Option<Result<(ProviderProfile,CatalogEntry), &'static str>> {
    local_remote::staged_native_binding(runtime, request)
        .or_else(|| plex_remote::staged_native_binding(runtime, request))
}

#[path = "ott_transient_plex.rs"]
pub(super) mod plex_remote;

fn capture_source_read(sources: &[LocalSource], profiles: &plx_data::ott::ProviderProfiles,
    generation: u64, source: &BoundSource) -> Option<LocalSourceRead> {
    let record = sources.iter().find(|row| row.descriptor().id == source.id)?;
    if !record.is_live() { return None; }
    let receipt = selection::SelectionBindings::new(profiles,
        sources.iter().map(LocalSource::descriptor), generation).ok()?
        .receipt(selection::SelectionOwner::LocalRoot(source)).ok()??;
    Some(LocalSourceRead { selection: LocalSelection {
        source_id: source.id.clone(), authority: source.authority_ref,
        generation: source.generation, receipt,
    }, source: record.clone() })
}
fn capture_root_read(sources: &[LocalSource], profiles: &plx_data::ott::ProviderProfiles,
    generation: u64, source: &BoundSource) -> Option<LocalRootRead> {
    capture_source_read(sources, profiles, generation, source)?.folder_read()
}
pub(super) use plex_remote::{
    kiosk_queue, kiosk_queue_allows, kiosk_queue_for_status, kiosk_queue_is_current, kiosk_queue_matches_current,
    kiosk_repeat_all, kiosk_restore_allows, kiosk_restore_queue,
};


#[path = "ott_transient_parental.rs"]
mod parental_source;
pub(super) use parental_source::{parental_source_authority, SourceAuthority};

#[path = "ott_local_video.rs"]
mod local_video;
pub(super) use local_video::{capture_video_root, open_video_source};

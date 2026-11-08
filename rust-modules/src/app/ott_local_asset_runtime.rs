//! Local optional assets on the existing metadata work slot. The source proof
//! stays in the application; the native track owner receives opaque targets.
use super::*;
use super::asset_selection;
use super::super::ott_remote::media_tracks as wire;
use plx_media::player::sidecar;
use plx_media::route::{self, ExternalTracks, LocalSubtitleTarget, LocalSubtitleTrack};
use plx_machine::machine::PosterKey;
use plx_platform::storage::readonly::ReadError;

#[path = "ott_local_asset_worker.rs"]
mod worker;

#[derive(Clone)]
struct Context {
    profile: ProviderProfile,
    entry: CatalogEntry,
    selection_id: String,
    read: transient::LocalRootRead,
    playback: Option<route::PlaybackIdentity>,
    native: Option<ExternalTracks>,
}
impl Context {
    fn path(&self) -> &str {
        match &self.entry.media { MediaLocator::Local { relative_path } => relative_path, _ => "" }
    }
    fn current(&self, runtime: &Runtime, app: &App) -> bool {
        if !runtime.capture_local_root(&self.read.selection)
            .is_some_and(|read| Arc::ptr_eq(&read.root, &self.read.root))
        { return false; }
        if let Some(identity) = &self.playback {
            route::has_url(&app.player.session)
                && !matches!(plx_media::player::state(&app.player.session),
                    plx_media::player::PlaybackState::Idle | plx_media::player::PlaybackState::Error)
                && runtime.playing.as_ref().is_some_and(|playing|
                    playing.identity.as_ref() == Some(identity)
                        && playing.profile == self.profile
                        && playing.entry.media == self.entry.media
                        && playing.selection_id == self.selection_id
                        && playing_matches(playing, app)
                        && !playing_superseded(playing, app))
        } else {
            runtime.metadata_target.as_ref() == Some(&(self.profile.id.clone(), self.selection_id.clone()))
        }
    }
}

#[derive(Clone)]
enum WorkKind {
    Preview,
    Discover { capacity: usize },
    Read { request: sidecar::LocalReadRequest, path: String },
}
#[derive(Clone)]
pub(in crate::app) struct Work { context: Context, kind: WorkKind }
pub(in crate::app) struct Completion { context: Context, result: Outcome }
enum Outcome {
    Preview(Result<Discovered, ReadError>),
    Discover(Result<Discovered, ReadError>),
    Read { request: sidecar::LocalReadRequest, result: Result<sidecar::PreparedLocal, sidecar::LocalFailure> },
}
struct Discovered {
    entry: CatalogEntry,
    poster: Option<String>,
    subtitles: Vec<DiscoveredSubtitle>,
    complete: bool,
}
struct DiscoveredSubtitle { path: String, format: sidecar::SidecarFormat, title: String, language: String }
struct BoundSubtitle { target: LocalSubtitleTarget, path: String }
struct Installed { context: Context, subtitles: Vec<BoundSubtitle>, art: Option<PosterKey> }
struct Preview { context: Context, art: Option<PosterKey> }

#[derive(Default)]
enum Purpose {
    #[default] Preview,
    Discover(Context),
    Publish(Context),
    Read(Context, sidecar::LocalReadRequest),
}
impl Purpose {
    fn is_preview(&self) -> bool { matches!(self, Self::Preview) }
    fn context(&self) -> Option<&Context> {
        match self { Self::Preview => None, Self::Discover(context) | Self::Publish(context) | Self::Read(context, _) => Some(context) }
    }
}
#[derive(Default)]
pub(super) struct State {
    purpose: Purpose,
    installed: Option<Installed>,
    preview: Option<Preview>,
}

impl Work {
    pub(in crate::app) fn failure(self, worker: &plx_base::task::OffFrame) -> Completion {
        let result = match self.kind {
            WorkKind::Read { request, .. } => Outcome::Read {
                result: Err(request.read_failed(worker)), request,
            },
            WorkKind::Preview => Outcome::Preview(Err(ReadError::Io(std::io::ErrorKind::Other))),
            WorkKind::Discover { .. } => Outcome::Discover(Err(ReadError::Io(std::io::ErrorKind::Other))),
        };
        Completion { context: self.context, result }
    }
}

fn bound_context(runtime: &Runtime, profile: ProviderProfile, entry: CatalogEntry,
    selection_id: String, selection: &transient::LocalSelection,
    playback: Option<route::PlaybackIdentity>, native: Option<ExternalTracks>) -> Option<Context>
{
    if profile.kind != ProviderKind::Local || !matches!(entry.media, MediaLocator::Local { .. }) { return None; }
    let root = runtime.current_local_root(selection)?;
    if root.canonical_path.as_deref() != Some(profile.source.as_str()) { return None; }
    // A retained logical playback may carry the previous inventory receipt.
    // Rebind only its exact live source, never a different ID/authority/root.
    let current = runtime.bind_local_selection(root).ok()?;
    let read = runtime.capture_local_root(&current)?;
    Some(Context { profile, entry, selection_id, read, playback, native })
}

fn playing_context(runtime: &Runtime, app: &App) -> Option<Context> {
    let playing = runtime.playing.as_ref()?;
    if !playing_matches(playing, app) || playing_superseded(playing, app)
        || !route::has_url(&app.player.session)
        || matches!(plx_media::player::state(&app.player.session),
            plx_media::player::PlaybackState::Idle | plx_media::player::PlaybackState::Error)
    { return None; }
    let selection = playing.local_selection.clone()
        .or_else(|| runtime.local_profile_selection(&playing.profile))?;
    bound_context(runtime, playing.profile.clone(), playing.entry.clone(),
        playing.selection_id.clone(), &selection,
        Some(playing.identity.as_ref()?.clone()), route::external_tracks(&app.player.session).cloned())
}

fn same_source(first: &Context, second: &Context) -> bool {
    first.playback == second.playback && first.profile == second.profile
        && first.entry.media == second.entry.media && first.selection_id == second.selection_id
        && first.read.selection.source_id == second.read.selection.source_id
        && first.read.selection.authority == second.read.selection.authority
        && first.read.selection.generation == second.read.selection.generation
        && Arc::ptr_eq(&first.read.root, &second.read.root)
}

impl Runtime {
    // A durable Local profile can reuse an existing unambiguous authorized
    // root. Matching a pathname never creates or refreshes a grant.
    pub(super) fn local_profile_selection(&self, profile: &ProviderProfile) -> Option<transient::LocalSelection> {
        if profile.kind != ProviderKind::Local { return None; }
        let mut roots = self.local_inventory.sources.iter().filter(|root|
            root.descriptor().canonical_path.as_deref() == Some(profile.source.as_str()));
        let root = roots.next()?;
        if roots.next().is_some() { return None; }
        self.bind_local_selection(root.descriptor()).ok()
    }

    /// The logical hover target may change, but cannot preempt an explicit read
    /// or the first optional discovery. Refresh it through the same slot later.
    pub(super) fn defer_preview_for_local_assets(&mut self, profile: &ProviderProfile, id: &str) -> bool {
        if self.local_assets.purpose.is_preview() { return false; }
        self.metadata_target = Some((profile.id.clone(), id.to_owned()));
        self.metadata_query = None;
        self.metadata_refresh_ms = 1;
        true
    }

    pub(super) fn local_asset_work_ready(&self) -> Option<bool> {
        (!self.local_assets.purpose.is_preview()).then(||
            matches!(self.local_inventory.phase, transient::InventoryPhase::Ready(_)))
    }

    pub(super) fn suspend_local_assets(&mut self) {
        // The lifecycle already cancelled/incremented the metadata generation.
        // Keep true worker occupancy and the logical hover target unchanged.
        self.metadata_query = None;
        self.local_assets.purpose = Purpose::Preview;
        sidecar::reset_local();
    }

    fn cancel_local_asset_work(&mut self) {
        if matches!(&self.local_assets.purpose, Purpose::Read(_, request)
            if sidecar::local_request_is_current(request)) { sidecar::reset_local(); }
        self.metadata_cancel.store(true, Ordering::Release);
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_next = None;
        self.metadata_query = None;
        self.local_assets.purpose = Purpose::Preview;
        self.metadata_refresh_ms = 1;
    }

    /// Thread creation failure is terminal for this discovery/selection. A new
    /// source or explicit selection may retry; a render tick must not retry it.
    pub(super) fn local_asset_start_failed(&mut self) {
        let purpose = std::mem::take(&mut self.local_assets.purpose);
        match purpose {
            Purpose::Read(_, request) => { sidecar::fail_local_start(&request); }
            Purpose::Discover(context) => {
                self.local_assets.installed = Some(Installed { context, subtitles: Vec::new(), art: None });
            }
            Purpose::Preview | Purpose::Publish(_) => {}
        }
        self.metadata_refresh_ms = 1;
    }

    fn enqueue_local_asset(&mut self, work: Work) {
        self.metadata_cancel.store(true, Ordering::Release);
        self.metadata_cancel = Arc::new(AtomicBool::new(false));
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_query = None;
        self.local_assets.purpose = match &work.kind {
            WorkKind::Preview => Purpose::Preview,
            WorkKind::Discover { .. } => Purpose::Discover(work.context.clone()),
            WorkKind::Read { request, .. } => Purpose::Read(work.context.clone(), request.clone()),
        };
        self.metadata_next = Some((work.context.profile.clone(), MetadataSelection::Local(work)));
        self.launch_metadata();
    }

    pub(super) fn request_local_asset_preview(&mut self, profile: ProviderProfile, entry: CatalogEntry,
        selection_id: String, selection: transient::LocalSelection) -> bool
    {
        if self.defer_preview_for_local_assets(&profile, &selection_id) { return true; }
        let Some(context) = bound_context(self, profile, entry, selection_id.clone(), &selection, None, None) else { return false; };
        self.metadata_target = Some((context.profile.id.clone(), selection_id));
        self.metadata_query = None;
        self.metadata_local_selection = None;
        // Optional reads yield to initial playback. Browsing without a pending
        // or retained playback can still show local metadata before selection.
        if !plx_media::player::seen_frame() && (self.play_target.is_some() || self.playing.is_some()) {
            self.metadata_refresh_ms = 1;
            return true;
        }
        if let Some(installed) = self.local_assets.installed.as_ref().filter(|installed|
            installed.context.profile == context.profile
                && installed.context.entry.media == context.entry.media
                && self.local_root_read_is_current(&installed.context.read))
        {
            self.local_assets.preview = Some(Preview { context, art: installed.art });
            self.metadata_refresh_ms = 0;
            return true;
        }
        self.enqueue_local_asset(Work { context, kind: WorkKind::Preview });
        true
    }

    pub(super) fn poll_local_assets(&mut self, app: &mut App) {
        if !self.source_work_allowed() || !playback_allowed(app) { return; }
        let stale = self.local_assets.purpose.context()
            .is_some_and(|context| !context.current(self, app));
        let retired = (!self.local_assets.purpose.is_preview() && self.metadata_cancel.load(Ordering::Acquire))
            || matches!(&self.local_assets.purpose, Purpose::Read(_, request)
                if !sidecar::local_request_is_current(request));
        if stale || retired { self.cancel_local_asset_work(); }
        if matches!(self.metadata_next.as_ref(), Some((_, MetadataSelection::LocalPublish(_)))) {
            if let Some((_, MetadataSelection::LocalPublish(completion))) = self.metadata_next.take() {
                self.accept_local_assets(app, self.metadata_generation, completion);
            }
            if !self.local_assets.purpose.is_preview() { return; }
        }
        let Some(context) = playing_context(self, app) else {
            // Pending validation is not proof of loss. The lifecycle keeps the
            // owner closed until it can distinguish this from a changed source.
            if !matches!(self.local_inventory.phase, transient::InventoryPhase::Ready(_)) { return; }
            if self.local_assets.installed.take().is_some() { sidecar::reset_local(); }
            return;
        };
        if self.local_assets.installed.as_ref().is_some_and(|old| !same_source(&old.context, &context)
            || !old.context.native.as_ref().zip(context.native.as_ref()).is_some_and(|(a, b)| a.same_session(b))) {
            self.local_assets.installed = None;
            sidecar::reset_local();
        }
        if let Some(installed) = self.local_assets.installed.as_mut() {
            // Refresh only a verified same-source receipt after lifecycle wake.
            installed.context.read = context.read.clone();
            if let Some(request) = sidecar::peek_local_request() {
                if request.playback() != context.playback.as_ref().unwrap() { sidecar::reset_local(); return; }
                let Some(path) = installed.subtitles.iter().find(|row| &row.target == request.target()).map(|row| row.path.clone()) else {
                    sidecar::reset_local(); return;
                };
                let Some(request) = sidecar::take_local_request() else { return; };
                self.enqueue_local_asset(Work { context, kind: WorkKind::Read { request, path } });
                return;
            }
            // A suspended selected read was retired. The route still owns the
            // choice and can mint a fresh ticket after this exact root rebind.
            if self.local_assets.purpose.is_preview()
                && !sidecar::selected()
                && route::external_tracks(&app.player.session).is_some_and(|tracks| tracks.selected_local_subtitle().is_some())
            {
                let _ = route::rearm_selected_local_subtitle(&app.player.session);
            }
            return;
        }
        if !self.local_assets.purpose.is_preview() || !plx_media::player::seen_frame() { return; }
        let Some(native) = context.native.as_ref() else { return; };
        let inventory = native.inventory();
        let capacity = wire::MAX_TRACKS_PER_KIND.saturating_sub(inventory.subtitles.len())
            .min(wire::MAX_TRACKS.saturating_sub(inventory.audio.len().saturating_add(inventory.subtitles.len())));
        // Publish that adjacent discovery is incomplete before starting IO.
        // A spawn refusal therefore cannot advertise a complete empty inventory.
        let Some(identity) = context.playback.as_ref() else { return; };
        match route::install_external_local_subtitles(&mut app.player.session, identity, native,
            Vec::new(), false, wire::MAX_TRACKS, wire::MAX_TRACKS_PER_KIND)
        {
            Err(route::ExternalTrackApplyError::Busy) => return,
            Err(_) => {
                self.local_assets.installed = Some(Installed { context, subtitles: Vec::new(), art: None });
                return;
            }
            Ok(_) => {}
        }
        let mut context = context;
        context.native = route::external_tracks(&app.player.session).cloned();
        self.changed();
        self.enqueue_local_asset(Work { context, kind: WorkKind::Discover { capacity } });
    }

    pub(super) fn accept_local_assets(&mut self, app: &mut App, generation: u64, completion: Completion) {
        if generation != self.metadata_generation { return; }
        self.local_assets.purpose = Purpose::Preview;
        self.metadata_refresh_ms = 1;
        if !self.source_work_allowed() || !completion.context.current(self, app) {
            // Never rebind a worker result to a fresh receipt. Retire only its
            // own still-current ticket; the verified live owner may rearm later.
            if let Outcome::Read { request, .. } = &completion.result {
                if sidecar::local_request_is_current(request) { sidecar::reset_local(); }
            }
            return;
        }
        let Completion { context, result } = completion;
        match result {
            Outcome::Read { request, result } => {
                match result {
                    Ok(prepared) => { sidecar::install_local(&request, prepared); }
                    Err(error) => { sidecar::fail_local(&request, error); }
                }
            }
            Outcome::Preview(result) => {
                self.metadata_refresh_ms = 0;
                if let Ok(data) = result {
                    let art = self.apply_local_metadata(&context, &data);
                    self.local_assets.preview = Some(Preview { context, art });
                }
            }
            Outcome::Discover(result) => {
                let data = result.unwrap_or_else(|_| Discovered {
                    entry: context.entry.clone(), poster: None, subtitles: Vec::new(), complete: false,
                });
                let mut rows = Vec::new();
                let mut bindings = Vec::new();
                for source in &data.subtitles {
                    let target = LocalSubtitleTarget::new();
                    rows.push(LocalSubtitleTrack { target: target.clone(), format: source.format, title: source.title.clone(), language: source.language.clone() });
                    bindings.push(BoundSubtitle { target, path: source.path.clone() });
                }
                let Some((identity, native)) = context.playback.as_ref().zip(context.native.as_ref()) else { return; };
                match route::install_external_local_subtitles(&mut app.player.session,
                    identity, native, rows, data.complete, wire::MAX_TRACKS, wire::MAX_TRACKS_PER_KIND)
                {
                    Err(route::ExternalTrackApplyError::Busy) => {
                        // Reuse the same metadata pending slot for completed data;
                        // do not re-read the directory or start a second worker.
                        self.local_assets.purpose = Purpose::Publish(context.clone());
                        self.metadata_next = Some((context.profile.clone(), MetadataSelection::LocalPublish(
                            Completion { context, result: Outcome::Discover(Ok(data)) })));
                        return;
                    }
                    Err(_) => {
                        // A rejected publication is terminal for this exact source
                        // and catalog. A later catalog/source change may retry.
                        self.local_assets.installed = Some(Installed { context, subtitles: Vec::new(), art: None });
                        return;
                    }
                    Ok(_) => {}
                }
                let art = self.apply_local_metadata(&context, &data);
                let mut context = context;
                context.native = route::external_tracks(&app.player.session).cloned();
                self.local_assets.installed = Some(Installed { context, subtitles: bindings, art });
                self.changed();
            }
        }
    }

    fn apply_local_metadata(&mut self, context: &Context, data: &Discovered) -> Option<PosterKey> {
        let art = data.poster.as_ref().and_then(|path| {
            let root = self.current_local_root(&context.read.selection)?;
            if !self.capture_local_root(&context.read.selection)
                .is_some_and(|read| Arc::ptr_eq(&read.root, &context.read.root)) { return None; }
            crate::app::adapters::poster::register_local(root, context.read.root.clone(), path, 480, 720)
        });
        if context.selection_id.starts_with("favorite:") {
            self.favorite_preview = Some((context.selection_id.clone(), data.entry.clone()));
        } else if self.effective_profile() == Some(&context.profile)
            && self.catalog.entries.iter().any(|entry| entry.id == data.entry.id && entry.media == data.entry.media)
        {
            if self.catalog.merge(Catalog { hierarchy: None, entries: vec![data.entry.clone()] }).is_ok() {
                self.catalog_revision = self.catalog_revision.wrapping_add(1);
            }
        }
        if let Some(playing) = self.playing.as_mut().filter(|playing|
            playing.identity == context.playback && playing.entry.media == data.entry.media)
        {
            playing.entry.title = data.entry.title.clone();
            playing.entry.description = data.entry.description.clone();
            playing.entry.duration_ms = data.entry.duration_ms;
        }
        self.changed();
        art
    }

    pub(super) fn local_playing_art(&self, playing: &Playing) -> Option<OttPoster> {
        let installed = self.local_assets.installed.as_ref()?;
        let context = &installed.context;
        let selected = playing.local_selection.clone()
            .or_else(|| self.local_profile_selection(&playing.profile))?;
        if playing.profile.kind != ProviderKind::Local || playing.profile != context.profile
            || playing.entry.media != context.entry.media || playing.identity != context.playback
            || playing.selection_id != context.selection_id
            || selected.source_id != context.read.selection.source_id
            || selected.authority != context.read.selection.authority
            || selected.generation != context.read.selection.generation
            || !self.local_root_read_is_current(&context.read)
        { return None; }
        let root = self.current_local_root(&selected)?;
        let rebound = self.bind_local_selection(root).ok()?;
        let current = self.capture_local_root(&rebound)?;
        if !Arc::ptr_eq(&current.root, &context.read.root) { return None; }
        installed.art.map(|key| OttPoster::Local { key })
    }

    pub(super) fn local_entry_art(&self, entry: &CatalogEntry) -> Option<OttPoster> {
        let valid = |context: &Context| {
            context.entry.id == entry.id && context.entry.media == entry.media
                && self.local_root_read_is_current(&context.read)
                && if context.selection_id.starts_with("favorite:") {
                    self.metadata_target.as_ref().is_some_and(|(profile, id)|
                        profile == &context.profile.id && id == &context.selection_id)
                } else { self.effective_profile() == Some(&context.profile) }
        };
        self.local_assets.preview.as_ref().filter(|value| valid(&value.context))
            .and_then(|value| value.art)
            .or_else(|| self.local_assets.installed.as_ref().filter(|value| valid(&value.context))
                .and_then(|value| value.art))
            .map(|key| OttPoster::Local { key })
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_local_asset_tests.rs"]
mod tests;

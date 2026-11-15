//! One source-bound list order for desktop and remote next/previous actions.
use super::*;

/// A failed explicit destination is a navigation fact, never installed playback.
/// Private, process-local proof; no credentials or source locators are projected.
#[derive(Default)]
pub(super) struct State {
    anchor: Option<Anchor>, chain: Option<Chain>, admitted: Option<AdmittedList>,
    step_list: Option<Arc<List>>,
    failure_scope: Option<Arc<List>>,
}
struct List {
    ids: Vec<String>,
    entries: std::collections::HashMap<String, String>,
    favorites: Option<String>,
    profile: Option<ProviderProfile>,
    category: String,
    generation: u64,
    profile_epoch: u64,
    catalog_revision: u64,
}
struct AdmittedList {
    identity: plx_media::route::PlaybackIdentity,
    selection: String,
    list: Arc<List>,
}
pub(super) struct FailureContinuation { list: Option<Arc<List>>, anchor: Option<Anchor> }
impl List {
    fn fingerprint(entry: &CatalogEntry) -> Option<String> {
        Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(&(&entry.id, &entry.media, &entry.live)).ok()?))
    }
    fn favorite_fingerprint(runtime: &Runtime) -> Option<String> {
        Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(runtime.saved.favorites.entries()).ok()?))
    }
    fn capture(runtime: &Runtime, id: &str) -> Option<Arc<Self>> {
        let ids = commands::selected_ids(runtime);
        if !ids.iter().any(|current| current == id) { return None; }
        let (category, favorites) = runtime.view_selection();
        let entries: std::collections::HashMap<String, String> = if favorites { Default::default() } else {
            let wanted: std::collections::HashSet<_> = ids.iter().map(String::as_str).collect();
            runtime.catalog.entries.iter().filter(|entry| wanted.contains(entry.id.as_str()))
                .map(|entry| Some((entry.id.clone(), Self::fingerprint(entry)?))).collect::<Option<_>>()?
        };
        Some(Arc::new(Self { ids, entries, favorites: if favorites { Some(Self::favorite_fingerprint(runtime)?) } else { None },
            profile: (!favorites).then(|| runtime.effective_profile().cloned()).flatten(),
            category: category.into(), generation: runtime.generation, profile_epoch: runtime.profile_edit_epoch,
            catalog_revision: runtime.catalog_revision }))
    }
    fn current(&self, runtime: &Runtime) -> bool {
        if !runtime.loaded || runtime.persistence_uncertain || self.generation != runtime.generation
            || self.profile_epoch == 0 || self.profile_epoch != runtime.profile_edit_epoch { return false; }
        if let Some(favorites) = &self.favorites { return Self::favorite_fingerprint(runtime).as_ref() == Some(favorites); }
        if self.profile.as_ref().is_none_or(|profile| runtime.effective_profile() != Some(profile)) { return false; }
        // Background discovery may append new rows. It cannot replace an old
        // occurrence, remove it, or change the admitted order/visibility.
        let wanted: std::collections::HashSet<_> = self.ids.iter().map(String::as_str).collect();
        let current: Vec<_> = runtime.organization_rows(&self.category).into_iter()
            .map(|(_, entry)| entry.id.as_str()).filter(|id| wanted.contains(id)).collect();
        current.iter().copied().eq(self.ids.iter().map(String::as_str))
    }
    fn entry_current(&self, runtime: &Runtime, id: &str) -> bool {
        if self.favorites.is_some() { return self.ids.iter().any(|current| current == id); }
        let old = self.entries.get(id);
        let current = runtime.catalog.entries.iter().find(|entry| entry.id == id);
        old.is_some() && current.and_then(Self::fingerprint).as_ref() == old
    }
    fn refreshed(self: &Arc<Self>, runtime: &Runtime) -> Option<Arc<Self>> {
        if !self.current(runtime) { return None; }
        if self.favorites.is_some() || self.catalog_revision == runtime.catalog_revision { return Some(self.clone()); }
        let mut ids = Vec::new();
        let mut entries = std::collections::HashMap::new();
        for (_, entry) in runtime.organization_rows(&self.category) {
            let fingerprint = Self::fingerprint(entry)?;
            if self.entries.get(&entry.id).is_some_and(|old| old != &fingerprint) { return None; }
            if entries.insert(entry.id.clone(), fingerprint).is_some() { return None; }
            ids.push(entry.id.clone());
        }
        Some(Arc::new(Self { ids, entries, favorites: None, profile: self.profile.clone(), category: self.category.clone(),
            generation: self.generation, profile_epoch: self.profile_epoch, catalog_revision: runtime.catalog_revision }))
    }
}
struct Chain {
    origin: plx_screens::registry::OttPlayerSource,
    latest: plx_screens::registry::OttPlayerSource,
    frame: u32,
    owner: plx_machine::machine::InstanceId,
}
#[derive(Clone)]
struct Anchor {
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Option<Favorite>,
    outgoing: Option<plx_media::route::PlaybackIdentity>,
    outgoing_id: Option<String>,
    ticket: plx_media::route::PlayRequestTicket,
    generation: u64,
    catalog_revision: u64,
    navigation_epoch: u64,
    playback_epoch: u64,
    play_generation: u64,
    category: String,
    favorites: bool,
    failed: bool,
    list: Option<Arc<List>>,
}
impl State {
    pub(super) fn clear(&mut self) { self.anchor = None; self.chain = None; }
    pub(super) fn input_publication(&mut self) { self.chain = None; }
}

fn same_entry(a: Option<&CatalogEntry>, b: Option<&CatalogEntry>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.media == b.media && a.live == b.live,
        (None, None) => true,
        _ => false,
    }
}
impl Anchor {
    fn binding_current(&self, runtime: &Runtime) -> bool {
        let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(self.request.clone()) else { return false; };
        request == self.request && profile == self.profile && favorite == self.favorite
            && same_entry(entry.as_ref(), self.entry.as_ref())
            && self.list.as_ref().map_or_else(|| commands::selected_ids(runtime).iter().any(|id| id == &self.request.id),
                |list| list.ids.iter().any(|id| id == &self.request.id))
    }
    fn current(&self, runtime: &Runtime, app: &App) -> bool {
        let list_changed = if let Some(list) = &self.list { !list.current(runtime) || !list.entry_current(runtime, &self.request.id) }
                else { runtime.catalog_revision != self.catalog_revision
                    || runtime.view_selection() != (self.category.as_str(), self.favorites) };
        if runtime.generation != self.generation || list_changed
            || runtime.navigation_epoch != self.navigation_epoch || runtime.playback_epoch != self.playback_epoch
            || runtime.play_generation != self.play_generation
            || !runtime.loaded || runtime.persistence_uncertain || !playback_allowed(app)
            || !plx_media::route::play_request_is_current(self.ticket)
            || match (&self.outgoing, &self.outgoing_id) {
                (Some(outgoing), Some(id)) => !runtime.playing.as_ref().is_some_and(|playing|
                    playing.selection_id == *id && playing.identity.as_ref() == Some(outgoing)
                        && playing_matches(playing, app)),
                (None, None) => runtime.playing.is_some(),
                _ => true,
            } {
            return false;
        }
        self.binding_current(runtime)
    }
}

/// Bind the one accepted pending request before its resolver can start. Native
/// identity is optional on initial startup; the admission ticket still fences
/// replacement, and no pending token grants decoder/transport authority.
pub(super) fn bind_pending(runtime: &mut Runtime) {
    let inherited = runtime.explicit_step.step_list.take().or_else(|| runtime.explicit_step.anchor.as_ref()
        .filter(|anchor| anchor.play_generation == runtime.play_generation
            && runtime.play_target.as_ref() == Some(&anchor.request)).and_then(|anchor| anchor.list.clone()))
        .or_else(|| runtime.explicit_step.failure_scope.clone());
    runtime.explicit_step.clear();
    if transient::plex_remote::step_id(runtime, 0).is_some() { return; }
    let Some(request) = runtime.play_target.as_ref().filter(|request|
        request.archive.is_none()).cloned() else { return; };
    let Some(ticket) = runtime.play_ticket else { return; };
    let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(request) else { return; };
    let outgoing = runtime.playing.as_ref().and_then(|playing| playing.identity.clone());
    let outgoing_id = outgoing.as_ref().and_then(|_| runtime.playing.as_ref()
        .map(|playing| playing.selection_id.clone()));
    let (category, favorites) = runtime.view_selection();
    let list = inherited.filter(|list| list.current(runtime) && list.entry_current(runtime, &request.id))
        .or_else(|| List::capture(runtime, &request.id));
    runtime.explicit_step.anchor = Some(Anchor { request, profile, entry, favorite, outgoing, outgoing_id, ticket, list,
        generation: runtime.generation, catalog_revision: runtime.catalog_revision,
        navigation_epoch: runtime.navigation_epoch, playback_epoch: runtime.playback_epoch,
        play_generation: runtime.play_generation, category: category.into(), favorites, failed: false });
}

/// Only actual ordinary playback admission binds an immutable list to the new
/// installed identity. Browsing never recaptures or grants this receipt.
pub(super) fn admitted(runtime: &mut Runtime) {
    let Some(playing) = runtime.playing.as_ref() else { runtime.explicit_step.admitted = None; return; };
    let list = runtime.explicit_step.anchor.as_ref()
        .filter(|anchor| anchor.request.id == playing.selection_id)
        .and_then(|anchor| anchor.list.clone())
        .filter(|list| list.current(runtime) && list.entry_current(runtime, &playing.selection_id));
    runtime.explicit_step.admitted = playing.identity.clone().zip(list).map(|(identity, list)|
        AdmittedList { identity, selection: playing.selection_id.clone(), list });
}

fn admitted_list<'a>(runtime: &'a Runtime, playing: &Playing) -> Option<&'a Arc<List>> {
    let receipt = runtime.explicit_step.admitted.as_ref()?;
    (playing.identity.as_ref() == Some(&receipt.identity) && playing.selection_id == receipt.selection
        && receipt.list.current(runtime) && receipt.list.entry_current(runtime, &playing.selection_id))
        .then_some(&receipt.list)
}

fn continuation_list(runtime: &Runtime) -> Result<Option<Arc<List>>, ()> {
    if let Some(request) = runtime.play_target.as_ref() {
        if let Some(anchor) = runtime.explicit_step.anchor.as_ref() {
            if &anchor.request != request || anchor.play_generation != runtime.play_generation
                || anchor.navigation_epoch != runtime.navigation_epoch || anchor.playback_epoch != runtime.playback_epoch
                || !plx_media::route::play_request_is_current(anchor.ticket)
                || runtime.play_cancel.load(Ordering::Acquire)
            { return Err(()); }
            return anchor.list.as_ref().map(|list| list.refreshed(runtime).ok_or(())).transpose();
        }
        return Ok(None);
    }
    if let Some(list) = runtime.explicit_step.failure_scope.as_ref() {
        return list.refreshed(runtime).map(Some).ok_or(());
    }
    if let Some(anchor) = runtime.explicit_step.anchor.as_ref().filter(|anchor|
        anchor.failed && anchor.request.advance_on_failure && anchor.generation == runtime.generation
            && runtime.continuity.active_selection(runtime.generation).is_some()) {
        if anchor.navigation_epoch != runtime.navigation_epoch || anchor.play_generation != runtime.play_generation
            || anchor.playback_epoch != runtime.playback_epoch || !anchor.binding_current(runtime)
        { return Err(()); }
        let list = anchor.list.as_ref().map(|list| list.refreshed(runtime).ok_or(())).transpose()?;
        return match (list, runtime.continuity.active_selection(runtime.generation)) {
            (Some(list), Some(owner)) if list.ids.iter().any(|id| id == owner) => Ok(Some(list)),
            _ => Err(()),
        };
    }
    let Some(receipt) = runtime.explicit_step.admitted.as_ref() else { return Ok(None); };
    // load_catalog_profile establishes a new continuity generation. Its own
    // authorized startup/restore must not inherit the outgoing video's list.
    // Same-domain removals or revocations instead fail closed below.
    if receipt.list.generation != runtime.generation { return Ok(None); }
    if let Some(playing) = runtime.playing.as_ref() {
        if playing.identity.as_ref() != Some(&receipt.identity) || playing.selection_id != receipt.selection { return Err(()); }
    } else {
        let Some(owner) = runtime.continuity.active_selection(runtime.generation) else { return Ok(None); };
        let list = receipt.list.refreshed(runtime).ok_or(())?;
        return if list.ids.iter().any(|id| id == owner) { Ok(Some(list)) } else { Err(()) };
    }
    receipt.list.refreshed(runtime).map(Some).ok_or(())
}

/// The error handler captures this while the actual failed occurrence still
/// owns its native identity. A missing/stale owned queue is an explicit denial.
pub(super) fn capture_failure(runtime: &Runtime, app: &App) -> Option<FailureContinuation> {
    runtime.explicit_step.admitted.as_ref()?;
    let list = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app))
        .and_then(|playing| admitted_list(runtime, playing)).and_then(|list| list.refreshed(runtime));
    Some(FailureContinuation { list, anchor: None })
}
pub(super) fn capture_pending_failure(runtime: &Runtime, app: &App, generation: u64) -> Option<FailureContinuation> {
    let anchor = runtime.explicit_step.anchor.as_ref().filter(|anchor|
        anchor.play_generation == generation && anchor.request.advance_on_failure && anchor.list.is_some())?;
    let list = anchor.current(runtime, app).then(|| anchor.list.as_ref().and_then(|list| list.refreshed(runtime))).flatten();
    Some(FailureContinuation { list, anchor: Some(anchor.clone()) })
}
pub(super) fn with_failure_continuation(runtime: &mut Runtime, proof: Option<FailureContinuation>,
    continue_failure: impl FnOnce(&mut Runtime)) {
    let (list, anchor) = match proof {
        Some(FailureContinuation { list, anchor }) => match list.and_then(|list| list.refreshed(runtime)) {
            Some(list) => (Some(list), anchor),
            None => {
                // Waiting here would let a later unscoped tick borrow the
                // browsed list. Withdraw automatic continuation permanently.
                runtime.continuity.stop(runtime.generation);
                runtime.explicit_step.failure_scope = None;
                return;
            },
        },
        None => (None, None),
    };
    let owned = list.is_some();
    runtime.explicit_step.failure_scope = list;
    continue_failure(runtime);
    runtime.explicit_step.failure_scope = None;
    // A cooldown/discovery wait may retain only this exact failed candidate,
    // witnessed by the unchanged existing Continuity owner. It grants no input
    // or decoder capability and is removed by normal cancellation.
    if runtime.play_target.is_none() && runtime.explicit_step.anchor.is_none() {
        if let Some(mut anchor) = anchor.filter(|anchor|
            runtime.continuity.active_selection(runtime.generation) == Some(anchor.request.id.as_str())) {
            anchor.failed = true;
            runtime.explicit_step.anchor = Some(anchor);
        }
        if owned && runtime.explicit_step.anchor.is_none() && runtime.explicit_step.admitted.is_none()
            && runtime.continuity.active_selection(runtime.generation).is_some() {
            // A refusal before a typed pending candidate existed cannot mint a
            // new witness. Stop rather than later borrow the browser's domain.
            runtime.continuity.stop(runtime.generation);
        }
    }
}

/// Carry the already-owned automatic sequence into the existing request
/// admission. A synchronous refusal cannot leave intent for another caller.
pub(super) fn continue_at(runtime: &mut Runtime, id: &str, position_ms: u64) -> Result<(), &'static str> {
    let list = if kiosk_source::strict_folder_traversal(runtime) { None }
        else { continuation_list(runtime).map_err(|_| "The playing queue changed")? };
    runtime.explicit_step.step_list = list;
    let result = runtime.play_at(id, position_ms);
    runtime.explicit_step.step_list = None;
    result
}

pub(super) fn continuation_denied(runtime: &Runtime) -> bool {
    !kiosk_source::strict_folder_traversal(runtime) && continuation_list(runtime).is_err()
}

pub(super) fn continuation_ids(runtime: &Runtime) -> Option<Vec<String>> {
    match continuation_list(runtime) {
        Ok(Some(list)) => Some(list.ids.clone()),
        Ok(None) => None,
        Err(()) => Some(Vec::new()),
    }
}
pub(super) fn continuation_complete(runtime: &Runtime) -> Option<bool> {
    match continuation_list(runtime) {
        Ok(Some(list)) => Some(list.favorites.is_some() || runtime.catalog_complete),
        Ok(None) => None,
        Err(()) => Some(false),
    }
}

/// An accepted explicit category choice lands on its playing occurrence when
/// that exact source is visible there. Ordinary list browsing stays separate.
pub(super) fn focus_category_current(runtime: &mut Runtime, app: &App) {
    if runtime.interface_mode() != plx_machine::machine::UiMode::Classic
        || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic
        || runtime.play_target.is_some() || runtime.pending_play.is_some()
    { return; }
    let Some(playing) = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app)) else { return; };
    let matches = runtime.raw_view().rows.into_iter().filter_map(|row| {
        if !row.enabled { return None; }
        let OttAction::Play(id) = row.action else { return None; };
        let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(PlaybackRequest::ordinary(&id, 0)) else { return None; };
        (profile == playing.profile && request.local_selection == playing.local_selection
            && request.transient_owner == playing.transient_owner
            && match (entry.as_ref(), favorite.as_ref()) {
                (Some(entry), None) => same_entry(Some(entry), Some(&playing.entry)),
                (None, Some(favorite)) => selection::matches_entry(favorite, &playing.entry),
                _ => false,
            }).then_some(id)
    }).collect::<Vec<_>>();
    if matches.len() == 1 { browse_focus::record(runtime, &matches[0]); }
}

/// The snapshot is only an input capability; the app revalidates the private
/// source/list receipt and page owner when consuming each physical rocker press.
pub(super) fn pending_source(runtime: &Runtime) -> Option<plx_screens::registry::OttPlayerSource> {
    let anchor = runtime.explicit_step.anchor.as_ref()?;
    if anchor.failed || runtime.play_target.as_ref() != Some(&anchor.request)
        || runtime.play_cancel.load(Ordering::Acquire)
        || runtime.play_generation != anchor.play_generation
        || !plx_media::route::play_request_is_current(anchor.ticket) { return None; }
    Some(plx_screens::registry::OttPlayerSource::Pending {
        generation: anchor.generation, play_generation: anchor.play_generation, ticket: anchor.ticket })
}

pub(super) fn pending_id<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a str> {
    let anchor = runtime.explicit_step.anchor.as_ref()?;
    pending_source(runtime)?;
    if !anchor.current(runtime, app) || !runtime.pending_attachment_current()
        || !transient::target_current(runtime) || !runtime.play_work_allowed()
        || !commands::pending_catalog_current(runtime)
        || runtime.pending_play.as_ref().is_some_and(|invocation|
            invocation.deadline_ms <= runtime.now_ms() || !runtime.remote.invocation_is_current(invocation))
    { return None; }
    Some(anchor.request.id.as_str())
}

/// Only a batch already admitted under the unchanged immutable publication
/// may continue its own latest selection. A later frame or unrelated request
/// cannot replay the old source token into this lineage.
pub(super) fn chained(runtime: &Runtime, app: &App,
    request: &plx_screens::registry::OttPlayerRequest) -> bool {
    runtime.explicit_step.chain.as_ref().is_some_and(|chain|
        chain.frame == app.loop_t && chain.owner == request.owner && chain.origin == request.source
            && pending_source(runtime).as_ref() == Some(&chain.latest)
            && pending_id(runtime, app).is_some())
}
pub(super) fn note_chain(runtime: &mut Runtime, app: &App,
    request: &plx_screens::registry::OttPlayerRequest) {
    runtime.explicit_step.chain = pending_source(runtime).map(|latest| Chain {
        origin: request.source.clone(), latest, frame: app.loop_t, owner: request.owner });
}

/// A matching worker refusal can retain position in the list, but cannot start
/// anything. Expired, superseded, edited or successfully installed requests retire it.
pub(super) fn resolved(runtime: &mut Runtime, app: &App, generation: u64, refused: bool) {
    let current = runtime.explicit_step.anchor.as_ref().is_some_and(|anchor|
        anchor.play_generation == generation && anchor.current(runtime, app)
            && runtime.play_target.as_ref() == Some(&anchor.request));
    if refused && current && runtime.explicit_step.anchor.as_ref()
        .is_some_and(|anchor| !anchor.request.advance_on_failure && anchor.outgoing.is_some()) {
        runtime.explicit_step.anchor.as_mut().expect("checked explicit step").failed = true;
    } else { runtime.explicit_step.clear(); }
}

fn failed_id<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a str> {
    runtime.explicit_step.anchor.as_ref().filter(|anchor| anchor.failed
        && runtime.play_target.is_none() && anchor.current(runtime, app)).map(|anchor| anchor.request.id.as_str())
}


pub(super) fn target(runtime: &Runtime, app: &App, offset: i64) -> Result<String, &'static str> {
    if runtime.play_target.is_some() {
        let anchor = pending_id(runtime, app).ok_or("Pending selection changed before continuation")?;
        return if let Some(list) = runtime.explicit_step.anchor.as_ref().and_then(|anchor| anchor.list.as_ref()) {
            let list = list.refreshed(runtime).ok_or("The playing queue changed")?;
            commands::step_in_list(&list.ids, Some(anchor), offset)
        } else {
            commands::step_in_list(&commands::selected_ids(runtime), Some(anchor), offset)
        }.ok_or("No next video is available");
    }
    let playing = runtime
        .playing
        .as_ref()
        .filter(|playing| playing_matches(playing, app))
        .or_else(|| failed_playback::admissible(runtime, app))
        .ok_or("The playing source is no longer current")?;
    // An admitted Plex occurrence queue owns its order, including repeat mode.
    if let Some(current) = transient::plex_remote::step_id(runtime, 0) {
        if current.as_deref() != Some(playing.selection_id.as_str()) {
            return Err("The playing queue changed");
        }
        return transient::plex_remote::step_id(runtime, offset)
            .flatten()
            .ok_or("No next video is available");
    }
    if runtime.explicit_step.admitted.is_some() {
        let list = admitted_list(runtime, playing).and_then(|list| list.refreshed(runtime)).ok_or("The playing queue changed")?;
        let anchor = failed_id(runtime, app).unwrap_or(&playing.selection_id);
        let target = commands::step_in_list(&list.ids, Some(anchor), offset).ok_or("No next video is available")?;
        if !list.entry_current(runtime, &target) { return Err("The playing queue changed"); }
        return Ok(target);
    }
    let favorites = runtime.view_selection().1;
    if !favorites && runtime.effective_profile() != Some(&playing.profile) {
        return Err("The selected list belongs to another source");
    }
    let ids = commands::selected_ids(runtime);
    if !ids.iter().any(|id| id == &playing.selection_id) {
        return Err("The playing video is not in the selected list");
    }
    let anchor = failed_id(runtime, app).unwrap_or(&playing.selection_id);
    commands::step_in_list(&ids, Some(anchor), offset).ok_or("No next video is available")
}

/// Shared fullscreen next/previous intent. Native Plex keeps its installed
/// playback owner; its admitted Classic list supplies the ordinary Play target.
pub(super) fn perform(runtime: &mut Runtime, app: &mut App, offset: i64)
    -> Result<(), &'static str>
{
    if !matches!(offset, -1 | 1) { return Err("Invalid playback direction"); }
    if !runtime.loaded || runtime.persistence_uncertain || !playback_allowed(app) {
        return Err("Playback is unavailable");
    }
    input_policy::navigation(runtime)?;
    if runtime.playing.is_none() && runtime.failed_playback.is_none() {
        runtime.explicit_step.admitted = None;
    }
    // Refresh compact discovery membership only when the catalogue changes;
    // the same Arc/order is reused by every subsequent CH press.
    if let Some(list) = runtime.explicit_step.anchor.as_ref().and_then(|anchor| anchor.list.as_ref()) {
        let list = list.refreshed(runtime).ok_or("The playing queue changed")?;
        runtime.explicit_step.anchor.as_mut().unwrap().list = Some(list);
    }
    if let Some(receipt) = runtime.explicit_step.admitted.as_ref().filter(|_| runtime.play_target.is_none()) {
        let list = receipt.list.refreshed(runtime).ok_or("The playing queue changed")?;
        runtime.explicit_step.admitted.as_mut().unwrap().list = list;
    }
    if runtime.play_target.is_none() && runtime.playing.is_none() && runtime.failed_playback.is_none() {
        let source = player_browse::native_identity(runtime, app)
            .ok_or("The playing source is no longer current")?;
        let queued = player_browse::native_list_step(runtime, app, &source, offset)?;
        if queued {
            if let Some(page) = bridge::player_mut(&mut app.pages) {
                page.begin_channel_change(clock::now());
            }
        }
        return Ok(());
    }
    let id = target(runtime, app, offset)?;
    input_policy::ui(runtime, &OttAction::Play(id.clone()))?;
    parental_runtime::ui_gate(runtime, app, &OttAction::Play(id.clone()))?;
    let selected = runtime.play_target.as_ref().map(|request| request.id.as_str())
        .or_else(|| runtime.playing.as_ref().map(|playing| playing.selection_id.as_str()));
    if selected == Some(id.as_str()) { return Ok(()); }
    runtime.checkpoint(app, true);
    // Reversing a pending step back to the still-playing source cancels the
    // incoming request. It is not a no-op against the old source and must not
    // leave that superseded destination queued behind its worker.
    if runtime.play_target.is_some() && runtime.playing.as_ref().is_some_and(|playing|
        playing.selection_id == id && playing_matches(playing, app))
        && app.adapters.player.is_live()
        && matches!(plx_media::player::state(&app.player.session),
            plx_media::player::PlaybackState::Playing | plx_media::player::PlaybackState::Paused
                | plx_media::player::PlaybackState::Buffering | plx_media::player::PlaybackState::Seeking)
    {
        runtime.cancel_play_request();
        runtime.selected = Some(id.clone());
        browse_focus::record(runtime, &id);
        runtime.continuity.user_selected(runtime.generation, &id);
        runtime.status.clear();
        runtime.cancel_setup();
        runtime.changed();
        if let Some(page) = bridge::player_mut(&mut app.pages) { page.begin_channel_change(clock::now()); }
        return Ok(());
    }
    // A refused destination must not fall forward to unrelated media.
    let list = runtime.explicit_step.anchor.as_ref().and_then(|anchor| anchor.list.clone())
        .or_else(|| runtime.playing.as_ref().and_then(|playing| admitted_list(runtime, playing)).cloned());
    runtime.explicit_step.step_list = list;
    let result = runtime.play_with_policy(app, &id, false);
    runtime.explicit_step.step_list = None;
    result?;
    // An admitted rocker destination is explicit list intent, just like Play
    // from a row. Record it now so a late resolver cannot rewind newer focus.
    browse_focus::record(runtime, &id);
    parental_runtime::mark_ui_origin(runtime, app);
    if let Some(page) = bridge::player_mut(&mut app.pages) {
        page.begin_channel_change(clock::now());
    }
    runtime.cancel_setup();
    classic_play_handoff::begin(runtime, app);
    Ok(())
}

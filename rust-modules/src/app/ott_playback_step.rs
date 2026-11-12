//! One source-bound list order for desktop and remote next/previous actions.
use super::*;

/// A failed explicit destination is a navigation fact, never installed playback.
/// Private, process-local proof; no credentials or source locators are projected.
#[derive(Default)]
pub(super) struct State { anchor: Option<Anchor>, chain: Option<Chain> }
struct Chain {
    origin: plx_screens::registry::OttPlayerSource,
    latest: plx_screens::registry::OttPlayerSource,
    frame: u32,
    owner: plx_machine::machine::InstanceId,
}
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
    fn current(&self, runtime: &Runtime, app: &App) -> bool {
        if runtime.generation != self.generation || runtime.catalog_revision != self.catalog_revision
            || runtime.navigation_epoch != self.navigation_epoch || runtime.playback_epoch != self.playback_epoch
            || runtime.play_generation != self.play_generation
            || runtime.view_selection() != (self.category.as_str(), self.favorites)
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
        let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(self.request.clone()) else { return false; };
        request == self.request && profile == self.profile && favorite == self.favorite
            && same_entry(entry.as_ref(), self.entry.as_ref())
            && commands::selected_ids(runtime).iter().any(|id| id == &self.request.id)
    }
}

/// Bind the one accepted pending request before its resolver can start. Native
/// identity is optional on initial startup; the admission ticket still fences
/// replacement, and no pending token grants decoder/transport authority.
pub(super) fn bind_pending(runtime: &mut Runtime) {
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
    runtime.explicit_step.anchor = Some(Anchor { request, profile, entry, favorite, outgoing, outgoing_id, ticket,
        generation: runtime.generation, catalog_revision: runtime.catalog_revision,
        navigation_epoch: runtime.navigation_epoch, playback_epoch: runtime.playback_epoch,
        play_generation: runtime.play_generation, category: category.into(), favorites, failed: false });
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
        let ids = commands::selected_ids(runtime);
        return commands::step_in_list(&ids, Some(anchor), offset).ok_or("No next video is available");
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
    input_policy::navigation(runtime)?;
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
        runtime.continuity.user_selected(runtime.generation, &id);
        runtime.status.clear();
        runtime.cancel_setup();
        runtime.changed();
        if let Some(page) = bridge::player_mut(&mut app.pages) { page.begin_channel_change(clock::now()); }
        return Ok(());
    }
    // A refused destination must not fall forward to unrelated media.
    runtime.play_with_policy(app, &id, false)?;
    parental_runtime::mark_ui_origin(runtime, app);
    if let Some(page) = bridge::player_mut(&mut app.pages) {
        page.begin_channel_change(clock::now());
    }
    runtime.cancel_setup();
    if matches!(app.route(), AppArg::Ott) { classic_play_handoff::begin(runtime, app); }
    Ok(())
}

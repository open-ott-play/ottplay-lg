//! One source-bound list order for desktop and remote next/previous actions.
use super::*;

/// A failed explicit destination is a navigation fact, never installed playback.
/// Private, process-local proof; no credentials or source locators are projected.
#[derive(Default)]
pub(super) struct State { anchor: Option<Anchor> }
struct Anchor {
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Option<Favorite>,
    outgoing: plx_media::route::PlaybackIdentity,
    outgoing_id: String,
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
    pub(super) fn clear(&mut self) { self.anchor = None; }
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
            || !plx_media::route::playback_identity_matches(&app.player.session, &self.outgoing)
            || !runtime.playing.as_ref().is_some_and(|playing| playing.selection_id == self.outgoing_id
                && playing.identity.as_ref() == Some(&self.outgoing) && playing_matches(playing, app)) {
            return false;
        }
        let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(self.request.clone()) else { return false; };
        request == self.request && profile == self.profile && favorite == self.favorite
            && same_entry(entry.as_ref(), self.entry.as_ref())
            && commands::selected_ids(runtime).iter().any(|id| id == &self.request.id)
    }
}

/// Arm only after the ordinary explicit step has accepted and bound its request.
fn arm(runtime: &mut Runtime, app: &App) {
    runtime.explicit_step.clear();
    // An admitted native Plex occurrence queue remains its own ordering owner.
    if transient::plex_remote::step_id(runtime, 0).is_some() { return; }
    let Some(playing) = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app)) else { return; };
    let Some(outgoing) = playing.identity.clone() else { return; };
    let outgoing_id = playing.selection_id.clone();
    let Some(request) = runtime.play_target.as_ref().filter(|request| !request.advance_on_failure
        && request.archive.is_none()).cloned() else { return; };
    let Ok((request, profile, entry, favorite)) = runtime.prepare_play_request(request) else { return; };
    let (category, favorites) = runtime.view_selection();
    runtime.explicit_step.anchor = Some(Anchor { request, profile, entry, favorite, outgoing, outgoing_id,
        generation: runtime.generation, catalog_revision: runtime.catalog_revision,
        navigation_epoch: runtime.navigation_epoch, playback_epoch: runtime.playback_epoch,
        play_generation: runtime.play_generation, category: category.into(), favorites, failed: false });
}

/// A matching worker refusal can retain position in the list, but cannot start
/// anything. Expired, superseded, edited or successfully installed requests retire it.
pub(super) fn resolved(runtime: &mut Runtime, app: &App, generation: u64, refused: bool) {
    let current = runtime.explicit_step.anchor.as_ref().is_some_and(|anchor|
        anchor.play_generation == generation && anchor.current(runtime, app)
            && runtime.play_target.as_ref() == Some(&anchor.request));
    if refused && current {
        runtime.explicit_step.anchor.as_mut().expect("checked explicit step").failed = true;
    } else { runtime.explicit_step.clear(); }
}

fn failed_id<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a str> {
    runtime.explicit_step.anchor.as_ref().filter(|anchor| anchor.failed
        && runtime.play_target.is_none() && anchor.current(runtime, app)).map(|anchor| anchor.request.id.as_str())
}


pub(super) fn target(runtime: &Runtime, app: &App, offset: i64) -> Result<String, &'static str> {
    let playing = runtime
        .playing
        .as_ref()
        .filter(|playing| playing_matches(playing, app))
        .or_else(|| failed_playback::admissible(runtime, app))
        .ok_or("The playing source is no longer current")?;
    if runtime.play_target.is_some() {
        return Err("Another video is already opening");
    }
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
    if runtime.playing.is_none() && runtime.failed_playback.is_none() {
        let source = player_browse::native_identity(runtime, app)
            .ok_or("The playing source is no longer current")?;
        player_browse::native_list_step(runtime, app, &source, offset)?;
        if let Some(page) = bridge::player_mut(&mut app.pages) {
            page.begin_channel_change(clock::now());
        }
        return Ok(());
    }
    let id = target(runtime, app, offset)?;
    input_policy::ui(runtime, &OttAction::Play(id.clone()))?;
    runtime.checkpoint(app, true);
    // A refused destination must not fall forward to unrelated media.
    runtime.play_with_policy(app, &id, false)?;
    if let Some(page) = bridge::player_mut(&mut app.pages) {
        page.begin_channel_change(clock::now());
    }
    runtime.cancel_setup();
    arm(runtime, app);
    Ok(())
}

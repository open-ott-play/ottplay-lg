//! Actual-playing live facts. This owner keeps request proofs only; bounded EPG
//! facts remain in the existing metadata projection and selected-guide slot.
use super::*;
use plx_data::metadata::{NowPlaying, OttLivePlaying, OttNowPlaying};
use plx_data::ott::guide::Query;
use live_witness::LiveWitness;

#[derive(Default)]
pub(super) struct State {
    request: Option<LiveWitness>,
    published: Option<LiveWitness>,
    pending: Option<(u64, bool)>, // admitted generation, clear on current error
}

fn capture(playing: &Playing, generation: u64, now_s: i64) -> Option<LiveWitness> {
    LiveWitness::capture(playing, &playing.profile, &playing.entry,
        &playing.entry.id, generation, Query::CurrentNext { now: now_s })
}

/// Called at dispatch, not when a reply arrives. Changing browse focus retires
/// only the request proof; it does not replace the actually playing projection.
pub(super) fn arm(runtime: &mut Runtime) {
    runtime.live_hud.pending = None;
    runtime.live_hud.request = (|| {
        let playing = runtime.playing.as_ref()?;
        let (profile, selection) = runtime.metadata_next.as_ref()?;
        let entry = match selection {
            MetadataSelection::Entry(entry) => entry,
            MetadataSelection::Favorite(_, _) => &playing.entry,
            _ => return None,
        };
        let target = runtime.metadata_target.as_ref()?;
        // Attachment previews carry the parent profile ID. They are finite
        // sources today, and must not masquerade as live child-profile replies.
        if target.0 != profile.id { return None; }
        LiveWitness::capture(playing, profile, entry, &target.1,
            runtime.metadata_generation, runtime.metadata_query?)
    })();
}

/// Common receiver checks run first. Typed source provenance is additionally
/// required for successes; an exact current failure clears only this channel.
pub(super) fn complete(runtime: &mut Runtime, app: &App,
    event: &super::super::ott_metadata::Preview,
    candidate: Option<&selected_guide::Candidate>) {
    let Some(playing) = runtime.playing.as_ref().filter(|p| playing_matches(p, app)) else { return; };
    let Some(query) = runtime.metadata_query else { return; };
    if runtime.metadata_cancel.load(Ordering::Acquire)
        || !runtime.live_hud.request.as_ref().is_some_and(|proof|
            proof.accepts_completion(playing, event.generation, &event.profile_id,
                &event.entry_id, query, runtime.metadata_playback_identity.as_ref())) {
        return;
    }
    let clear = match candidate {
        Some(candidate) => match candidate.playing_facts(playing, runtime.metadata_generation,
            runtime.metadata_target.as_ref(), runtime.metadata_query) {
            Some(facts) => facts.is_none(),
            None => return,
        },
        None if event.result.is_err() => true,
        None => return,
    };
    runtime.live_hud.pending = Some((event.generation, clear));
}

/// Browsing another row borrows the existing worker. On returning to playback,
/// restore its actual source target without creating another worker/store.
pub(super) fn rearm_player(runtime: &mut Runtime, app: &App) {
    if !matches!(app.route(), AppArg::Player) || runtime.guide.is_some()
        || !runtime.source_work_allowed() || !transient::lifecycle_metadata_ready(runtime) {
        return;
    }
    let Some(playing) = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app)
        && playing.archive.is_none()) else { return; };
    if runtime.metadata_playback_identity == playing.identity
        && runtime.metadata_target.as_ref().is_some_and(|(profile, target)|
            profile == &playing.profile.id && (target == &playing.entry.id || target == &playing.selection_id)) {
        return;
    }
    if !live_witness::live_source(&playing.profile, &playing.entry) { return; }
    let playing = playing.clone();
    if runtime.playing_attachment.is_some() && playing.selection_id.starts_with("favorite:") {
        let _ = runtime.request_favorite_metadata(&playing.selection_id);
    } else if runtime.saved.profiles.profiles.iter().any(|profile|
        profile.enabled && profile == &playing.profile) {
        runtime.request_metadata(playing.profile, playing.entry);
    }
}

/// Returns true only for a live source. Finite media/archive keep their existing
/// collection, confirmed start and next-file projection.
pub(super) fn publish(runtime: &mut Runtime, app: &mut App, changed: bool) -> bool {
    let Some(playing) = runtime.playing.as_ref().filter(|p| playing_matches(p, app)) else {
        runtime.live_hud = State::default();
        return false;
    };
    let now_s = i64::try_from(runtime.wall_clock_ms / 1000).unwrap_or(i64::MAX);
    let same = runtime.live_hud.published.as_ref().is_some_and(|p| p.matches_playing(playing));
    let request_matches = runtime.live_hud.request.as_ref().is_some_and(|p| p.matches_playing(playing));
    if playing.archive.is_some() || playing.identity.is_none()
        || (!same && !request_matches && !live_witness::live_source(&playing.profile, &playing.entry)) {
        runtime.live_hud.published = None;
        runtime.live_hud.pending = None;
        return false;
    }
    runtime.programme_leaf = None;
    let start = playing.identity.as_ref().and_then(|id| app.player.programme.started_at(id));
    if start.is_none() {
        // The new source has not presented a frame; do not inherit an old HUD.
        runtime.live_hud.published = None;
        if let Some(mut now) = app.bridge.metadata_view().now_playing()
            .filter(|now| now.ott.is_some()).cloned() {
            now.ott = None;
            app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(now)));
            runtime.programme_metadata_generation = Some(app.bridge.metadata_mut().gen());
        }
        return true;
    }
    let current = app.bridge.metadata_view().now_playing();
    let pending = runtime.live_hud.pending.filter(|(generation, _)|
        *generation == runtime.metadata_generation && runtime.live_hud.request.as_ref()
            .is_some_and(|request| request.matches_playing(playing)));
    if !changed && same && pending.is_none() && current.is_some_and(|n|
        n.ott.as_ref().is_some_and(|o| o.live.is_some())) { return true; }
    let previous_guide = current.filter(|_| same).and_then(|n| n.ott.as_ref())
        .and_then(|o| o.live.as_ref()).and_then(|live| live.guide.clone());
    let guide = match pending {
        Some((_, true)) => None,
        Some((_, false)) => runtime.playing_row_guide(playing).unwrap_or(previous_guide),
        None => previous_guide,
    };
    let catalog = runtime.effective_profile().map(|profile| live_label::CatalogSource {
        profile, entries: &runtime.catalog.entries,
        transient_owner: runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner),
        source_current: runtime.loaded && !runtime.persistence_uncertain
            && !runtime.cancel.load(Ordering::Acquire),
    });
    let label = live_label::project(playing, catalog);
    let title = label.label.to_owned();
    let summary = guide.as_ref().and_then(|guide| guide.at(now_s).0)
        .map(|programme| programme.description.clone()).unwrap_or_default();
    let projection = OttNowPlaying {
        live: Some(OttLivePlaying { channel_title: title.clone(), ordinal: label.ordinal, guide }),
        archive: None,
        collection_label: title.clone(), confirmed_start_unix_ms: start,
        next: None, art: entry_poster(&playing.entry),
    };
    let unchanged = current.is_some_and(|old| old.title == title && old.summary == summary
        && old.dur_ms == 0 && old.ott.as_ref().is_some_and(|old| same_programme(old, &projection)));
    runtime.live_hud.published = capture(playing, runtime.metadata_generation, now_s);
    runtime.live_hud.pending = None;
    if !unchanged {
        let now = NowPlaying { ott: Some(projection), is_episode: false, is_real_episode: false,
            title, ep_title: String::new(), season: 0, index: 0, summary, year: 0, dur_ms: 0,
            rating: String::new(), thumb: String::new(), detail_rk: String::new() };
        app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(now)));
        runtime.programme_metadata_generation = Some(app.bridge.metadata_mut().gen());
    }
    true
}

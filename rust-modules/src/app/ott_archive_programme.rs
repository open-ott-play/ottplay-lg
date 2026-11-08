//! Projection of the accepted archive interval into the shared player footer.
//! This consumes the existing playback identity and confirmed-start observation;
//! it does not invent a live EPG successor or an independent playback clock.
use super::*;
use plx_data::metadata::{NowPlaying, OttNowPlaying};

fn same(left: &NowPlaying, right: &NowPlaying) -> bool {
    left.title == right.title
        && left.summary == right.summary
        && left.dur_ms == right.dur_ms
        && left.is_episode == right.is_episode
        && left.is_real_episode == right.is_real_episode
        && left.ep_title == right.ep_title
        && left.season == right.season
        && left.index == right.index
        && left.year == right.year
        && left.rating == right.rating
        && left.thumb == right.thumb
        && left.detail_rk == right.detail_rk
        && left.ott.as_ref().zip(right.ott.as_ref())
            .is_some_and(|(left, right)| same_programme(left, right))
}

pub(super) fn publish(runtime: &mut Runtime, app: &mut App) -> bool {
    let Some(playing) = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app)) else {
        return false;
    };
    let Some(archive) = playing.archive.as_ref() else { return false; };
    let programme = archive.programme();
    let interval = archive.interval();
    let observed_duration = plx_media::player::duration_ns().max(0) / 1_000_000;
    let duration = if observed_duration > 0 { observed_duration } else {
        interval.end.checked_sub(interval.start).and_then(|seconds| seconds.checked_mul(1000))
            .filter(|milliseconds| *milliseconds > 0).unwrap_or(0)
    };
    let projection = OttNowPlaying {
        live: None,
        archive: Some(plx_data::metadata::OttArchivePlaying {
            start_s: interval.start, end_s: interval.end,
        }),
        collection_label: playing.entry.title.clone(),
        confirmed_start_unix_ms: playing.identity.as_ref()
            .and_then(|identity| app.player.programme.started_at(identity)),
        // An archive is one explicit interval; its EOF does not autoplay live TV.
        next: None,
        art: entry_poster(&playing.entry),
    };
    let now = NowPlaying {
        ott: Some(projection),
        is_episode: false,
        is_real_episode: false,
        title: programme.as_ref().map(|row| row.title.clone())
            .filter(|title| !title.trim().is_empty()).unwrap_or_else(|| playing.entry.title.clone()),
        ep_title: String::new(),
        season: 0,
        index: 0,
        // Absence of retained archive metadata must not borrow the live programme.
        summary: programme.map(|row| row.description).unwrap_or_default(),
        year: 0,
        dur_ms: duration,
        rating: String::new(),
        thumb: String::new(),
        detail_rk: String::new(),
    };
    runtime.programme_leaf = None;
    if !app.bridge.metadata_view().now_playing().is_some_and(|old| same(old, &now)) {
        app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(now)));
    }
    runtime.programme_metadata_generation = Some(app.bridge.metadata_mut().gen());
    true
}

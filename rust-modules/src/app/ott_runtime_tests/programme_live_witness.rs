//! Uses programme.rs's installed native-route fixture. No provider work starts.
use super::*;
use crate::app::ott_runtime::live_witness::LiveWitness;
use plx_data::ott::guide::{ChannelMetadata, Query};

fn live_fixture() -> App {
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile.kind = ProviderKind::M3u;
    playing.profile.source = "https://media.invalid/live.m3u".into();
    playing.entry.duration_ms = None;
    playing.entry.live = Some(ChannelMetadata::default());
    app
}

fn query() -> Query {
    Query::CurrentNext { now: 100 }
}

fn capture(playing: &Playing, target: &str) -> LiveWitness {
    LiveWitness::capture(
        playing,
        &playing.profile,
        &playing.entry,
        target,
        17,
        query(),
    )
    .unwrap()
}

#[test]
fn actual_live_witness_accepts_only_its_exact_completion() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let app = live_fixture();
    let playing = app.ott.as_ref().unwrap().playing.as_ref().unwrap();
    let witness = capture(playing, &playing.entry.id);
    let accepts = |generation, profile: &str, target: &str, query, identity| {
        witness.accepts_completion(playing, generation, profile, target, query, identity)
    };
    assert!(accepts(
        17,
        &playing.profile.id,
        &playing.entry.id,
        query(),
        playing.identity.as_ref()
    ));
    assert!(!accepts(
        18,
        &playing.profile.id,
        &playing.entry.id,
        query(),
        playing.identity.as_ref()
    ));
    assert!(!accepts(
        17,
        "other-profile",
        &playing.entry.id,
        query(),
        playing.identity.as_ref()
    ));
    assert!(!accepts(
        17,
        &playing.profile.id,
        "other-target",
        query(),
        playing.identity.as_ref()
    ));
    assert!(!accepts(
        17,
        &playing.profile.id,
        &playing.entry.id,
        Query::CurrentNext { now: 101 },
        playing.identity.as_ref()
    ));
    assert!(!accepts(
        17,
        &playing.profile.id,
        &playing.entry.id,
        query(),
        None
    ));
}

#[test]
fn ordinary_favorite_allows_resolved_or_outer_target_only_with_exact_source() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_fixture();
    let playing = app.ott.as_mut().unwrap().playing.as_mut().unwrap();
    playing.selection_id = "favorite:fixture".into();
    assert!(capture(playing, &playing.entry.id).matches_playing(playing));
    assert!(capture(playing, &playing.selection_id).matches_playing(playing));
    let mut other = playing.entry.clone();
    other.media = MediaLocator::Http {
        url: "https://media.invalid/other.m3u8".into(),
    };
    assert!(LiveWitness::capture(
        playing,
        &playing.profile,
        &other,
        &playing.selection_id,
        17,
        query()
    )
    .is_none());
    assert!(LiveWitness::capture(
        playing,
        &playing.profile,
        &playing.entry,
        "other-target",
        17,
        query()
    )
    .is_none());
}

#[test]
fn witness_survives_browsing_but_never_a_new_same_url_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_ref().unwrap();
    let witness = capture(playing, &playing.entry.id);
    runtime.metadata_generation += 1;
    runtime.generation += 1;
    runtime.selected = Some("different-channel".into());
    assert!(witness.matches_playing(runtime.playing.as_ref().unwrap()));
    install(&mut app);
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity =
        route::playback_identity(&app.player.session);
    assert!(!witness.matches_playing(app.ott.as_ref().unwrap().playing.as_ref().unwrap()));
}

#[test]
fn witness_ignores_description_enrichment_but_rejects_changed_source() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_fixture();
    let playing = app.ott.as_mut().unwrap().playing.as_mut().unwrap();
    let witness = capture(playing, &playing.entry.id);
    playing.entry.description = "New complete description".into();
    playing.entry.poster.clear();
    assert!(witness.matches_playing(playing));
    playing.profile.source = "https://media.invalid/replaced.m3u".into();
    assert!(!witness.matches_playing(playing));
}

#[test]
fn finite_untagged_and_day_requests_cannot_capture_a_live_witness() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_fixture();
    let playing = app.ott.as_mut().unwrap().playing.as_mut().unwrap();
    assert!(LiveWitness::capture(
        playing,
        &playing.profile,
        &playing.entry,
        &playing.entry.id,
        17,
        Query::Day {
            start: 0,
            end: 86400
        }
    )
    .is_none());
    playing.entry.duration_ms = Some(1000);
    assert!(LiveWitness::capture(
        playing,
        &playing.profile,
        &playing.entry,
        &playing.entry.id,
        17,
        query()
    )
    .is_none());
    playing.entry.duration_ms = None;
    playing.identity = None;
    assert!(LiveWitness::capture(
        playing,
        &playing.profile,
        &playing.entry,
        &playing.entry.id,
        17,
        query()
    )
    .is_none());
}

#[test]
fn starting_browsed_channel_rearms_metadata_for_the_playing_identity() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_ref().unwrap().clone();
    runtime.metadata_target = Some((playing.profile.id.clone(), playing.entry.id.clone()));
    runtime.metadata_playback_identity = None;
    let before = runtime.metadata_generation;
    runtime.request_metadata(playing.profile.clone(), playing.entry.clone());
    assert_ne!(runtime.metadata_generation, before);
    assert_eq!(runtime.metadata_playback_identity, playing.identity);
    assert!(runtime.metadata_next.is_some());
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
}

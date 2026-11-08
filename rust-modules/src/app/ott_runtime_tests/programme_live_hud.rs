//! Actual installed-route and metadata mailbox coverage for the live HUD.
//! Native presentation is injected; no provider worker, network or decoder runs.
use super::*;
use crate::app::ott_metadata::{Event, Guide};
use plx_base::task::time::ClockSample;
use plx_data::metadata::OttLivePlaying;
use plx_data::ott::{epg::Programme, guide::{ChannelMetadata, Context, Coverage, Request, ResultSet}};

fn live_app() -> (App, ClockSample) {
    let mut app = fixture();
    let sample = ClockSample::capture_main(app.adapters.player.mt()).expect("fixture clock");
    let runtime = app.ott.as_mut().unwrap();
    let mut profile = runtime.playing.as_ref().unwrap().profile.clone();
    profile.kind = ProviderKind::M3u;
    profile.source = "https://media.invalid/live.m3u".into();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = profile.kind;
    for entry in &mut runtime.catalog.entries {
        entry.duration_ms = None;
        entry.live = Some(ChannelMetadata::default());
    }
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile;
    playing.entry = runtime.catalog.entries[0].clone();
    runtime.page = "channels";
    runtime.selected = Some("c".into());
    runtime.clock_sample = Some(sample);
    runtime.wall_clock_ms = sample.unix_ms();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.cancel.store(false, Ordering::Release);
    runtime.metadata_target = None;
    runtime.metadata_query = None;
    runtime.metadata_playback_identity = None;
    (app, sample)
}

fn presented(app: &mut App, sample: ClockSample) {
    player::SHARED.start_queued.store(false, Ordering::Release);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    super::super::super::observe_programme(app, sample.unix_ms());
}

fn arm_playing(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_ref().unwrap().clone();
    runtime.request_metadata(playing.profile, playing.entry);
    assert!(runtime.metadata_next.is_some());
    assert_eq!(runtime.metadata_playback_identity,
        runtime.playing.as_ref().unwrap().identity);
}

fn refresh_playing(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    let previous = runtime.metadata_generation;
    runtime.metadata_refresh_ms = 1;
    runtime.refresh_metadata();
    assert_ne!(runtime.metadata_generation, previous);
}

fn reply(app: &App, title: &str) -> Guide {
    let runtime = app.ott.as_ref().unwrap();
    let (profile, selection) = runtime.metadata_next.as_ref().expect("captured metadata request");
    let MetadataSelection::Entry(entry) = selection else { panic!("entry metadata request") };
    let target = runtime.metadata_target.as_ref().unwrap();
    let query = runtime.metadata_query.unwrap();
    let now = (runtime.wall_clock_ms / 1000) as i64;
    let context = Context { generation: runtime.metadata_generation,
        profile_id: profile.id.clone(), entry_id: entry.id.clone() };
    Guide { row_guide: None, source_profile: profile.clone(), generation: runtime.metadata_generation,
        profile_id: target.0.clone(), entry_id: target.1.clone(), entry: entry.clone(),
        request: Request { context: context.clone(), query },
        result: Ok(ResultSet { context, query, coverage: Coverage::RequestedWindow,
            programmes: vec![
                Programme { channel: "fixture".into(), start: now - 60, end: now + 60,
                    title: title.into(), description: "First sentence. Second sentence.\n\nFINAL_PARAGRAPH".into() },
                Programme { channel: "fixture".into(), start: now + 60, end: now + 180,
                    title: "Next programme".into(), description: "Full next programme description".into() },
            ] }) }
}

fn deliver(app: &mut App, sample: ClockSample, guide: Guide) {
    app.ott.as_ref().unwrap().metadata_tx.send(Event::Guide(guide)).unwrap();
    super::super::super::tick(app, sample);
    assert!(!app.ott.as_ref().unwrap().metadata_done.load(Ordering::Acquire),
        "the fixture never frees the held metadata producer slot");
}

fn live(app: &App) -> Option<OttLivePlaying> {
    app.bridge.metadata_view().now_playing().and_then(|now| now.ott.as_ref())
        .and_then(|ott| ott.live.clone())
}

fn ready_live() -> (App, ClockSample) {
    let (mut app, sample) = live_app();
    presented(&mut app, sample);
    arm_playing(&mut app);
    let guide = reply(&app, "Current programme");
    deliver(&mut app, sample, guide);
    (app, sample)
}

#[test]
fn live_hud_publishes_actual_channel_raw_ordinal_and_full_broadcast_facts() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (app, sample) = ready_live();
    let value = live(&app).expect("presented live projection");
    assert_eq!(value.channel_title, "Current file");
    assert_eq!(value.ordinal, Some(1));
    let guide = value.guide.unwrap();
    let now = (sample.unix_ms() / 1000) as i64;
    let (current, next) = guide.at(now);
    let current = current.unwrap();
    assert_eq!(current.title, "Current programme");
    assert_eq!(current.description, "First sentence. Second sentence.\n\nFINAL_PARAGRAPH");
    assert_eq!((current.start_s, current.end_s), (now - 60, now + 60));
    assert_eq!(next.unwrap().title, "Next programme");
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}

#[test]
fn live_hud_waits_for_a_presented_frame_and_keeps_early_guide_until_then() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = live_app();
    arm_playing(&mut app);
    let guide = reply(&app, "Early programme");
    deliver(&mut app, sample, guide);
    assert!(live(&app).is_none());
    player::SHARED.seen_frame.store(true, Ordering::Release);
    player::SHARED.start_queued.store(true, Ordering::Release);
    super::super::super::observe_programme(&mut app, sample.unix_ms());
    publish(&mut app);
    assert!(live(&app).is_none(), "old frame latch cannot confirm a queued source");
    presented(&mut app, sample);
    publish(&mut app);
    assert_eq!(live(&app).unwrap().guide.unwrap().current.unwrap().title, "Early programme");
}

#[test]
fn live_hud_browsing_and_metadata_for_another_row_preserve_playing_a() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = ready_live();
    let before = live(&app).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.playing.as_ref().unwrap().profile.clone();
    let browsed = runtime.catalog.entries[1].clone();
    runtime.selected = Some(browsed.id.clone());
    runtime.request_metadata(profile, browsed);
    let guide = reply(&app, "Browsed programme B");
    deliver(&mut app, sample, guide);
    assert_eq!(live(&app).as_ref(), Some(&before));
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().entry.id, "c");
}

#[test]
fn live_hud_current_error_clears_guide_but_stale_error_cannot_clear_new_facts() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = ready_live();
    let mut stale_error = reply(&app, "unused");
    stale_error.result = Err("stale failure");
    refresh_playing(&mut app);
    let fresh = reply(&app, "Fresh programme");
    deliver(&mut app, sample, fresh);
    let before = live(&app).unwrap();
    deliver(&mut app, sample, stale_error);
    assert_eq!(live(&app).as_ref(), Some(&before));
    refresh_playing(&mut app);
    let mut current_error = reply(&app, "unused");
    current_error.result = Err("current failure");
    deliver(&mut app, sample, current_error);
    let value = live(&app).unwrap();
    assert!(value.guide.is_none());
    assert_eq!(value.channel_title, "Current file");
    assert_eq!(value.ordinal, Some(1));
}

#[test]
fn live_hud_same_url_new_lifetime_retires_old_guide_and_rejects_old_completion() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = ready_live();
    let mut old = reply(&app, "Old lifetime");
    old.result = Err("old lifetime failure");
    let old_identity = route::playback_identity(&app.player.session);
    install(&mut app);
    assert_ne!(route::playback_identity(&app.player.session), old_identity);
    publish(&mut app);
    assert!(live(&app).is_none(), "old Playing cannot label another installed identity");
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = route::playback_identity(&app.player.session);
    presented(&mut app, sample);
    publish(&mut app);
    assert!(live(&app).unwrap().guide.is_none(), "new same-URL lifetime cannot reuse old EPG");
    arm_playing(&mut app);
    let fresh = reply(&app, "New lifetime");
    deliver(&mut app, sample, fresh);
    let before = live(&app).unwrap();
    deliver(&mut app, sample, old);
    assert_eq!(live(&app).as_ref(), Some(&before));
}

#[test]
fn live_hud_foreign_same_id_source_cannot_lend_its_title_or_number() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, _sample) = ready_live();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles[0].source = "https://media.invalid/other.m3u".into();
    runtime.catalog.entries[0].title = "Foreign source title".into();
    publish(&mut app);
    let value = live(&app).unwrap();
    assert_eq!(value.channel_title, "Current file");
    assert_eq!(value.ordinal, None);
    assert_eq!(value.guide.unwrap().current.unwrap().title, "Current programme");
}

#[test]
fn live_hud_favorite_uses_only_proven_underlying_catalog_number() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, _sample) = ready_live();
    let runtime = app.ott.as_mut().unwrap();
    runtime.playing.as_mut().unwrap().selection_id = "favorite:opaque".into();
    runtime.saved.favorites_selected = true;
    runtime.catalog.entries.swap(0, 2);
    publish(&mut app);
    assert_eq!(live(&app).unwrap().ordinal, Some(3));
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].id = "different-profile".into();
    publish(&mut app);
    assert_eq!(live(&app).unwrap().ordinal, None);
    assert_eq!(live(&app).unwrap().channel_title, "Current file");
}

#[test]
fn live_hud_rejects_typed_reply_with_same_ids_but_wrong_source_profile() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = ready_live();
    refresh_playing(&mut app);
    let before = live(&app).unwrap();
    let mut foreign = reply(&app, "Foreign programme");
    foreign.source_profile.source = "https://media.invalid/other-source.m3u".into();
    deliver(&mut app, sample, foreign);
    assert_eq!(live(&app).as_ref(), Some(&before));
}

#[test]
fn live_hud_return_to_player_rearms_playing_source_once_without_restarting_media() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, sample) = ready_live();
    let before_live = live(&app).unwrap();
    let before_identity = route::playback_identity(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.playing.as_ref().unwrap().profile.clone();
    let browsed = runtime.catalog.entries[1].clone();
    runtime.selected = Some(browsed.id.clone());
    runtime.request_metadata(profile.clone(), browsed);
    let browsed_generation = runtime.metadata_generation;
    mount_player(&mut app);
    assert!(matches!(app.route(), AppArg::Player));
    super::super::super::tick(&mut app, sample);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.metadata_target, Some((profile.id, "c".into())));
    assert_ne!(runtime.metadata_generation, browsed_generation);
    assert_eq!(runtime.metadata_playback_identity, before_identity);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    let rearmed_generation = runtime.metadata_generation;
    assert_eq!(live(&app).as_ref(), Some(&before_live));
    super::super::super::tick(&mut app, sample);
    assert_eq!(app.ott.as_ref().unwrap().metadata_generation, rearmed_generation,
        "an already correct player target must not restart metadata every frame");
    assert_eq!(route::playback_identity(&app.player.session), before_identity);
}

#[path = "programme_info_cycle.rs"]
mod info_cycle;

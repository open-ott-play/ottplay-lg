//! Compact rows share one admitted catalogue result; selected full guides stay separate.
use super::*;
use plx_data::ott::guide::rows::{CoverageReport, CoverageState, CurrentProgramme, Outcome, Snapshot};
use std::collections::{BTreeSet, HashMap};

#[path = "row_guide_coverage.rs"]
mod coverage;

fn batch_fixture() -> (App, ClockSample, Guide) {
    let (mut app, sample, mut reply) = fixture();
    let now = (sample.unix_ms() / 1000) as i64;
    let runtime = app.ott.as_mut().unwrap();
    runtime.wall_clock_ms = sample.unix_ms();
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    for id in ["second", "third"] {
        let mut entry = reply.entry.clone();
        entry.id = id.into();
        entry.title = id.into();
        entry.provider_id = Some(id.into());
        runtime.catalog.entries.push(entry);
    }
    let request = runtime.request_row_guide(&reply.source_profile, &reply.entry, reply.request.query)
        .expect("complete ordinary M3U catalogue");
    let programmes = ["guide-channel", "second", "third"].into_iter().map(|id| (
        id.into(), CurrentProgramme {
            title: format!("Programme for {id}"), start_s: now - 10,
            end_s: now + if id == "second" { 5 } else { 60 },
        },
    )).collect::<HashMap<_, _>>();
    reply.row_guide = Some(super::super::super::row_guide::Reply {
        request, outcome: Outcome {
            snapshot: Some(Snapshot { now_s: now, next_transition_s: Some(now + 5), programmes }),
            report: CoverageReport {
                state: CoverageState::Complete, reasons: BTreeSet::new(),
                requested: 3, matched: 3, current: 3,
                configured_sources: 1, attempted_sources: 1, loaded_sources: 1,
            },
        },
    });
    (app, sample, reply)
}

#[test]
fn compact_batch_enriches_other_rows_without_expanding_full_selected_guide() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = batch_fixture();
    let raw = app.ott.as_ref().unwrap().catalog.entries[1..].to_vec();
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(&runtime.catalog.entries[1..], raw.as_slice());
    let view = runtime.raw_view();
    for id in ["guide-channel", "second", "third"] {
        let row = view.rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row.current_programme.as_ref().unwrap().title, format!("Programme for {id}"));
        assert_eq!(row.guide.is_some(), id == "guide-channel");
    }
    assert!(runtime.metadata_refresh_ms <= runtime.now_ms().saturating_add(5_000));
}

#[test]
fn compact_batch_survives_selection_cancel_and_selected_metadata_enrichment() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = batch_fixture();
    let mut next = copy_guide(&reply);
    next.row_guide = None;
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries[1].clone();
    let old_cancel = runtime.metadata_cancel.clone();
    runtime.request_metadata(profile.clone(), entry.clone());
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(runtime.request_row_guide(&profile, &entry, runtime.metadata_query.unwrap()).is_none());
    assert!(runtime.current_row_programme(&runtime.catalog.entries[2]).is_some());
    next.generation = runtime.metadata_generation;
    next.entry_id = entry.id.clone();
    next.entry = entry;
    next.request.context.generation = next.generation;
    next.request.context.entry_id = next.entry_id.clone();
    next.request.query = runtime.metadata_query.unwrap();
    let result = next.result.as_mut().unwrap();
    result.context = next.request.context.clone();
    result.query = next.request.query;
    runtime.metadata_next = None;
    deliver(&mut app, sample, next);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.current_row_programme(&runtime.catalog.entries[2]).is_some());
    assert!(runtime.raw_view().rows.iter().find(|row| row.id == "second").unwrap().guide.is_some());
}

#[test]
fn compact_batch_rejects_stale_queued_reply_ownership() {
    let _serial = plx_base::testlock::serial();
    for change in 0..10 {
        let (mut app, sample, reply) = batch_fixture();
        let runtime = app.ott.as_mut().unwrap();
        match change {
            0 => runtime.generation = runtime.generation.wrapping_add(1),
            1 => runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1),
            2 => runtime.catalog.entries[1].live.as_mut().unwrap().guide_shift_seconds = 60,
            3 => runtime.saved.profiles.profiles[0].source = "https://other.invalid/list.m3u".into(),
            4 => runtime.metadata_cancel.store(true, Ordering::Release),
            5 => runtime.persistence_uncertain = true,
            6 => runtime.catalog_complete = false,
            7 => runtime.metadata_target.as_mut().unwrap().1 = "second".into(),
            8 => runtime.metadata_query = Some(Query::CurrentNext { now: 0 }),
            _ => runtime.metadata_generation = runtime.metadata_generation.wrapping_add(1),
        }
        deliver(&mut app, sample, reply);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog.entries.iter().all(|entry| runtime.current_row_programme(entry).is_none()), "change {change}");
    }
}

#[test]
fn compact_batch_lookup_refuses_changed_source_selector_or_catalogue() {
    let _serial = plx_base::testlock::serial();
    for change in 0..10 {
        let (mut app, sample, reply) = batch_fixture();
        deliver(&mut app, sample, reply);
        let runtime = app.ott.as_mut().unwrap();
        match change {
            0 => runtime.catalog.entries[1].duration_ms = Some(1_000),
            1 => runtime.catalog.entries[1].media = MediaLocator::Http { url: "https://other.invalid/live".into() },
            2 => runtime.catalog.entries[1].provider_id = Some("replacement".into()),
            3 => runtime.catalog.entries[1].title = "replacement".into(),
            4 => runtime.catalog.entries[1].live.as_mut().unwrap().tvg_name = "replacement".into(),
            5 => runtime.catalog.entries[1].live.as_mut().unwrap().guide_shift_seconds = 60,
            6 => runtime.generation = runtime.generation.wrapping_add(2), // Source A -> B -> A.
            7 => runtime.catalog_revision = runtime.catalog_revision.wrapping_add(2),
            8 => runtime.persistence_uncertain = true,
            _ => runtime.page = "favorites",
        }
        assert!(runtime.current_row_programme(&runtime.catalog.entries[1]).is_none(), "change {change}");
    }
}

#[test]
fn compact_batch_expires_at_earliest_transition_and_clock_rollback() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = batch_fixture();
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries[0].clone();
    runtime.wall_clock_ms = (sample.unix_ms() / 1000 + 5) * 1000;
    assert!(runtime.current_row_programme(&runtime.catalog.entries[1]).is_none());
    let query = Query::CurrentNext { now: (runtime.wall_clock_ms / 1000) as i64 };
    assert!(runtime.request_row_guide(&profile, &entry, query).is_some());
    runtime.wall_clock_ms = (sample.unix_ms() / 1000 - 1) * 1000;
    assert!(runtime.current_row_programme(&entry).is_none());
    assert!(runtime.request_row_guide(&profile, &entry, Query::CurrentNext { now: -1 }).is_none());
    let query = Query::CurrentNext { now: (runtime.wall_clock_ms / 1000) as i64 };
    assert!(runtime.request_row_guide(&profile, &entry, query).is_some());
}

#[test]
fn compact_batch_capture_is_only_complete_ordinary_current_next_catalogue() {
    let _serial = plx_base::testlock::serial();
    for change in 0..5 {
        let (mut app, _, reply) = batch_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let mut entry = reply.entry.clone();
        let mut query = reply.request.query;
        match change {
            0 => runtime.page = "favorites",
            1 => runtime.catalog_complete = false,
            2 => entry.duration_ms = Some(1_000),
            3 => query = Query::Day { start: 0, end: 86_400 },
            _ => runtime.catalog.entries.push(entry.clone()),
        }
        assert!(runtime.request_row_guide(&reply.source_profile, &entry, query).is_none(), "change {change}");
    }
}

#[test]
fn compact_batch_missing_at_catalogue_completion_reuses_idle_metadata_slot() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = batch_fixture();
    reply.row_guide = None;
    app.ott.as_mut().unwrap().catalog_complete = false;
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_complete = true;
    let old_deadline = runtime.metadata_refresh_ms;
    runtime.tick_row_guide();
    assert_eq!(runtime.metadata_refresh_ms, old_deadline, "busy worker is not cancelled for a batch");
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.tick_row_guide();
    assert_eq!(runtime.metadata_refresh_ms, runtime.now_ms().max(1));
    // Hold the real producer slot again: refresh queues through the existing owner.
    runtime.metadata_done.store(false, Ordering::Release);
    let generation = runtime.metadata_generation;
    runtime.refresh_metadata();
    assert_eq!(runtime.metadata_generation, generation.wrapping_add(1));
    assert!(matches!(runtime.metadata_next, Some((_, MetadataSelection::Entry(_)))));
}

#[test]
fn compact_batch_expiry_invalidates_view_once_while_refresh_worker_is_busy() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = batch_fixture();
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    runtime.wall_clock_ms = (sample.unix_ms() / 1000 + 5) * 1000;
    let revision = runtime.revision;
    let metadata_generation = runtime.metadata_generation;
    runtime.tick_row_guide();
    assert_eq!(runtime.revision, revision.wrapping_add(1));
    assert!(runtime.raw_view().rows.iter().all(|row| row.current_programme.is_none()));
    runtime.tick_row_guide();
    assert_eq!(runtime.revision, revision.wrapping_add(1));
    assert_eq!(runtime.metadata_generation, metadata_generation);
    assert!(runtime.metadata_next.is_none());
}

#[test]
fn compact_batch_refused_sideband_backs_off_despite_selected_enrichment() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = batch_fixture();
    let sideband = reply.row_guide.as_mut().unwrap();
    app.ott.as_mut().unwrap().note_row_guide_attempt(&sideband.request);
    sideband.outcome.snapshot.as_mut().unwrap().now_s -= 1; // Self-consistent selected guide, wrong batch time.
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.metadata_refresh_ms = 0;
    runtime.tick_row_guide();
    assert_eq!(runtime.metadata_refresh_ms, 0, "refused batch must not retry on every frame");
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries[0].clone();
    assert!(runtime.request_row_guide(&profile, &entry, runtime.metadata_query.unwrap()).is_none());
}

#[test]
fn compact_batch_refuses_missing_or_late_transition_without_losing_selected_guide() {
    let _serial = plx_base::testlock::serial();
    for late in [false, true] {
        let (mut app, sample, mut reply) = batch_fixture();
        let snapshot = reply.row_guide.as_mut().unwrap().outcome.snapshot.as_mut().unwrap();
        snapshot.next_transition_s = late.then_some(snapshot.now_s + 60);
        deliver(&mut app, sample, reply);
        let view = app.ott.as_ref().unwrap().raw_view();
        assert!(view.rows.iter().all(|row| row.current_programme.is_none()));
        assert!(view.rows.iter().find(|row| row.id == "guide-channel").unwrap().guide.is_some());
    }
}

//! Host state tests: producers stay held, with no storage, provider or decoder IO.
use super::*;
use crate::app::ott_metadata::{Guide, Preview};
use plx_data::ott::guide::{Context, Coverage, Request};

fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    let profile = ProviderProfile {
        id: "fixture-guide".into(),
        name: "Home".into(),
        kind: ProviderKind::M3u,
        source: "https://fixture.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    runtime.loaded = true;
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles.push(profile);
    runtime.wall_clock_ms = 1700000000000;
    runtime.page = "channels";
    runtime.resume_source_work();
    runtime.rearm_metadata_work();
    assert!(runtime.metadata_work_allowed());
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.catalog.entries.push(CatalogEntry { protection: None,
        synopsis: None,
        id: "channel-one".into(),
        title: "Live channel".into(),
        category: String::new(),
        description: "Original channel description".into(),
        poster: "https://fixture.invalid/poster.jpg".into(),
        duration_ms: None,
        provider_id: Some("guide-one".into()),
        provider_id_field: None,
        live: Some(Default::default()),
        media: MediaLocator::Http {
            url: "https://fixture.invalid/live.m3u8".into(),
        },
    });
    runtime
}

fn response(runtime: &Runtime) -> Guide {
    let state = runtime.guide.as_ref().unwrap();
    let context = Context {
        generation: state.request_generation,
        profile_id: state.profile.id.clone(),
        entry_id: state.entry.id.clone(),
    };
    Guide { row_guide: None,
        source_profile: state.profile.clone(),
        generation: state.request_generation,
        profile_id: state.profile.id.clone(),
        entry_id: state.selection_id.clone(),
        entry: state.entry.clone(),
        request: Request {
            context: context.clone(),
            query: state.query,
        },
        result: Ok(ResultSet {
            context,
            query: state.query,
            coverage: Coverage::RequestedWindow,
            programmes: vec![Programme {
                channel: "guide-one".into(),
                start: 1699999920,
                end: 1700003520,
                title: "Programme one".into(),
                description: "First sentence. Complete second sentence.".into(),
            }],
        }),
    }
}

#[test]
fn live_guide_uses_existing_worker_and_keeps_full_programme_metadata() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    assert!(
        runtime.metadata_next.is_some(),
        "occupied worker retains next request"
    );
    let reply = response(&runtime);
    let id = programme_id(&reply.result.as_ref().unwrap().programmes[0]);
    assert!(
        accept(&mut runtime, &reply),
        "day reply does not enter current/next footer"
    );
    let view = view(&runtime).unwrap();
    let row = view.rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.description, "First sentence. Complete second sentence.");
    assert!(row.poster.is_some());
    assert_eq!(
        runtime.catalog.entries[0].description,
        "Original channel description"
    );
    assert!(matches!(row.action, OttAction::GuideProgramme { .. }));
}

#[test]
fn closed_guide_does_not_cancel_a_newer_local_metadata_owner() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    runtime.metadata_generation += 1;
    runtime.metadata_target = Some(("local".into(), "current".into()));
    let cancel = runtime.metadata_cancel.clone();
    close(&mut runtime);
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(
        runtime.metadata_target,
        Some(("local".into(), "current".into()))
    );
    assert_eq!(runtime.page, "channels");
}

#[test]
fn changed_source_disables_old_rows_and_rejects_old_response() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let reply = response(&runtime);
    runtime.saved.profiles.profiles[0].source = "https://fixture.invalid/replacement.m3u".into();
    accept(&mut runtime, &reply);
    assert!(runtime.guide.as_ref().unwrap().result.is_none());
    let view = view(&runtime).unwrap();
    assert!(view.rows.iter().all(|row| !row.enabled));
    assert_eq!(view.status, msg::ott_guide_source_changed());
}

#[test]
fn old_query_or_request_cannot_replace_a_new_day() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let old = response(&runtime);
    let state = runtime.guide.as_ref().unwrap().clone();
    let Query::Day { start, end } = state.query else {
        panic!("day fixture");
    };
    enqueue(
        &mut runtime,
        state,
        Query::Day {
            start: start + 86400,
            end: end + 86400,
        },
    )
    .unwrap();
    accept(&mut runtime, &old);
    assert!(runtime.guide.as_ref().unwrap().result.is_none());
    let mut forged = response(&runtime);
    forged.result.as_mut().unwrap().query = old.request.query;
    accept(&mut runtime, &forged);
    assert!(runtime.guide.as_ref().unwrap().result.is_none());
}

#[test]
fn rejected_interval_or_missing_favorite_does_not_replace_current_owner() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let state = runtime.guide.as_ref().unwrap().clone();
    let generation = runtime.metadata_generation;
    let target = runtime.metadata_target.clone();
    let cancel = runtime.metadata_cancel.clone();
    assert!(enqueue(&mut runtime, state.clone(), Query::Day { start: 1, end: 1 }).is_err());
    let mut missing = state;
    missing.selection_id = "favorite:missing".into();
    assert!(enqueue(
        &mut runtime,
        missing,
        Query::CurrentNext { now: 1700000000 }
    )
    .is_err());
    assert_eq!(runtime.metadata_generation, generation);
    assert_eq!(runtime.metadata_target, target);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.guide.is_some());
}

#[test]
fn absent_resolved_entry_reports_failure_instead_of_permanent_loading() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let error = Preview {
        generation: runtime.metadata_generation,
        profile_id: "fixture-guide".into(),
        entry_id: "channel-one".into(),
        entry: None,
        result: Err("Source lookup failed"),
    };
    assert!(accept_failure(&mut runtime, &error));
    assert_eq!(view(&runtime).unwrap().status, "Source lookup failed");
    close(&mut runtime);
    assert_eq!(runtime.page, "channels");
}

#[test]
fn changed_resolved_media_is_visible_failure_not_authority_for_old_archive() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let mut reply = response(&runtime);
    reply.entry.media = MediaLocator::Http {
        url: "https://fixture.invalid/other.m3u8".into(),
    };
    accept(&mut runtime, &reply);
    assert!(runtime.guide.as_ref().unwrap().result.is_none());
    assert!(runtime.guide.as_ref().unwrap().error.is_some());
    assert!(!view(&runtime)
        .unwrap()
        .rows
        .iter()
        .any(|row| matches!(row.action, OttAction::PlayArchive { .. })));
}

#[test]
fn catalogue_capability_exposes_m3u_guide_without_guessing_movie_subtype() {
    let runtime = fixture();
    let view = runtime.view();
    assert!(view.rows[0].guide_available);
    assert_eq!(
        view.rows[0].media.as_ref().unwrap().kind,
        plx_screens::ott::OttMediaKind::Video
    );
}

#[test]
fn local_plex_and_portal_keep_film_metadata_instead_of_a_guide() {
    for kind in [
        ProviderKind::Local,
        ProviderKind::Plex,
        ProviderKind::VPortal,
    ] {
        let mut runtime = fixture();
        runtime.saved.profiles.profiles[0].kind = kind;
        assert!(open(&mut runtime, "channel-one").is_err());
        assert!(runtime.guide.is_none());
        assert_eq!(
            runtime.catalog.entries[0].description,
            "Original channel description"
        );
    }
}

#[test]
fn duplicate_programme_identity_is_never_used_as_an_ambiguous_action() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let mut reply = response(&runtime);
    let rows = &mut reply.result.as_mut().unwrap().programmes;
    let mut duplicate = rows[0].clone();
    duplicate.title = "Conflicting title".into();
    let id = programme_id(&duplicate);
    rows.push(duplicate);
    accept(&mut runtime, &reply);
    assert!(selected(runtime.guide.as_ref().unwrap(), &id).is_err());
}

#[test]
fn deferred_worker_start_failure_lands_in_the_open_guide() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    start_failed(&mut runtime);
    assert_eq!(
        view(&runtime).unwrap().status,
        "Could not start guide request"
    );
}

#[test]
fn short_guide_refresh_advances_time_but_day_refresh_retains_civil_window() {
    assert_eq!(
        refreshed_query(Query::CurrentNext { now: 100 }, 200),
        Query::CurrentNext { now: 200 }
    );
    let day = Query::Day {
        start: 100,
        end: 10000,
    };
    assert_eq!(refreshed_query(day, 200), day);
}

#[test]
fn rejected_current_next_reply_is_consumed_before_footer_projection() {
    for mutate_source in [false, true] {
        let mut runtime = fixture();
        open(&mut runtime, "channel-one").unwrap();
        let state = runtime.guide.as_ref().unwrap().clone();
        enqueue(&mut runtime, state, Query::CurrentNext { now: 1700000000 }).unwrap();
        let mut reply = response(&runtime);
        if mutate_source {
            runtime.saved.profiles.profiles[0].source =
                "https://fixture.invalid/changed.m3u".into();
        } else {
            reply.entry.media = MediaLocator::Http {
                url: "https://fixture.invalid/changed.m3u8".into(),
            };
        }
        assert!(
            accept(&mut runtime, &reply),
            "rejected reply must not enter preview/footer path"
        );
        assert!(runtime.guide.as_ref().unwrap().result.is_none());
    }
}

#[test]
fn guide_working_status_is_only_the_current_unsettled_request() {
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    assert!(view(&runtime).unwrap().status_working, "a pending Guide response is typed as working");
    start_failed(&mut runtime);
    assert!(!view(&runtime).unwrap().status_working, "failure is settled status");

    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let reply = response(&runtime);
    assert!(accept(&mut runtime, &reply));
    assert!(!view(&runtime).unwrap().status_working, "accepted data is settled status");

    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    runtime.generation = runtime.generation.wrapping_add(1);
    assert!(!view(&runtime).unwrap().status_working, "retired source must not look pending");
}

#[test]
fn programme_timer_marker_follows_only_committed_source_bound_reminders() {
    use super::super::reminders::SaveFailure;
    use plx_data::ott::reminders::Record;
    let mut runtime = fixture();
    open(&mut runtime, "channel-one").unwrap();
    let mut reply = response(&runtime);
    let programme = &mut reply.result.as_mut().unwrap().programmes[0];
    programme.start = 1700000120;
    programme.end = 1700000240;
    let programme = programme.clone();
    let id = programme_id(&programme);
    assert!(accept(&mut runtime, &reply));
    runtime.reminders.load_committed(&runtime.saved, runtime.wall_clock_ms, 1).unwrap();
    let record = Record { key: Key {
        source_id: runtime.reminder_source_id().unwrap(),
        channel_id: "channel-one".into(), programme_id: id.clone(), start_s: programme.start,
    }, end_s: programme.end, title: programme.title.clone() };
    let marked = |runtime: &Runtime| view(runtime).unwrap().rows.iter()
        .find(|row| row.id == id).unwrap().favorite;
    assert!(!marked(&runtime));
    let (_, rejected) = runtime.reminders.toggle(&runtime.saved, record.clone(), runtime.wall_clock_ms).unwrap();
    assert!(!marked(&runtime), "an unacknowledged save cannot acquire a marker");
    runtime.reminders.finish_save(rejected, Err(SaveFailure::Rejected)).unwrap();
    assert!(!marked(&runtime));
    let (saved, token) = runtime.reminders.toggle(&runtime.saved, record.clone(), runtime.wall_clock_ms).unwrap();
    runtime.reminders.finish_save(token, Ok(&saved)).unwrap();
    runtime.saved = saved;
    assert!(marked(&runtime));
    runtime.reminders.invalidate(); assert!(!marked(&runtime));
    runtime.reminders.load_committed(&runtime.saved, runtime.wall_clock_ms, 2).unwrap();
    assert!(marked(&runtime), "a verified reload restores the same marker");
    runtime.saved.profiles.profiles[0].source.push_str("?other");
    assert!(!marked(&runtime), "same programme under a changed source is not its timer");
    runtime.saved.profiles.profiles[0].source.truncate("https://fixture.invalid/list.m3u".len());
    assert!(marked(&runtime));
    runtime.wall_clock_ms = programme.end as u64 * 1000;
    assert!(!marked(&runtime), "an ended timer has no pending marker");
    runtime.wall_clock_ms = 1700000000000;
    let (saved, token) = runtime.reminders.toggle(&runtime.saved, record, runtime.wall_clock_ms).unwrap();
    runtime.reminders.finish_save(token, Ok(&saved)).unwrap(); runtime.saved = saved;
    assert!(!marked(&runtime));
}

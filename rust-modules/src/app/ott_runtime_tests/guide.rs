//! Typed guide replies enter through the runtime's existing metadata receiver.
//! The clock is captured once at the app boundary and reused for every tick;
//! catalogue, metadata and resolver producer slots stay held, so no IO starts.
use super::*;
use crate::app::ott_metadata::{Event, Guide};
use plx_base::task::time::ClockSample;
use plx_data::ott::{
    epg::Programme,
    guide::{ChannelMetadata, Context, Coverage, Query, Request, ResultSet},
};

#[path = "row_guide.rs"]
mod row_guide;

fn fixture() -> (App, ClockSample, Guide) {
    let mut app = app_fixture();
    let sample = ClockSample::capture_main(app.adapters.player.mt()).expect("fixture clock");
    let now = (sample.unix_ms() / 1000) as i64;
    let profile = ProviderProfile {
        id: "guide-profile".into(),
        name: "Guide fixture".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/guide-fixture.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let mut entry = synthetic_entry();
    entry.id = "guide-channel".into();
    entry.title = "Fixture channel".into();
    entry.description = "Description before guide".into();
    entry.duration_ms = None;
    entry.media = MediaLocator::Http {
        url: "https://media.invalid/guide-fixture.m3u8".into(),
    };
    entry.live = Some(ChannelMetadata {
        tvg_name: "Fixture channel".into(),
        guide_shift_seconds: 0,
        catchup: None,
    });
    let context = Context {
        generation: 17,
        profile_id: profile.id.clone(),
        entry_id: entry.id.clone(),
    };
    let query = Query::CurrentNext { now };
    let reply = Guide { row_guide: None,
        source_profile: profile.clone(),
        generation: context.generation,
        profile_id: context.profile_id.clone(),
        entry_id: context.entry_id.clone(),
        entry: entry.clone(),
        request: Request {
            context: context.clone(),
            query,
        },
        result: Ok(ResultSet {
            context,
            query,
            coverage: Coverage::RequestedWindow,
            programmes: vec![Programme {
                channel: "xmltv-fixture-channel".into(),
                start: now.saturating_sub(60),
                end: now.saturating_add(60),
                title: "Current fixture programme".into(),
                description: "Complete accepted programme description".into(),
            }],
        }),
    };
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = profile.kind;
    runtime.loaded = true;
    runtime.page = "channels";
    runtime.catalog.entries = vec![entry.clone()];
    runtime.catalog_dirty = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.write_permit.revoke();
    runtime.selected = Some(entry.id.clone());
    runtime.metadata_generation = reply.generation;
    runtime.metadata_target = Some((profile.id, entry.id));
    runtime.metadata_query = Some(query);
    runtime.metadata_refresh_ms = 0;
    (app, sample, reply)
}

fn deliver(app: &mut App, sample: ClockSample, reply: Guide) {
    app.ott
        .as_ref()
        .unwrap()
        .metadata_tx
        .send(Event::Guide(reply))
        .unwrap_or_else(|_| panic!("fixture metadata receiver disappeared"));
    super::super::tick(app, sample);
}

fn assert_rejected(app: &mut App, sample: ClockSample, reply: Guide) {
    let runtime = app.ott.as_ref().unwrap();
    let entries = runtime.catalog.entries.clone();
    let revision = runtime.catalog_revision;
    let preview = runtime.favorite_preview.clone();
    deliver(app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(runtime.catalog_revision, revision);
    assert_eq!(runtime.favorite_preview, preview);
    assert_eq!(
        runtime.metadata_refresh_ms, 0,
        "rejected reply scheduled refresh"
    );
}

#[test]
fn exact_typed_guide_reply_enriches_only_the_current_catalogue_entry() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    let mut unrelated = synthetic_entry();
    unrelated.id = "other-channel".into();
    unrelated.description = "Unrelated description".into();
    app.ott
        .as_mut()
        .unwrap()
        .catalog
        .entries
        .push(unrelated.clone());
    let revision = app.ott.as_ref().unwrap().catalog_revision;
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.catalog.entries[0].description,
        "Complete accepted programme description"
    );
    assert_eq!(runtime.catalog.entries[0].title, "Fixture channel");
    assert_eq!(runtime.catalog.entries[1], unrelated);
    assert_eq!(runtime.catalog_revision, revision.wrapping_add(1));
    assert!(runtime.metadata_refresh_ms > 0);
    assert!(runtime.favorite_preview.is_none());
}

#[test]
fn self_consistent_old_now_reply_cannot_replace_the_current_query() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let Query::CurrentNext { now } = reply.request.query else { panic!("current/next fixture"); };
    reply.request.query = Query::CurrentNext { now: now - 1 };
    reply.result.as_mut().unwrap().query = reply.request.query;
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn self_consistent_old_generation_reply_is_not_published() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    reply.generation -= 1;
    reply.request.context.generation = reply.generation;
    reply.result.as_mut().unwrap().context.generation = reply.generation;
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn reply_for_the_previous_selected_id_is_not_published() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.selected = Some("new-selection".into());
    runtime.metadata_target.as_mut().unwrap().1 = "new-selection".into();
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn reply_for_the_previous_profile_is_not_published() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .metadata_target
        .as_mut()
        .unwrap()
        .0 = "new-profile".into();
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn cancelled_typed_reply_does_not_enrich_or_schedule_refresh() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    app.ott
        .as_ref()
        .unwrap()
        .metadata_cancel
        .store(true, Ordering::Release);
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn result_context_cannot_borrow_an_otherwise_current_outer_request() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    reply.result.as_mut().unwrap().context.entry_id = "another-resolved-channel".into();
    assert_rejected(&mut app, sample, reply);
}

#[test]
fn favorite_guide_retains_outer_selection_and_enriches_its_resolved_entry() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let favorite = selection::capture(
        &app.ott.as_ref().unwrap().saved.profiles.profiles[0],
        &reply.entry,
    )
    .unwrap();
    let outer = format!("favorite:{}", favorite.identity());
    reply.entry_id = outer.clone();
    let resolved = reply.entry.id.clone();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.page = "favorites";
    runtime.selected = Some(outer.clone());
    runtime.metadata_target.as_mut().unwrap().1 = outer.clone();
    let original_catalogue = runtime.catalog.entries.clone();
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    let (selection, preview) = runtime
        .favorite_preview
        .as_ref()
        .expect("accepted favorite preview");
    assert_eq!(selection, &outer);
    assert_eq!(preview.id, resolved);
    assert_eq!(
        preview.description,
        "Complete accepted programme description"
    );
    assert_eq!(
        runtime.catalog.entries, original_catalogue,
        "a favorite preview must not retarget a catalogue row"
    );
    assert!(runtime.metadata_refresh_ms > 0);
}

#[test]
fn obsolete_favorite_reply_cannot_replace_the_current_favorite_preview() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    reply.entry_id = "favorite:previous-selection".into();
    let runtime = app.ott.as_mut().unwrap();
    runtime.metadata_target.as_mut().unwrap().1 = "favorite:current-selection".into();
    runtime.favorite_preview = Some(("favorite:current-selection".into(), synthetic_entry()));
    assert_rejected(&mut app, sample, reply);
}


#[test]
fn classic_selected_guide_retains_now_next_only_for_its_row() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let now = (sample.unix_ms() / 1000) as i64;
    reply.result.as_mut().unwrap().programmes.push(Programme {
        channel: "xmltv-fixture-channel".into(), start: now + 60, end: now + 3600,
        title: "Next fixture programme".into(), description: "Full next description.\nFinal paragraph.".into(),
    });
    let mut other = synthetic_entry(); other.id = "other-guide-row".into();
    app.ott.as_mut().unwrap().catalog.entries.push(other);
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.raw_view();
    let row = view.rows.iter().find(|row| row.id == "guide-channel").unwrap();
    let facts = row.guide.as_ref().expect("selected guide is projected");
    assert_eq!(facts.now_s, now);
    let current = facts.current.as_ref().unwrap();
    assert_eq!(current.title, "Current fixture programme");
    assert_eq!((current.start_s, current.end_s), (now - 60, now + 60));
    assert_eq!(current.description, "Complete accepted programme description");
    assert_eq!(facts.next.as_ref().unwrap().description, "Full next description.\nFinal paragraph.");
    assert!(view.rows.iter().filter(|row| row.id != "guide-channel").all(|row| row.guide.is_none()));
    assert_eq!(row.content_title.as_deref(), Some("Fixture channel"));
}

#[test]
fn classic_empty_guide_replaces_the_previous_selected_programme() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    let mut empty = Guide { row_guide: None,
        generation: reply.generation, profile_id: reply.profile_id.clone(),
        source_profile: reply.source_profile.clone(), entry_id: reply.entry_id.clone(),
        entry: reply.entry.clone(), request: reply.request.clone(), result: reply.result.clone(),
    };
    empty.result.as_mut().unwrap().programmes.clear();
    deliver(&mut app, sample, reply);
    deliver(&mut app, sample, empty);
    let view = app.ott.as_ref().unwrap().raw_view();
    let facts = view.rows.iter().find(|row| row.id == "guide-channel").unwrap().guide.as_ref().unwrap();
    assert!(facts.current.is_none() && facts.next.is_none());
    assert!(view.rows.iter().find(|row| row.id == "guide-channel").unwrap().description.is_empty());
}

#[test]
fn classic_guide_checks_dispatch_profile_not_just_current_profile_id() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].source = "https://media.invalid/changed-source.m3u".into();
    assert_rejected(&mut app, sample, reply);
    assert!(app.ott.as_ref().unwrap().raw_view().rows.iter().all(|row| row.guide.is_none()));
}

#[test]
fn classic_day_reply_does_not_become_a_selected_now_next_projection() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let now = (sample.unix_ms() / 1000) as i64;
    let query = Query::Day { start: now - 60, end: now + 3600 };
    reply.request.query = query; reply.result.as_mut().unwrap().query = query;
    app.ott.as_mut().unwrap().metadata_query = Some(query);
    deliver(&mut app, sample, reply);
    assert!(app.ott.as_ref().unwrap().raw_view().rows.iter().all(|row| row.guide.is_none()));
}

#[test]
fn classic_cancelled_owner_does_not_expose_retained_guide_facts() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    deliver(&mut app, sample, reply);
    app.ott.as_mut().unwrap().metadata_cancel.store(true, Ordering::Release);
    assert!(app.ott.as_ref().unwrap().raw_view().rows.iter().all(|row| row.guide.is_none()));
}

#[test]
fn classic_nonactive_favorite_source_change_retires_aba_guide_reply() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let source = reply.source_profile.clone();
    let favorite = selection::capture(&source, &reply.entry).unwrap();
    let selection_id = format!("favorite:{}", favorite.identity());
    reply.entry_id = selection_id.clone();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(favorite).unwrap();
    let mut active = source.clone(); active.id = "other-active-profile".into();
    active.name = "Other active source".into(); active.source = "https://media.invalid/other.m3u".into();
    runtime.saved.profiles.active_profile = Some(active.id.clone());
    runtime.saved.profiles.profiles.push(active);
    runtime.page = "favorites";
    runtime.saved.favorites_selected = true;
    runtime.metadata_target.as_mut().unwrap().1 = selection_id.clone();
    let generation = runtime.metadata_generation;
    let mut changed = runtime.saved.clone();
    changed.profiles.profiles[0].source = "https://media.invalid/temporary-source.m3u".into();
    let restored = runtime.saved.clone();
    for saved in [changed, restored] {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saving = true; runtime.save_navigation = SaveNavigation::Preserve;
        runtime.save_ui_owner = None;
        runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"}))).unwrap_or_else(|_| panic!("fixture receiver missing"));
        super::super::tick(&mut app, sample);
    }
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.metadata_generation != generation);
    assert!(runtime.metadata_target.is_none());
    assert_rejected(&mut app, sample, reply);
    assert!(app.ott.as_ref().unwrap().raw_view().rows.iter().all(|row| row.guide.is_none()));
}


fn copy_guide(reply: &Guide) -> Guide {
    Guide { row_guide: None, generation: reply.generation, profile_id: reply.profile_id.clone(),
        source_profile: reply.source_profile.clone(), entry_id: reply.entry_id.clone(),
        entry: reply.entry.clone(), request: reply.request.clone(), result: reply.result.clone() }
}

#[test]
fn classic_current_error_clears_facts_but_stale_error_cannot_clear_newer_facts() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = fixture();
    let mut current_error = copy_guide(&reply); current_error.result = Err("Fixture guide unavailable");
    let mut stale_error = copy_guide(&current_error); stale_error.generation -= 1;
    stale_error.request.context.generation = stale_error.generation;
    deliver(&mut app, sample, reply);
    let before = app.ott.as_ref().unwrap().raw_view().rows[0].guide.clone();
    assert!(before.is_some());
    deliver(&mut app, sample, stale_error);
    assert_eq!(app.ott.as_ref().unwrap().raw_view().rows[0].guide, before);
    deliver(&mut app, sample, current_error);
    assert!(app.ott.as_ref().unwrap().raw_view().rows[0].guide.is_none());
}

#[test]
fn classic_guide_uses_wall_clock_boundary_without_mutating_retained_facts() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = fixture();
    let now = (sample.unix_ms() / 1000) as i64;
    reply.result.as_mut().unwrap().programmes.push(Programme {
        channel: "xmltv-fixture-channel".into(), start: now + 60, end: now + 3600,
        title: "Next fixture programme".into(), description: "Full next description".into(),
    });
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_mut().unwrap();
    runtime.wall_clock_ms = ((now + 60) as u64) * 1000;
    let advanced = runtime.raw_view().rows[0].guide.clone().unwrap();
    assert_eq!(advanced.current.unwrap().title, "Next fixture programme");
    assert!(advanced.next.is_none());
    runtime.wall_clock_ms = (now as u64) * 1000;
    assert_eq!(runtime.raw_view().rows[0].guide.as_ref().unwrap().current.as_ref().unwrap().title,
        "Current fixture programme");
}

//! Guide/archive composition with actual Runtime admission. Existing producer
//! slots stay held; these fixtures do not start storage, provider or decoder IO.
use super::*;
use crate::app::ott_metadata::Guide;
use plx_data::ott::{
    archive::{ArchiveInterval, CatchupMode, CatchupSource},
    epg::Programme,
    guide::{ChannelMetadata, Context, Coverage, Request, ResultSet},
};

fn fixture() -> (Box<App>, Runtime) {
    let mut app = Box::new(app_fixture());
    let mut runtime = app.ott.take().unwrap();
    let profile = ProviderProfile {
        id: "guide-admission".into(),
        name: "Guide".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let mut entry = synthetic_entry();
    entry.id = "channel".into();
    entry.title = "Fixture channel".into();
    entry.media = MediaLocator::Http {
        url: "https://media.invalid/live.m3u8".into(),
    };
    entry.duration_ms = None;
    entry.provider_id = Some("channel".into());
    entry.provider_id_field = Some("tvg-id".into());
    entry.live = Some(ChannelMetadata { guide_id: None, guide_feed: String::new(),
        tvg_name: entry.title.clone(),
        guide_shift_seconds: 0,
        catchup: Some(CatchupSource {
            mode: CatchupMode::Append,
            source: "?start={utc}&end={utcend}".into(),
            retention_seconds: 86400,
            correction_seconds: 0,
        }),
    });
    runtime.loaded = true;
    runtime.entropy = Some([9; 16]);
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles = vec![profile];
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![entry];
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.catalog_dirty = false;
    runtime.page = "channels";
    assert!(
        runtime.metadata_work_allowed(),
        "normal fixture completed its frame lifecycle rearm"
    );
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.write_permit.revoke();
    runtime.wall_clock_ms = 1_700_000_000_000;
    (app, runtime)
}
fn archive(runtime: &Runtime) -> PlaybackRequest {
    let mut request = PlaybackRequest::ordinary("channel", 0);
    request.advance_on_failure = false;
    request.archive = Some(
        archive_play::Intent::new(
            runtime.active().unwrap(),
            &runtime.catalog.entries[0],
            ArchiveInterval {
                start: 1_699_998_800,
                end: 1_699_999_400,
            },
            1_700_000_000,
        )
        .unwrap(),
    );
    request
}
fn pin(runtime: &mut Runtime, app: &App, strict: bool) {
    let prepared = kiosk_source::prepare(runtime, app, Some("1"), strict).unwrap();
    runtime.saved = kiosk_source::save_candidate(runtime, &prepared).unwrap();
}
fn show_programme(runtime: &mut Runtime, app: &mut App) -> OttAction {
    handle_action(runtime, app, OttAction::OpenGuide("channel".into())).unwrap();
    let profile = runtime.active().unwrap();
    let context = Context {
        generation: runtime.metadata_generation,
        profile_id: profile.id.clone(),
        entry_id: "channel".into(),
    };
    let query = runtime.metadata_query.unwrap();
    let reply = Guide { row_guide: None,
        source_profile: profile.clone(),
        generation: context.generation,
        profile_id: context.profile_id.clone(),
        entry_id: context.entry_id.clone(),
        entry: runtime.catalog.entries[0].clone(),
        request: Request {
            context: context.clone(),
            query,
        },
        result: Ok(ResultSet {
            context,
            query,
            coverage: Coverage::RequestedWindow,
            programmes: vec![Programme {
                channel: "channel".into(),
                start: 1_699_998_800,
                end: 1_699_999_400,
                title: "Archived programme".into(),
                description: "Full description".into(),
            }],
        }),
    };
    assert!(guide_ui::accept(runtime, &reply));
    let programme = runtime
        .view()
        .rows
        .into_iter()
        .find(|row| matches!(row.action, OttAction::GuideProgramme { .. }))
        .unwrap()
        .action;
    handle_action(runtime, app, programme).unwrap();
    runtime
        .view()
        .rows
        .into_iter()
        .find(|row| matches!(row.action, OttAction::PlayArchive { .. }))
        .unwrap()
        .action
}
#[test]
fn strict_pin_fences_guide_before_cancellation_or_ui_mutation() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    pin(&mut runtime, &app, true);
    let cancel = runtime.metadata_cancel.clone();
    let generation = runtime.metadata_generation;
    assert!(handle_action(
        &mut runtime,
        &mut app,
        OttAction::OpenGuide("channel".into())
    )
    .is_err());
    assert_eq!(runtime.page, "channels");
    assert!(runtime.guide.is_none());
    assert_eq!(runtime.metadata_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.play_target.is_none());
}
#[test]
fn normal_pin_guide_back_restores_browse_without_retiring_source() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    pin(&mut runtime, &app, false);
    let generation = runtime.play_generation;
    show_programme(&mut runtime, &mut app);
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "guide");
    assert!(runtime.guide.is_some());
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(runtime.guide.is_none());
    assert_eq!(runtime.play_generation, generation);
    assert!(!runtime.saving);
}
#[test]
fn guide_archive_keeps_same_pin_and_has_no_live_recovery_receipt() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    pin(&mut runtime, &app, false);
    let saved = runtime.saved.settings["nativeKiosk"].clone();
    let action = show_programme(&mut runtime, &mut app);
    handle_action(&mut runtime, &mut app, action).unwrap();
    let request = runtime.play_target.as_ref().unwrap();
    assert_eq!(request.id, "channel");
    assert!(!request.advance_on_failure);
    assert_eq!(
        request.archive.as_ref().unwrap().programme().unwrap().title,
        "Archived programme"
    );
    assert!(runtime.resolver_next.as_ref().unwrap().archive.is_some());
    assert_eq!(runtime.saved.settings["nativeKiosk"], saved);
    assert!(kiosk_source::admitted_pinned_request(&runtime).is_none());
    let generation = runtime.play_generation;
    runtime.kiosk_recovery_tick_at(&mut app, 0);
    runtime.kiosk_recovery_tick_at(&mut app, 120_000);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.as_ref().unwrap().archive.is_some());
}
#[test]
fn retired_workers_reject_archive_before_changing_playback_intent() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    let request = archive(&runtime);
    runtime.suspend_source_work(&mut app);
    runtime.resume_source_work();
    assert!(!runtime.play_work_allowed());
    let generation = runtime.play_generation;
    let epoch = runtime.playback_epoch;
    let selected = runtime.selected.clone();
    let cancel = runtime.play_cancel.clone();
    assert_eq!(
        runtime.play_request_with_policy(&mut app, request),
        Err("Media work is waiting for foreground or worker retirement")
    );
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(runtime.selected, selected);
    assert!(Arc::ptr_eq(&runtime.play_cancel, &cancel));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}
#[test]
fn archive_validation_uses_wall_time_and_refuses_implicit_continuation() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    let request = archive(&runtime);
    assert!(runtime.prepare_play_request(request.clone()).is_ok());
    let mut resumed = request.clone();
    resumed.position_ms = 1000;
    assert!(runtime.play_request_with_policy(&mut app, resumed).is_err());
    assert!(runtime.play_target.is_none());
    runtime.wall_clock_ms += 86_400_000;
    assert!(runtime.play_request_with_policy(&mut app, request).is_err());
    assert!(runtime.play_target.is_none());
}

#[test]
fn parental_rejection_keeps_archive_out_of_the_resolver() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    runtime.catalog.entries[0].protection = Some(true);
    let request = archive(&runtime);
    runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    let generation = runtime.play_generation;
    let epoch = runtime.playback_epoch;
    assert!(runtime.play_request_with_policy(&mut app, request).is_err());
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.playback_epoch, epoch);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!runtime.kiosk_capture.pending());
}

fn reject_guide_without_replacing_metadata(
    runtime: &mut Runtime,
    app: &mut App,
    action: OttAction,
) {
    let generation = runtime.metadata_generation;
    let target = runtime.metadata_target.clone();
    let query = runtime.metadata_query;
    let refresh = runtime.metadata_refresh_ms;
    let cancel = runtime.metadata_cancel.clone();
    let cancelled = cancel.load(Ordering::Acquire);
    let queued = runtime.metadata_next.is_some();
    let page = runtime.page;
    let guide = runtime.guide.is_some();
    assert!(handle_action(runtime, app, action).is_err());
    assert_eq!(runtime.metadata_generation, generation);
    assert_eq!(runtime.metadata_target, target);
    assert_eq!(runtime.metadata_query, query);
    assert_eq!(runtime.metadata_refresh_ms, refresh);
    assert!(Arc::ptr_eq(&runtime.metadata_cancel, &cancel));
    assert_eq!(cancel.load(Ordering::Acquire), cancelled);
    assert_eq!(runtime.metadata_next.is_some(), queued);
    assert_eq!(runtime.page, page);
    assert_eq!(runtime.guide.is_some(), guide);
}

#[test]
fn suspended_guide_refuses_new_queries_but_keeps_back_navigation() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    let programme = show_programme(&mut runtime, &mut app);
    let OttAction::PlayArchive { generation, .. } = programme else {
        panic!("archive fixture")
    };
    let plx_data::ott::guide::Query::Day { start, .. } = runtime.metadata_query.unwrap() else {
        panic!("day fixture")
    };
    runtime.suspend_source_work(&mut app);
    assert!(!runtime.metadata_work_allowed());
    for action in [
        OttAction::OpenGuide("channel".into()),
        OttAction::GuideDay {
            generation,
            day_start_s: start,
        },
        OttAction::Refresh,
    ] {
        reject_guide_without_replacing_metadata(&mut runtime, &mut app, action);
    }
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "guide");
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(runtime.guide.is_none());
}

#[test]
fn guide_waits_for_metadata_retirement_even_when_catalogue_work_has_resumed() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = fixture();
    runtime.suspend_source_work(&mut app);
    runtime.resume_source_work();
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert!(runtime.play_work_allowed(), "catalogue worker has retired");
    assert!(
        !runtime.metadata_work_allowed(),
        "old metadata worker still owns its slot"
    );
    reject_guide_without_replacing_metadata(
        &mut runtime,
        &mut app,
        OttAction::OpenGuide("channel".into()),
    );
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert!(runtime.metadata_work_allowed());
    // Hold the producer slot after lifecycle completion to inspect the queued
    // request without starting any IO. A busy healthy worker can retain next.
    runtime.metadata_done.store(false, Ordering::Release);
    handle_action(
        &mut runtime,
        &mut app,
        OttAction::OpenGuide("channel".into()),
    )
    .unwrap();
    assert_eq!(runtime.page, "guide");
    assert!(runtime.guide.is_some() && runtime.metadata_next.is_some());
}

#[path = "guide_order.rs"]
mod order;

#[path = "records.rs"]
mod records;

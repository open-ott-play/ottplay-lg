//! Records through the real Runtime action/Guide receiver, with existing
//! producer slots held. No provider, decoder, network or persistence IO.
use super::*;
use plx_data::ott::guide::Query;

const NOW: i64 = 1_700_000_000;
const DEPTH: u64 = 3 * 86400;

fn records_fixture() -> (Box<App>, Runtime) {
    let (app, mut runtime) = fixture();
    runtime.catalog.entries[0].live.as_mut().unwrap().catchup.as_mut().unwrap()
        .retention_seconds = DEPTH;
    (app, runtime)
}

fn programme(title: &str, start: i64, end: i64, description: &str) -> Programme {
    Programme { channel: "channel".into(), start, end, title: title.into(), description: description.into() }
}

fn response(runtime: &Runtime, programmes: Vec<Programme>) -> Guide {
    let profile = runtime.active().unwrap();
    let context = Context { generation: runtime.metadata_generation,
        profile_id: profile.id.clone(), entry_id: "channel".into() };
    let query = runtime.metadata_query.unwrap();
    Guide { row_guide: None, source_profile: profile.clone(), generation: context.generation,
        profile_id: context.profile_id.clone(), entry_id: context.entry_id.clone(),
        entry: runtime.catalog.entries[0].clone(),
        request: Request { context: context.clone(), query },
        result: Ok(ResultSet { context, query, coverage: Coverage::RequestedWindow, programmes }) }
}

fn open_records(app: &mut App, runtime: &mut Runtime) {
    handle_action(runtime, app, OttAction::OpenRecords("channel".into())).unwrap();
    assert_eq!(runtime.metadata_query, Some(Query::Records { start: NOW - DEPTH as i64, end: NOW }));
    assert!(runtime.metadata_next.is_some(), "existing held worker owns the one queued request");
}

fn rows(runtime: &Runtime) -> Vec<OttRow> {
    runtime.view().rows.into_iter()
        .filter(|row| matches!(row.action, OttAction::GuideProgramme { .. })).collect()
}

fn archive_action(app: &mut App, runtime: &mut Runtime) -> OttAction {
    open_records(app, runtime);
    let reply = response(runtime, vec![programme("Past", NOW - 600, NOW - 300, "Full programme")]);
    assert!(guide_ui::accept(runtime, &reply), "Records cannot replace the live current/next footer");
    let selected = rows(runtime).remove(0).action;
    handle_action(runtime, app, selected).unwrap();
    runtime.view().rows.into_iter().find(|row| matches!(row.action, OttAction::PlayArchive { .. })).unwrap().action
}

#[test]
fn records_full_retention_has_classic_projection_without_changing_source_or_starting_playback() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = records_fixture();
    assert!(runtime.view().rows[0].records_available);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let entries = runtime.catalog.entries.clone();
    let play_generation = runtime.play_generation;
    open_records(&mut app, &mut runtime);
    let input = vec![
        programme("Zulu", NOW - 2 * 86400, NOW - 2 * 86400 + 600, "Older than one day"),
        programme("Same", NOW - 2000, NOW - 1800, "Old duplicate"),
        programme("Same", NOW - 1000, NOW - 800, "Latest duplicate"),
        programme("Alpha", NOW - 700, NOW - 500, "Alphabetical first"),
        programme("Current", NOW - 20, NOW + 60, "Not completed"),
        programme("At cutoff", NOW - DEPTH as i64, NOW - DEPTH as i64 + 50, "Strict bound"),
        programme("Future", NOW + 100, NOW + 300, "Not completed"),
    ];
    let reply = response(&runtime, input.clone());
    assert!(guide_ui::accept(&mut runtime, &reply));
    let visible = rows(&runtime);
    assert_eq!(visible.iter().map(|row| row.title.as_str()).collect::<Vec<_>>(), ["Alpha", "Same", "Zulu"]);
    assert_eq!(visible[1].description, "Latest duplicate");
    assert_eq!(reply.result.unwrap().programmes, input, "original publication stays intact");
    assert!(!runtime.view().rows.iter().any(|row| matches!(row.action, OttAction::GuideDay { .. } | OttAction::GuideOrder { .. })));
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.play_generation, play_generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none() && !runtime.saving);
}

#[test]
fn records_overlapping_feed_duplicate_selects_retained_metadata_and_uses_existing_archive_path() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = records_fixture();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    // Obtain the real historical row identity from ordinary Guide rather than
    // reimplementing its hash. It must lose authority when Records hides it.
    handle_action(&mut runtime, &mut app, OttAction::OpenGuide("channel".into())).unwrap();
    let repeated = vec![programme("Same", NOW - 1200, NOW - 900, "Hidden old"),
        programme("Same", NOW - 600, NOW - 300, "Visible latest")];
    let day = response(&runtime, repeated.clone());
    assert!(guide_ui::accept(&mut runtime, &day));
    let old_id = rows(&runtime).into_iter().find(|row| row.description == "Hidden old").unwrap().id;
    open_records(&mut app, &mut runtime);
    let records = response(&runtime, repeated);
    assert!(guide_ui::accept(&mut runtime, &records));
    let hidden_action = OttAction::GuideProgramme { generation: runtime.metadata_generation, programme_id: old_id };
    assert!(handle_action(&mut runtime, &mut app, hidden_action).is_err());
    let ambiguous = response(&runtime, vec![
        programme("Title A", NOW - 600, NOW - 300, "Conflicting feed"),
        programme("Title B", NOW - 600, NOW - 300, "Conflicting feed"),
    ]);
    assert!(guide_ui::accept(&mut runtime, &ambiguous));
    let forged = rows(&runtime).remove(0).action;
    assert!(handle_action(&mut runtime, &mut app, forged).is_err(), "two visible programmes cannot share identity");
    assert!(runtime.play_target.is_none());
    let reply = response(&runtime, vec![
        programme("Same", NOW - 600, NOW - 300, "First feed"),
        programme("Same", NOW - 600, NOW - 300, "Later feed full description"),
    ]);
    assert!(guide_ui::accept(&mut runtime, &reply));
    let visible = rows(&runtime);
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].description, "Later feed full description");
    handle_action(&mut runtime, &mut app, visible[0].action.clone()).unwrap();
    let action = runtime.view().rows.into_iter().find(|row| matches!(row.action, OttAction::PlayArchive { .. })).unwrap().action;
    handle_action(&mut runtime, &mut app, action).unwrap();
    let request = runtime.play_target.as_ref().unwrap();
    assert_eq!(request.id, "channel");
    assert!(!request.advance_on_failure);
    assert_eq!(request.archive.as_ref().unwrap().programme().unwrap().description, "Later feed full description");
    assert!(runtime.resolver_next.as_ref().unwrap().archive.is_some());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn records_empty_failed_and_cancelled_views_allow_back_and_discard_late_results() {
    let _serial = plx_base::testlock::serial();
    for result in ["empty", "failed", "pending"] {
        let (mut app, mut runtime) = records_fixture();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let source = runtime.generation;
        let play = runtime.play_generation;
        open_records(&mut app, &mut runtime);
        let late = response(&runtime, vec![programme("Late", NOW - 600, NOW - 300, "Late")]);
        if result != "pending" {
            let mut reply = response(&runtime, Vec::new());
            if result == "failed" { reply.result = Err("Records feed is unavailable"); }
            assert!(guide_ui::accept(&mut runtime, &reply));
            assert!(rows(&runtime).is_empty());
            assert!(!runtime.view().status.is_empty());
        }
        let cancel = runtime.metadata_cancel.clone();
        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
        assert_eq!(runtime.page, "channels");
        assert!(runtime.guide.is_none() && runtime.metadata_next.is_none());
        assert!(cancel.load(Ordering::Acquire));
        assert!(guide_ui::accept(&mut runtime, &late));
        assert!(runtime.guide.is_none());
        assert_eq!((runtime.generation, runtime.play_generation), (source, play));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    }
}

#[test]
fn records_refresh_replaces_the_whole_window_and_never_degrades_to_one_day() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = records_fixture();
    open_records(&mut app, &mut runtime);
    let old = response(&runtime, vec![programme("Old", NOW - 600, NOW - 300, "Old")]);
    let generation = runtime.metadata_generation;
    for action in [OttAction::GuideDay { generation, day_start_s: NOW },
        OttAction::GuideOrder { generation, order: plx_screens::ott::OttGuideOrder::Chronological }] {
        reject_guide_without_replacing_metadata(&mut runtime, &mut app, action);
    }
    runtime.wall_clock_ms += 60_000;
    handle_action(&mut runtime, &mut app, OttAction::Refresh).unwrap();
    assert!(runtime.metadata_generation > generation);
    assert_eq!(runtime.metadata_query, Some(Query::Records { start: NOW + 60 - DEPTH as i64, end: NOW + 60 }));
    assert!(guide_ui::accept(&mut runtime, &old));
    assert!(rows(&runtime).is_empty());
    let mut short = response(&runtime, vec![programme("Partial", NOW - 600, NOW - 300, "Short")]);
    short.result.as_mut().unwrap().query = Query::CurrentNext { now: NOW + 60 };
    assert!(guide_ui::accept(&mut runtime, &short));
    assert!(rows(&runtime).is_empty(), "a short result cannot masquerade as complete Records");
}

#[test]
fn records_rejects_unqualified_sources_before_metadata_cancellation() {
    let _serial = plx_base::testlock::serial();
    for kind in ["no-rule", "zero-depth", "portal", "finite", "too-deep"] {
        let (mut app, mut runtime) = records_fixture();
        match kind {
            "no-rule" => runtime.catalog.entries[0].live.as_mut().unwrap().catchup = None,
            "zero-depth" => runtime.catalog.entries[0].live.as_mut().unwrap().catchup.as_mut().unwrap().retention_seconds = 0,
            "portal" => runtime.saved.profiles.profiles[0].kind = ProviderKind::VPortal,
            "finite" => runtime.catalog.entries[0].duration_ms = Some(60_000),
            "too-deep" => runtime.catalog.entries[0].live.as_mut().unwrap().catchup.as_mut().unwrap().retention_seconds = 366 * 86400,
            _ => unreachable!(),
        }
        assert!(!runtime.view().rows[0].records_available, "{kind}");
        reject_guide_without_replacing_metadata(&mut runtime, &mut app, OttAction::OpenRecords("channel".into()));
    }
}

#[test]
fn records_archive_rechecks_parental_policy_source_and_expiry_before_resolver_admission() {
    let _serial = plx_base::testlock::serial();
    for change in ["parental", "source", "expiry", "strict-kiosk"] {
        let (mut app, mut runtime) = records_fixture();
        let action = archive_action(&mut app, &mut runtime);
        match change {
            "parental" => { runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65")); }
            "source" => runtime.saved.profiles.profiles[0].source = "https://media.invalid/changed.m3u".into(),
            "expiry" => runtime.wall_clock_ms += DEPTH * 1000,
            "strict-kiosk" => pin(&mut runtime, &app, true),
            _ => unreachable!(),
        }
        let generation = runtime.play_generation;
        assert!(handle_action(&mut runtime, &mut app, action).is_err(), "{change}");
        assert_eq!(runtime.play_generation, generation);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
    let (mut app, mut runtime) = records_fixture();
    pin(&mut runtime, &app, true);
    reject_guide_without_replacing_metadata(&mut runtime, &mut app, OttAction::OpenRecords("channel".into()));
}

#[test]
fn records_fallback_change_retires_old_window_without_rebinding_source() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = records_fixture();
    runtime.catalog.entries[0].live.as_mut().unwrap().catchup.as_mut().unwrap().retention_seconds = 0;
    runtime.saved.settings.insert("history:guide-admission".into(), json!(72));
    open_records(&mut app, &mut runtime);
    let reply = response(&runtime, vec![programme("Past", NOW - 600, NOW - 300, "Past")]);
    runtime.saved.settings.insert("history:guide-admission".into(), json!(24));
    assert!(guide_ui::accept(&mut runtime, &reply));
    assert!(rows(&runtime).is_empty());
    assert!(handle_action(&mut runtime, &mut app, OttAction::Refresh).is_err());
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn provider_records_load_one_day_keep_partial_rows_and_reject_old_page_receipts() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime)=records_fixture();
    let profile=&mut runtime.saved.profiles.profiles[0];
    profile.kind=ProviderKind::Xtream;
    profile.source=r#"{"server":"https://archive.invalid","username":"fixture","password":"fixture"}"#.into();
    runtime.provider=ProviderKind::Xtream;
    runtime.catalog.entries[0].media=MediaLocator::Xtream {stream_id:"42".into(),stream_kind:"live".into(),
        extension:"ts".into(),series_id:None,has_direct_source:false};
    let source=runtime.catalog.entries[0].live.as_mut().unwrap().catchup.as_mut().unwrap();
    source.mode=plx_data::ott::archive::CatchupMode::Provider;source.source="xtream".into();
    handle_action(&mut runtime,&mut app,OttAction::OpenRecords("channel".into())).unwrap();
    assert_eq!(runtime.metadata_query,Some(Query::Day{start:NOW-86400,end:NOW}));
    let first=response(&runtime,vec![programme("Newest",NOW-1000,NOW-900,"First day")]);
    assert!(guide_ui::accept(&mut runtime,&first));
    assert_eq!(rows(&runtime).len(),1);
    let more=runtime.view().rows.into_iter().find(|row|row.id=="records:more").unwrap().action;
    handle_action(&mut runtime,&mut app,more.clone()).unwrap();
    assert_eq!(runtime.metadata_query,Some(Query::Day{start:NOW-2*86400,end:NOW-86400}));
    assert_eq!(rows(&runtime).len(),1,"loading an older day preserves the accepted list");
    let second=response(&runtime,vec![programme("Older",NOW-100000,NOW-99000,"Second day")]);
    assert!(guide_ui::accept(&mut runtime,&second));
    assert_eq!(rows(&runtime).len(),2);
    assert!(handle_action(&mut runtime,&mut app,more).is_err(),"old addressed cursor must not enqueue another day");
    let generation=runtime.metadata_generation;
    assert!(guide_ui::accept(&mut runtime,&first));
    assert_eq!(runtime.metadata_generation,generation);
    assert_eq!(rows(&runtime).len(),2,"late previous-day replies cannot replace accumulated rows");
    let more=runtime.view().rows.into_iter().find(|row|row.id=="records:more").unwrap().action;
    handle_action(&mut runtime,&mut app,more).unwrap();
    assert!(runtime.view().status_working,"older-day loading uses the standard indicator");
    assert_eq!(rows(&runtime).len(),2,"accepted rows remain visible during loading");
    let cancelled_page=response(&runtime,vec![programme("Cancelled",NOW-200000,NOW-199000,"Late after Back")]);
    handle_action(&mut runtime,&mut app,OttAction::Back).unwrap();
    assert_eq!(runtime.page,"channels");
    assert!(runtime.guide.is_none());
    assert!(runtime.metadata_cancel.load(Ordering::Acquire));
    assert!(runtime.metadata_query.is_none() && runtime.metadata_next.is_none());
    assert!(guide_ui::accept(&mut runtime,&cancelled_page));
    assert!(runtime.guide.is_none(),"cancelled page cannot reopen or repopulate the guide");
    assert_eq!(runtime.page,"channels");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    handle_action(&mut runtime,&mut app,OttAction::OpenRecords("channel".into())).unwrap();
    let reopened=response(&runtime,vec![programme("Newest",NOW-1000,NOW-900,"Reopened")]);
    assert!(guide_ui::accept(&mut runtime,&reopened));
    runtime.saved.profiles.profiles[0].source=runtime.saved.profiles.profiles[0].source.replace("fixture","replacement");
    let more=runtime.view().rows.into_iter().find(|row|row.id=="records:more").unwrap().action;
    assert!(handle_action(&mut runtime,&mut app,more).is_err());
}

//! Coverage travels through the same metadata admission and authenticated query paths.
use super::*;
use plx_data::ott::guide::rows::{CoverageReason, CoverageReport, CoverageState, Outcome};
use std::collections::BTreeSet;

fn unavailable() -> CoverageReport {
    CoverageReport {
        state: CoverageState::Unavailable,
        reasons: BTreeSet::from([CoverageReason::IndexLimit]),
        requested: 3, matched: 0, current: 0,
        configured_sources: 1, attempted_sources: 1, loaded_sources: 1,
    }
}

fn pair() -> (App, ClockSample, Guide, Guide) {
    let (app, sample, mut accepted) = batch_fixture();
    // Keep this source revision unchanged so both independently captured replies
    // exercise the same catalogue owner rather than the stale-revision refusal.
    accepted.result.as_mut().unwrap().programmes[0].description = accepted.entry.description.clone();
    let mut failed = copy_guide(&accepted);
    let runtime = app.ott.as_ref().unwrap();
    let request = runtime.request_row_guide(&failed.source_profile, &failed.entry, failed.request.query).unwrap();
    failed.row_guide = Some(crate::app::ott_runtime::row_guide::Reply {
        request, outcome: Outcome { snapshot: None, report: unavailable() },
    });
    failed.result = Err("Fixture selected guide failure");
    (app, sample, accepted, failed)
}

#[test]
fn coverage_without_snapshot_is_admitted_and_keeps_fresh_earlier_rows() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, accepted, failed) = pair();
    deliver(&mut app, sample, accepted);
    deliver(&mut app, sample, failed);
    let runtime = app.ott.as_ref().unwrap();
    let diagnostic = runtime.row_guide_diagnostics();
    assert_eq!(diagnostic["coverage"]["state"], "unavailable");
    assert_eq!(diagnostic["coverage"]["reasons"], json!(["index-limit"]));
    assert_eq!(diagnostic["fresh"], true);
    assert!(runtime.current_row_programme(&runtime.catalog.entries[1]).is_some());
}

#[test]
fn partial_coverage_keeps_available_rows_and_original_selected_metadata() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = batch_fixture();
    let outcome = &mut reply.row_guide.as_mut().unwrap().outcome;
    outcome.snapshot.as_mut().unwrap().programmes.remove("third");
    outcome.report.state = CoverageState::Partial;
    outcome.report.reasons.insert(CoverageReason::Unmatched);
    outcome.report.matched = 2;
    outcome.report.current = 2;
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.raw_view();
    assert!(view.rows.iter().find(|row| row.id == "second").unwrap().current_programme.is_some());
    assert!(view.rows.iter().find(|row| row.id == "third").unwrap().current_programme.is_none());
    assert!(view.rows.iter().find(|row| row.id == "guide-channel").unwrap().guide.is_some());
    assert_eq!(runtime.row_guide_diagnostics()["coverage"]["state"], "partial");
}

#[test]
fn cancelled_coverage_cannot_replace_current_diagnostics() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, accepted, failed) = pair();
    deliver(&mut app, sample, accepted);
    let before = app.ott.as_ref().unwrap().row_guide_diagnostics();
    app.ott.as_ref().unwrap().metadata_cancel.store(true, Ordering::Release);
    deliver(&mut app, sample, failed);
    assert_eq!(app.ott.as_ref().unwrap().row_guide_diagnostics(), before);
}

#[test]
fn coverage_counts_and_source_binding_are_checked_before_diagnostic_publication() {
    let _serial = plx_base::testlock::serial();
    for change in 0..11 {
        let (mut app, sample, mut reply) = batch_fixture();
        let report = &mut reply.row_guide.as_mut().unwrap().outcome.report;
        match change {
            0 => report.requested = 50_001,
            1 => report.requested = 4,
            2 => report.matched = 4,
            3 => report.current = 4,
            4 => report.configured_sources = 50_001,
            5 => { report.configured_sources = 17; report.attempted_sources = 17; }
            6 => report.attempted_sources = 2,
            7 => report.loaded_sources = 2,
            8 => app.ott.as_mut().unwrap().generation += 1,
            9 => app.ott.as_mut().unwrap().catalog_revision += 1,
            _ => app.ott.as_mut().unwrap().saved.profiles.profiles[0].source = "https://other.invalid/changed.m3u".into(),
        }
        deliver(&mut app, sample, reply);
        assert!(app.ott.as_ref().unwrap().row_guide_diagnostics().is_null(), "change {change}");
    }
}

#[test]
fn malformed_snapshot_does_not_discard_a_valid_coverage_report() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, mut reply) = batch_fixture();
    reply.row_guide.as_mut().unwrap().outcome.snapshot.as_mut().unwrap().next_transition_s = None;
    deliver(&mut app, sample, reply);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.row_guide_diagnostics()["coverage"]["state"], "complete");
    assert_eq!(runtime.row_guide_diagnostics()["fresh"], false);
    assert!(runtime.raw_view().rows.iter().all(|row| row.current_programme.is_none()));
}

#[test]
fn authenticated_state_and_status_expose_only_closed_row_coverage() {
    let _serial = plx_base::testlock::serial();
    let (mut app, sample, reply) = batch_fixture();
    deliver(&mut app, sample, reply);
    for (index, action) in ["state", "status"].into_iter().enumerate() {
        let mut runtime = app.ott.take().unwrap();
        let body = json!({"commands":[],"server_time":1000,"requests":[{
            "id":format!("{:032x}", index + 1), "action":action, "params":{}, "expires_at":1020
        }]}).to_string();
        let now = runtime.now_ms();
        let mut requests = runtime.remote.raw_poll_for_test(&body, now);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert!(runtime.remote.invocation_is_current(&request));
        let reply = commands::command(&mut runtime, &mut app, &request).unwrap();
        assert_eq!(reply["status"], "ok");
        let diagnostic = &reply["data"]["row_guide"];
        assert_eq!(diagnostic["coverage"]["requested"], 3);
        assert_eq!(diagnostic["fresh"], true);
        let encoded = diagnostic.to_string();
        for private in ["https://", "Programme for", "guide-profile", "Fixture"] {
            assert!(!encoded.contains(private));
        }
        app.ott = Some(runtime);
    }
}

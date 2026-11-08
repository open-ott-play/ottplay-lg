//! Regression contract uses only baseline actions: the new controls are found
//! through the same published rows as the renderer. No provider/storage IO.
use super::*;

fn programme(title: &str, start: i64) -> Programme {
    Programme {
        channel: "channel".into(),
        start,
        end: start + 300,
        title: title.into(),
        description: format!("Description of {title} at {start}"),
    }
}

fn deliver(runtime: &mut Runtime, programmes: Vec<Programme>) -> Guide {
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
            programmes,
        }),
    };
    guide_ui::accept(runtime, &reply);
    reply
}

fn opened() -> (Box<App>, Runtime) {
    let (mut app, mut runtime) = fixture();
    handle_action(
        &mut runtime,
        &mut app,
        OttAction::OpenGuide("channel".into()),
    )
    .unwrap();
    deliver(
        &mut runtime,
        vec![
            programme("Zulu", 1_699_998_000),
            programme("Alpha", 1_699_998_600),
            programme("Middle", 1_700_000_600),
        ],
    );
    (app, runtime)
}

fn control(runtime: &Runtime, id: &str) -> OttAction {
    runtime
        .view()
        .rows
        .into_iter()
        .find(|row| row.id == id && row.enabled)
        .unwrap_or_else(|| panic!("guide must expose enabled order control {id}"))
        .action
}

fn programmes(runtime: &Runtime) -> Vec<OttRow> {
    runtime
        .view()
        .rows
        .into_iter()
        .filter(|row| matches!(row.action, OttAction::GuideProgramme { .. }))
        .collect()
}

fn titles(runtime: &Runtime) -> Vec<String> {
    programmes(runtime)
        .into_iter()
        .map(|row| row.title)
        .collect()
}

fn alphabetical(runtime: &Runtime) -> OttAction {
    control(runtime, "guide:order:alphabetical")
}
fn chronological(runtime: &Runtime) -> OttAction {
    control(runtime, "guide:order:chronological")
}

#[test]
fn guide_order_switch_preserves_programme_identity_owner_and_worker() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = opened();
    let alpha = alphabetical(&runtime);
    let before = programmes(&runtime);
    let selected = before[1].clone();
    handle_action(&mut runtime, &mut app, selected.action.clone()).unwrap();
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(
        runtime.view().selected_row.as_deref(),
        Some(selected.id.as_str())
    );
    let epochs = (
        runtime.generation,
        runtime.playback_epoch,
        runtime.navigation_epoch,
        runtime.metadata_generation,
        runtime.play_generation,
    );
    let target = runtime.metadata_target.clone();
    let query = runtime.metadata_query;
    let cancel = runtime.metadata_cancel.clone();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    handle_action(&mut runtime, &mut app, alpha).unwrap();
    assert_eq!(titles(&runtime), ["Alpha", "Middle", "Zulu"]);
    assert_eq!(
        runtime.view().selected_row.as_deref(),
        Some(selected.id.as_str())
    );
    for row in programmes(&runtime) {
        let original = before
            .iter()
            .find(|old| old.id == row.id)
            .expect("same programme IDs");
        assert_eq!(row.action, original.action);
        assert_eq!(row.description, original.description);
    }
    let time = chronological(&runtime);
    handle_action(&mut runtime, &mut app, time).unwrap();
    assert_eq!(titles(&runtime), ["Zulu", "Alpha", "Middle"]);
    assert_eq!(
        runtime.view().selected_row.as_deref(),
        Some(selected.id.as_str())
    );
    assert_eq!(
        (
            runtime.generation,
            runtime.playback_epoch,
            runtime.navigation_epoch,
            runtime.metadata_generation,
            runtime.play_generation
        ),
        epochs
    );
    assert_eq!(runtime.metadata_target, target);
    assert_eq!(runtime.metadata_query, query);
    assert!(Arc::ptr_eq(&runtime.metadata_cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!runtime.saving);
}

#[test]
fn guide_order_matches_classic_utf16_title_then_time_without_dropping_repeats() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = opened();
    let alpha = alphabetical(&runtime);
    let input = vec![
        programme("\u{e000}", 1_699_998_000),
        programme("\u{10000}", 1_699_998_300),
        programme("Repeat", 1_699_998_900),
        programme("Repeat", 1_699_998_600),
    ];
    let reply = deliver(&mut runtime, input.clone());
    let original = programmes(&runtime);
    handle_action(&mut runtime, &mut app, alpha).unwrap();
    let sorted = programmes(&runtime);
    assert_eq!(
        titles(&runtime),
        ["Repeat", "Repeat", "\u{10000}", "\u{e000}"]
    );
    let earlier = original
        .iter()
        .find(|row| row.description.ends_with("1699998600"))
        .unwrap();
    assert_eq!(sorted[0].id, earlier.id);
    assert_eq!(
        reply.result.as_ref().unwrap().programmes,
        input,
        "producer publication is not mutated"
    );
    let next = vec![
        programme("Zulu", 1_700_000_600),
        programme("Alpha", 1_700_000_900),
    ];
    deliver(&mut runtime, next);
    assert_eq!(
        titles(&runtime),
        ["Alpha", "Zulu"],
        "accepted refresh retains selected mode"
    );
}

#[test]
fn guide_order_rejects_stale_source_request_detail_page_and_strict_policy() {
    let _serial = plx_base::testlock::serial();
    for case in 0..4 {
        let (mut app, mut runtime) = opened();
        let action = alphabetical(&runtime);
        match case {
            0 => {
                runtime.generation += 1;
            }
            1 => {
                handle_action(&mut runtime, &mut app, OttAction::Refresh).unwrap();
            }
            2 => {
                let programme = programmes(&runtime)[0].action.clone();
                handle_action(&mut runtime, &mut app, programme).unwrap();
            }
            _ => {
                pin(&mut runtime, &app, true);
            }
        }
        let before = titles(&runtime);
        reject_guide_without_replacing_metadata(&mut runtime, &mut app, action);
        assert_eq!(titles(&runtime), before);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(!runtime.saving);
    }
}

#[test]
fn guide_order_keeps_archive_and_reminder_actions_bound_to_original_programmes() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = opened();
    let alpha = alphabetical(&runtime);
    let rows = programmes(&runtime);
    let future = rows[2].clone();
    handle_action(&mut runtime, &mut app, future.action.clone()).unwrap();
    let reminder = control(&runtime, "guide:reminder");
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    handle_action(&mut runtime, &mut app, alpha).unwrap();
    handle_action(&mut runtime, &mut app, future.action).unwrap();
    assert_eq!(control(&runtime, "guide:reminder"), reminder);
    assert!(
        matches!(reminder, OttAction::ToggleReminder { programme_id, .. } if programme_id == future.id)
    );
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    let archived = rows[0].clone();
    handle_action(&mut runtime, &mut app, archived.action).unwrap();
    let archive = control(&runtime, "guide:archive");
    handle_action(&mut runtime, &mut app, archive).unwrap();
    let request = runtime
        .play_target
        .as_ref()
        .expect("archive intent uses existing resolver");
    assert_eq!(request.id, "channel");
    let programme = request.archive.as_ref().unwrap().programme().unwrap();
    assert_eq!(programme.title, "Zulu");
    assert_eq!(programme.start, 1_699_998_000);
    assert_eq!(programme.end, 1_699_998_300);
    assert!(!request.advance_on_failure);
    assert!(runtime.resolver_next.as_ref().unwrap().archive.is_some());
}

#[test]
fn guide_order_dispatch_is_presentation_only_not_source_navigation() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = opened();
    // Match the established UI harness: all worker slots remain occupied.
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    app.ott = Some(runtime);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let action = alphabetical(runtime);
    let epochs = (
        runtime.generation,
        runtime.navigation_epoch,
        runtime.playback_epoch,
        runtime.metadata_generation,
    );
    let target = runtime.metadata_target.clone();
    let cancel = runtime.metadata_cancel.clone();
    let reply = app.pages.top_page().expect("mounted OTT page");
    app.pages.emit(
        MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(titles(runtime), ["Alpha", "Middle", "Zulu"]);
    assert_eq!(runtime.page, "guide");
    assert_eq!(
        (
            runtime.generation,
            runtime.navigation_epoch,
            runtime.playback_epoch,
            runtime.metadata_generation
        ),
        epochs
    );
    assert_eq!(runtime.metadata_target, target);
    assert!(Arc::ptr_eq(&runtime.metadata_cancel, &cancel));
    assert!(!runtime.saving);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn guide_order_roundtrip_restores_original_equal_start_order() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = opened();
    let alpha = alphabetical(&runtime);
    let zulu = programme("Zulu", 1_699_998_600);
    let mut alpha_same_start = programme("Alpha", zulu.start);
    alpha_same_start.end += 300; // distinct broadcasts with overlapping starts
    deliver(
        &mut runtime,
        vec![zulu, alpha_same_start, programme("Middle", 1_699_998_000)],
    );
    let original_ids: Vec<_> = programmes(&runtime)
        .iter()
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(titles(&runtime), ["Middle", "Zulu", "Alpha"]);
    for _ in 0..2 {
        handle_action(&mut runtime, &mut app, alpha.clone()).unwrap();
        assert_eq!(titles(&runtime), ["Alpha", "Middle", "Zulu"]);
        let time = chronological(&runtime);
        handle_action(&mut runtime, &mut app, time).unwrap();
        assert_eq!(titles(&runtime), ["Middle", "Zulu", "Alpha"]);
        let ids: Vec<_> = programmes(&runtime)
            .iter()
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(ids, original_ids);
    }
}

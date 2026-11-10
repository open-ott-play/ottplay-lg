//! Actual worker-event -> Runtime -> addressed snapshot publication regressions.
//! Synthetic catalogues only; all provider, resolver and metadata worker slots stay held.
use super::*;
use plx_data::ott::{CategoryHierarchy, CategoryMembership, CategoryNode};
use plx_machine::machine::{Delivery, InstanceId, Stamped, UiMode};
use plx_screens::registry::AppMsg;
use plx_ui::screen::ScreenEvent;

#[derive(Default)]
struct SnapshotTap(Vec<(InstanceId, Arc<OttView>)>);
impl plx_ui::dispatch::Tap<bridge::AppHost> for SnapshotTap {
    fn effect(&mut self, _: u64, stamped: &Stamped<bridge::AppHost>) {
        if let Fx::Deliver(MachineId::Instance(owner), Delivery::Screen(
            ScreenEvent::App(AppMsg::OttSnapshot(view)))) = &stamped.fx {
            self.0.push((*owner, view.clone()));
        }
    }
}
fn drain(app: &mut App) -> Vec<(InstanceId, Arc<OttView>)> {
    let tick = next_tick(app);
    let mut tap = SnapshotTap::default();
    app.pages.frame_with(&mut app.bridge, tick, Vec::new(), Vec::new(), &mut tap, false);
    tap.0
}
fn catalog(count: usize) -> Catalog {
    let items: Vec<_> = (0..count).map(|n| json!({"type":"stream", "id":n.to_string(),
        "title":format!("Video {n}"), "request":{"id":n.to_string()}})).collect();
    plx_data::ott::parse_vportal_response(&json!({"type":"videoportal","items":items}).to_string(),
        &synthetic_profile(), "").unwrap().catalog
}
fn node(id: &str) -> Catalog {
    Catalog { entries: Vec::new(), hierarchy: Some(CategoryHierarchy {
        nodes: vec![CategoryNode { id:id.into(), parent:None, label:id.into(),
            anonymous_fingerprint:None, source_identity:None }], memberships:Vec::new(),
    }) }
}
fn mounted(count: usize) -> App {
    let mut app = ready_app_fixture();
    app.running = true;
    app.bridge.apply_ui_mode(UiMode::Classic);
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("interfaceMode".into(), json!(0));
    r.saved.profiles.profiles = vec![synthetic_profile()];
    r.saved.profiles.active_profile = Some("portal-fixture".into());
    r.catalog = catalog(count);
    r.catalog_loading = true;
    r.catalog_complete = false;
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    r.plex_refresh_pending = true;
    r.write_permit.revoke();
    r.continuity.stop(r.generation);
    r.page = "channels";
    r.status = format!("{count} videos");
    r.sync_saved_context();
    r.changed();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    // Consume the normal mount/preview messages before the measured worker event.
    for _ in 0..4 { runtime_tick(&mut app); drain(&mut app); settle_pages(&mut app); }
    let r = app.ott.as_mut().unwrap();
    r.status = format!("{count} videos");
    let before = r.revision;
    runtime_tick(&mut app);
    assert!(drain(&mut app).is_empty(), "fixture must be stable before the worker event");
    assert_eq!(app.ott.as_ref().unwrap().revision, before);
    app
}
fn send(app: &App, batch: Catalog) {
    let r = app.ott.as_ref().unwrap();
    r.catalog_tx.send(CatalogEvent::Batch(r.generation, batch)).unwrap();
}
fn tick(app: &mut App) -> Vec<(InstanceId, Arc<OttView>)> {
    runtime_tick(app);
    drain(app)
}
fn ui(app: &mut App, action: OttAction) {
    let reply = app.ott.as_ref().unwrap().reply.unwrap();
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    drain(app);
    runtime_tick(app);
    drain(app);
    settle_pages(app);
}
fn categories(app: &mut App, serial: u64, cursor: Option<Value>) -> Value {
    let mut r = app.ott.take().unwrap();
    let mut params = json!({"op":"list", "runtime":r.remote.runtime(), "offset":0, "limit":20});
    if let Some(cursor) = cursor { params["catalog"] = cursor; }
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"expires_at":1020,"action":"categories","params":params
    }]}).to_string();
    let now = r.now_ms();
    let inv = r.remote.raw_poll_for_test(&body, now).pop().unwrap();
    let response = commands::command(&mut r, app, &inv).unwrap();
    app.ott = Some(r);
    response
}

#[test]
fn nodes_only_with_twenty_thousand_rows_keeps_view_and_emits_no_full_snapshot() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(20_000));
    let before = app.ott.as_ref().unwrap().view();
    let (revision, generation) = { let r=app.ott.as_ref().unwrap(); (r.catalog_revision,r.generation) };
    send(&app, node("new-folder"));
    assert!(tick(&mut app).is_empty(), "nodes-only update must not emit an addressed OttSnapshot");
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.catalog_revision, revision + 1, "new read snapshots must observe the catalogue revision");
    assert_eq!(r.generation, generation);
    assert!(r.catalog.hierarchy.as_ref().unwrap().nodes.iter().any(|n| n.id == "new-folder"));
    assert_eq!(r.view(), before, "presentation revision, rows, focus and action contexts are unchanged");
    assert!(tick(&mut app).is_empty(), "no deferred publication or timer was introduced");
}

#[test]
fn modern_all_publishes_category_node_changes_for_shelf_projection() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(3));
    app.bridge.apply_ui_mode(UiMode::Modern);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    runtime.changed();
    tick(&mut app);
    assert!(tick(&mut app).is_empty(), "presentation switch must settle first");
    send(&app, node("new-folder"));
    assert_eq!(tick(&mut app).len(), 1, "Modern category shape must be republished");
    assert!(tick(&mut app).is_empty(), "one source update must not cause continuous publication");
}

#[test]
fn nodes_only_preserves_catalog_receipt_and_rendered_numeric_and_preview_admission() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(3));
    let response = categories(&mut app, 1, None);
    assert_eq!(response["status"], "ok");
    let cursor = response["data"]["catalog"].clone();
    // The raw poll also changes remote status to connected. Publish that setup
    // change separately so the worker assertion below measures only its batch.
    let remote = app.ott.as_ref().unwrap().remote.snapshot();
    assert_eq!(remote["state"], "connected");
    assert_ne!(app.ott.as_ref().unwrap().remote_snapshot, remote);
    assert_eq!(tick(&mut app).len(), 1, "the remote status change must publish");
    assert_eq!(app.ott.as_ref().unwrap().remote_snapshot, remote);
    assert!(tick(&mut app).is_empty(), "remote setup must settle before the batch");
    let view = app.ott.as_ref().unwrap().view();
    send(&app, node("new-folder"));
    assert!(tick(&mut app).is_empty());
    let retained = categories(&mut app, 2, Some(cursor));
    assert_eq!(retained["status"], "ok");
    assert_eq!(retained["data"]["categories"], response["data"]["categories"]);
    assert_eq!(retained["data"]["total"], response["data"]["total"]);
    let r = app.ott.as_ref().unwrap();
    let id = r.catalog.entries[0].id.clone();
    assert!(preview::resolve(r, OttAction::PreviewAt {
        context:view.preview_context.unwrap(), id,
    }).is_ok());
    assert_eq!(r.view().random_context, view.random_context);
    app.ev = crate::app::events::encode_key(u32::from(b'2'), 0, true);
    assert!(crate::app::ott_runtime::numeric_input::ingest(&mut app), "unchanged rendered revision still admits numeric input");
    app.ev = crate::app::events::encode_key(u32::from(b'2'), 0, false);
    assert!(crate::app::ott_runtime::numeric_input::ingest(&mut app));
}

#[test]
fn nodes_only_does_not_hide_named_missing_category_editor_or_other_page_changes() {
    let _lock = plx_base::testlock::serial();
    for case in 0..6 {
        let mut app = Box::new(mounted(3));
        let r = app.ott.as_mut().unwrap();
        match case {
            0 => r.saved.category = "not-yet-discovered".into(),
            1 => r.page = "categories",
            2 => r.saved.favorites_selected = true,
            3 => { r.saved.settings.insert(catalog_organization::KEY.into(), json!({"version":1,"profiles":{}})); },
            4 => r.status = "Existing provider warning".into(),
            _ => r.edit_session = Some(EditorSession { id:1, owner:r.reply.unwrap(), key:"search".into(),
                purpose:search::EditorPurpose::Setting }),
        }
        send(&app, node("new-folder"));
        assert_eq!(tick(&mut app).len(), 1, "case {case} must publish normally");
    }
}

#[test]
fn first_rows_replacements_memberships_terminal_and_failed_merge_publish_immediately() {
    let _lock = plx_base::testlock::serial();
    for case in 0..6 {
        let mut app = Box::new(mounted(if case == 0 { 0 } else { 3 }));
        let mut batch = node("new-folder");
        match case {
            0 => batch.entries = catalog(1).entries,
            1 => { let mut entry=app.ott.as_ref().unwrap().catalog.entries[0].clone();
                entry.description="Updated metadata".into(); batch.entries.push(entry); },
            2 => batch.hierarchy.as_mut().unwrap().memberships.push(CategoryMembership {
                entry_id:app.ott.as_ref().unwrap().catalog.entries[0].id.clone(), category_id:"new-folder".into(),
            }),
            5 => batch.hierarchy.as_mut().unwrap().nodes[0].parent=Some("missing-parent".into()),
            _ => {},
        }
        send(&app, batch);
        if case == 3 || case == 4 {
            let r=app.ott.as_ref().unwrap();
            r.catalog_tx.send(CatalogEvent::Finished(r.generation,
                if case == 3 { Ok(()) } else { Err("Provider failed") })).unwrap();
        }
        assert_eq!(tick(&mut app).len(), 1, "case {case} must not elide publication");
    }
}

#[test]
fn nodes_only_does_not_suppress_a_same_tick_ui_refresh() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(3));
    let owner = app.ott.as_ref().unwrap().reply.unwrap();
    app.pages.emit(MachineId::Instance(owner), Fx::App(AppFx::Ott(OttOut::Refresh { reply:owner })));
    drain(&mut app);
    send(&app, node("new-folder"));
    assert_eq!(tick(&mut app).len(), 1);
}

#[test]
fn nodes_only_with_committed_search_and_selected_metadata_keeps_normal_publication() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(3));
    ui(&mut app, OttAction::BeginSearch);
    let session = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    ui(&mut app, OttAction::SubmitSearch { session, value:"Video".into() });
    assert_eq!(app.ott.as_ref().unwrap().search_view().unwrap().query, "Video");
    send(&app, node("search-folder"));
    assert_eq!(tick(&mut app).len(), 1);

    let mut app = Box::new(mounted(3));
    let r = app.ott.as_mut().unwrap();
    let id = r.catalog.entries[0].id.clone();
    r.metadata_target = Some(("portal-fixture".into(), id.clone()));
    r.metadata_tx.send(crate::app::ott_metadata::Event::Preview(crate::app::ott_metadata::Preview {
        generation:r.metadata_generation, profile_id:"portal-fixture".into(), entry_id:id,
        entry:None, result:Ok(json!({"description":"New selected metadata"})),
    })).unwrap();
    send(&app, node("metadata-folder"));
    assert_eq!(tick(&mut app).len(), 1);
    assert_eq!(app.ott.as_ref().unwrap().catalog.entries[0].description, "New selected metadata");
}

#[test]
fn nodes_only_proof_rejects_source_owner_navigation_and_status_replacement() {
    let _lock = plx_base::testlock::serial();
    for case in 0..5 {
        let mut app = Box::new(mounted(3));
        let r = app.ott.as_ref().unwrap();
        let proof = super::super::catalog_publication::UnchangedAll::capture(r, &app, &node("new"), r.revision).unwrap();
        let r = app.ott.as_mut().unwrap();
        match case {
            0 => r.generation += 1,
            1 => r.navigation_epoch += 1,
            2 => r.saved.profiles.profiles[0].source = "portal::[key:changed]https://other.invalid/api".into(),
            3 => r.reply = None,
            _ => r.status = "New failure".into(),
        }
        assert!(!proof.current(app.ott.as_ref().unwrap(), &app), "case {case}");
    }
}


// Draw the actual already-mounted screen using the same current host views,
// measurement and focus. Only the painter is recording; no alternate Form or
// Table is built by this test, and no GL frame clear is permitted.
fn painted_catalog(app: &mut App) -> Vec<plx_ui::draw_census::Run> {
    let owner = bridge::ott_ui_target(&app.pages).expect("mounted catalogue owner");
    let parts = app.pages.parts(Tick { ms: app.loop_t, dt_us: 0 });
    let split = plx_ui::dispatch::Rig::<bridge::AppHost>::split(&mut app.bridge);
    let cx = parts.cx::<bridge::AppHost>(split.views, split.measure);
    let body = app.pages.nav.instance_mut(owner).unwrap();
    assert_eq!(body.screen.name(), "ott");
    plx_ui::draw_census::capture_runs(|| {
        let mut frame = plx_ui::screen::DrawFrame::new(&cx, plx_ui::Painter::recording());
        plx_gfx::gfx::without_frame_clear(|| body.screen.draw(&mut frame));
    }).into_iter().filter(|run| run.ink[3] > 0.0).collect()
}

fn settled_catalog_paint(app: &mut App) -> Vec<plx_ui::draw_census::Run> {
    let mut previous = painted_catalog(app);
    for _ in 0..128 {
        drain(app); // Real screen ticks, never a direct assignment to a spring.
        let current = painted_catalog(app);
        if current == previous { return current; }
        previous = current;
    }
    panic!("catalogue text positions did not settle in the bounded fixture");
}

struct CatalogTypographyRestore(plx_machine::machine::FontScale);
impl Drop for CatalogTypographyRestore {
    fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
}

#[test]
fn warmed_mounted_table_replaces_same_count_rows_and_rejects_late_generation_payload() {
    let _lock = plx_base::testlock::serial();
    let _typography = CatalogTypographyRestore(plx_ui::typography::current());
    let mut app = Box::new(mounted(40));
    app.bridge.apply_ui_font_scale(100).unwrap();
    // Real focus movement, well beyond the first page. This must remain a
    // scrolled list across publication; a reset to row zero cannot pass.
    for _ in 0..30 {
        let at = next_tick(&mut app);
        bridge::frame(&mut app.pages, &mut app.bridge, at,
            bridge::script_key(plx_machine::machine::Key::Down, at));
        runtime_tick(&mut app);
        drain(&mut app);
    }
    settle_pages(&mut app);
    runtime_tick(&mut app);
    drain(&mut app);
    let owner = bridge::ott_ui_target(&app.pages).unwrap();
    let focus = app.pages.focus();
    assert!(focus.is_some());
    let (generation, selected, mut detail, mut list_row, list_index) = {
        let r = app.ott.as_ref().unwrap();
        let selected = r.view().selected_row.expect("actual focused catalogue row");
        let index = r.catalog.entries.iter().position(|entry| entry.id == selected).unwrap();
        assert_eq!(index, 30, "thirty real Down presses moved beyond the first visible page");
        (r.generation, selected, r.catalog.entries[index].clone(), r.catalog.entries[index + 1].clone(), index + 1)
    };
    let old_title = list_row.title.clone();
    // The first paint warms the real Table's derived certificate/layout state.
    painted_catalog(&mut app);
    let before = settled_catalog_paint(&mut app);
    let old_runs: Vec<_> = before.iter().filter(|run| run.text == old_title).collect();
    assert_eq!(old_runs.len(), 1, "the unselected following row is painted by the list only");
    let old_at = (old_runs[0].at.x, old_runs[0].at.y);
    let spacing = |runs: &[plx_ui::draw_census::Run]| {
        runs.iter().find(|run| run.text == "Video 28").unwrap().at.y
            - runs.iter().find(|run| run.text == "Video 27").unwrap().at.y
    };
    let old_spacing = spacing(&before);

    send(&app, node("same-count-folder"));
    assert!(tick(&mut app).is_empty());
    let retained = painted_catalog(&mut app);
    let retained_row = retained.iter().find(|run| run.text == old_title).unwrap();
    assert_eq!((retained_row.at.x, retained_row.at.y), old_at);
    assert_eq!(app.pages.focus(), focus);

    // VPortal receives late title/synopsis facts in catalogue batches. It does
    // not use request_metadata's M3U/Xtream/Stalker Preview service: no invented
    // metadata_target or synthetic successful persistence is needed here.
    detail.description = "LATE_SYNOPSIS".into();
    list_row.title = "Replaced item".into();
    let detail_id = detail.id.clone();
    let list_id = list_row.id.clone();
    let list_locator = list_row.media.clone();
    send(&app, Catalog { entries: vec![detail.clone(), list_row.clone()], hierarchy: None });
    let published = tick(&mut app);
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, owner);
    assert_eq!(published[0].1.rows.len(), 40);
    let row = published[0].1.rows.iter().find(|row| row.id == list_id).unwrap();
    assert_eq!(row.content_title.as_deref(), Some("Replaced item"));
    assert_eq!(published[0].1.rows.iter().find(|row| row.id == detail_id).unwrap().description, "LATE_SYNOPSIS");
    let after = painted_catalog(&mut app);
    let changed_row = after.iter().find(|run| run.text == "Replaced item").expect("new list payload painted");
    assert_eq!((changed_row.at.x, changed_row.at.y), old_at, "same-count replacement keeps the visible row position");
    assert!(!after.iter().any(|run| run.text == old_title), "old cached label must disappear");
    assert!(after.iter().any(|run| run.text == "LATE_SYNOPSIS"), "selected late catalogue metadata is painted");
    assert_eq!(bridge::ott_ui_target(&app.pages), Some(owner));
    assert_eq!(app.pages.focus(), focus);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.catalog.entries.len(), 40);
    assert_eq!(r.catalog.entries[list_index].media, list_locator);
    assert_eq!(r.view().selected_row.as_deref(), Some(selected.as_str()));
    let (revision, catalog_revision) = (r.revision, r.catalog_revision);

    detail.description = "STALE_SYNOPSIS".into();
    list_row.title = "STALE_REPLACEMENT".into();
    r.catalog_tx.send(CatalogEvent::Batch(generation.wrapping_sub(1),
        Catalog { entries: vec![detail, list_row], hierarchy: None })).unwrap();
    assert!(tick(&mut app).is_empty(), "stale worker result must not publish");
    let rejected = painted_catalog(&mut app);
    assert!(rejected.iter().any(|run| run.text == "Replaced item"));
    assert!(rejected.iter().any(|run| run.text == "LATE_SYNOPSIS"));
    assert!(!rejected.iter().any(|run| run.text.starts_with("STALE_")));
    let r = app.ott.as_ref().unwrap();
    assert_eq!((r.revision, r.catalog_revision), (revision, catalog_revision));
    assert_eq!(r.view().selected_row.as_deref(), Some(selected.as_str()));
    assert_eq!(app.pages.focus(), focus);

    // A real host font change must relayout the retained scrolled Table even
    // without a new snapshot; another nodes-only admission must keep that result.
    let old_size = rejected.iter().find(|run| run.text == "Replaced item").unwrap().size;
    app.bridge.apply_ui_font_scale(200).unwrap();
    settle_pages(&mut app);
    assert!(tick(&mut app).is_empty());
    let scaled = settled_catalog_paint(&mut app);
    let scaled_row = scaled.iter().find(|run| run.text == "Replaced item")
        .expect("retained scrolled row after font change");
    assert!(scaled_row.size > old_size, "real recorded text size must respond to the new measure");
    assert_ne!(spacing(&scaled), old_spacing, "the retained table relayouts its rows for the new measure");
    let scaled_at = (scaled_row.at.x, scaled_row.at.y, scaled_row.size);
    assert!(scaled.iter().any(|run| run.text == "LATE_SYNOPSIS"));
    assert_eq!(app.pages.focus(), focus);
    send(&app, node("scaled-folder"));
    assert!(tick(&mut app).is_empty());
    let kept = painted_catalog(&mut app);
    let kept_row = kept.iter().find(|run| run.text == "Replaced item").unwrap();
    assert_eq!((kept_row.at.x, kept_row.at.y, kept_row.size), scaled_at);
    assert_eq!(app.pages.focus(), focus);
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some(selected.as_str()));
}


// Append to catalog_publication.rs: reuse its actual mounted/send/tick fixture.
// The held resolver slot allows ordinary Play admission without provider/decoder IO.
fn numeric_ingress(app: &mut App, down: bool) {
    app.ev = crate::app::events::encode_key(u32::from(b'2'), 0, down);
    let mut frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: clock::now(), dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "ott",
    };
    unsafe {
        run::ingest_sdl_event_with_window(app, &mut frame,
            |_| panic!("numeric input must not restore a window"));
    }
}

#[test]
fn fresh_numeric_after_nodes_only_batch_uses_retained_screen_order_and_source() {
    let _lock = plx_base::testlock::serial();
    let mut app = Box::new(mounted(3));
    let owner = bridge::ott_ui_target(&app.pages).expect("mounted catalogue owner");
    let focus = app.pages.focus();
    assert!(focus.is_some());
    let (view, catalog_revision, wanted, profile) = {
        let runtime = app.ott.as_ref().unwrap();
        (runtime.view(), runtime.catalog_revision, runtime.catalog.entries[1].clone(),
            runtime.effective_profile().unwrap().clone())
    };
    assert_eq!(Some(view.page_id.clone()), app.ott.as_ref().unwrap().search_page_id(),
        "numeric input uses the published source-bound search context even with an empty query");
    let rendered_ids: Vec<_> = view.rows.iter().filter_map(|row| match &row.action {
        OttAction::Play(id) => Some(id.clone()),
        _ => None,
    }).collect();
    assert_eq!(rendered_ids.len(), 3);
    assert_eq!(rendered_ids[1], wanted.id);
    assert!(matches!(&wanted.media, MediaLocator::VPortal { .. }));
    assert!(app.ott.as_ref().unwrap().organization_projection().is_some(),
        "exercise the shared checked category projection, not its raw-list fallback");
    assert!(app.inputs.is_empty());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());

    send(&app, node("numeric-new-folder"));
    assert!(tick(&mut app).is_empty(), "the worker event must retain the mounted snapshot");
    assert_eq!(bridge::ott_ui_target(&app.pages), Some(owner));
    assert_eq!(app.pages.focus(), focus, "nonvisual admission must retain actual form focus");
    {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog_revision, catalog_revision + 1);
        assert_eq!(runtime.view(), view);
        let screen = app.pages.nav.instance(owner).unwrap().screen.as_any().unwrap()
            .downcast_ref::<plx_screens::ott::OttScreen>().unwrap();
        assert!(screen.accepts_channel_number());
        assert!(screen.channel_number_uses_list_order());
        assert!(screen.channel_number_context(&view.page_id, view.revision),
            "the numeric authority is the retained real screen, not a synthetic row map");
    }

    // Start a fresh number only after the nonvisual batch; cancellation of a
    // number already pending before a catalogue revision is a separate policy.
    let sample = app.ott.as_ref().unwrap().clock_sample.unwrap();
    let navigation = app.ott.as_ref().unwrap().navigation_epoch;
    numeric_ingress(&mut app, true);
    numeric_ingress(&mut app, false);
    assert!(app.inputs.is_empty(), "both edges must be consumed by the real numeric owner");
    super::super::tick(&mut app, sample.advance_for_test(Duration::from_millis(1_999)));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    super::super::tick(&mut app, sample.advance_for_test(Duration::from_millis(2_000)));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.navigation_epoch, navigation + 1);
    assert_eq!(runtime.selected.as_deref(), Some(wanted.id.as_str()));
    let target = runtime.play_target.as_ref().expect("ordinary numeric Play admission");
    assert_eq!(target.id, wanted.id);
    assert!(!target.advance_on_failure, "numeric selection owns the exact row; a failure is not EOF");
    assert_eq!(runtime.view().pending_play.as_deref(), Some(wanted.id.as_str()),
        "numeric selection exposes the same source-bound pending handoff as OK");
    let queued = runtime.resolver_next.as_ref().expect("held actual resolver request");
    assert_eq!(queued.profile, profile);
    assert_eq!(queued.entry.as_ref(), Some(&wanted));
    assert!(queued.local_read.is_none() && queued.plex.is_none() && queued.favorite.is_none());
    assert!(runtime.playing.is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
    let epoch = runtime.playback_epoch;
    super::super::tick(&mut app, sample.advance_for_test(Duration::from_millis(9_000)));
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, epoch, "deadline commits once");
    assert_eq!(app.ott.as_ref().unwrap().resolver_next.as_ref().unwrap().entry.as_ref(), Some(&wanted));
}

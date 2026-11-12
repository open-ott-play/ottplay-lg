//! Exercise the real native adapter, Runtime and shared OttView/OttAction path.
//! The inert existing supervisor deliberately starts no helper or transport.
use super::*;
use crate::app::ott_companion::protocol::{Category, IndexerChild, SearchItem, SearchPage};

fn fixture() -> (Runtime, App, Adapter) {
    let app = crate::app::run::lifecycle_regression_tests::app();
    let mut runtime = Runtime::unloaded(Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.loaded = true;
    let worker = plx_base::task::OffFrame::for_test();
    runtime.clock_sample = Some(ClockSample::capture(&worker).unwrap());
    let mut adapter = Adapter::default();
    adapter.applied = Some(Configuration {
        enabled: true,
        ..Configuration::default()
    });
    adapter.source = "home".into();
    adapter.sources = vec![IndexerSource {
        id: "home".into(),
        label: "Home".into(),
    }];
    adapter.categories = Some(IndexerCapabilities {
        source_id: "home".into(),
        search_available: true,
        max_limit: 100,
        categories: vec![Category {
            id: 2000,
            name: "Movies".into(),
            parent_id: None,
        }],
    });
    adapter.category = vec![2000];
    adapter.query = "open film".into();
    (runtime, app, adapter)
}
fn item(index: usize, language: &str) -> SearchItem {
    SearchItem {
        result_ref: format!("{index:032x}"),
        title: format!("Film {index}"),
        size: None,
        seeders: Some(2),
        published_at: None,
        category_ids: vec![2000],
        languages: vec![language.into()],
    }
}
fn page(items: Vec<SearchItem>, next: Option<u64>) -> SearchPage {
    SearchPage {
        source_id: "home".into(),
        items,
        next_offset: next,
        total: Some(150),
    }
}
fn select_session() -> Session {
    crate::app::ott_companion::protocol::session_for_test(json!({
        "id":"lease-1", "state":"select",
        "files":[{"index":0,"name":"First.mp4","length":4096},{"index":1,"name":"Second.mp4","length":4096}]
    })).unwrap()
}
fn capture_file_action(runtime: &mut Runtime, adapter: Adapter) -> (Adapter, OttAction) {
    runtime.page = "torrent-files";
    runtime.companion_adapter = Some(adapter);
    let action = view(runtime)
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.content_title.as_deref() == Some("First.mp4"))
        .unwrap()
        .action;
    (runtime.companion_adapter.take().unwrap(), action)
}

// Configuration is already applied; tick must not cancel the fixture merely
// because it observes the initial durable configuration for the first time.
fn navigation_fixture() -> (Runtime, App, Adapter) {
    let (runtime, app, mut adapter) = fixture();
    adapter.applied_revision = Some(0);
    (runtime, app, adapter)
}

#[test]
fn generic_navigation_retires_torrent_catalog_requests_without_reopening_the_page() {
    let _serial = plx_base::testlock::serial();
    for action in [OttAction::ShowProviders, OttAction::ShowProfiles,
        OttAction::ShowSettings, OttAction::SelectProvider("local".into())] {
        let (mut runtime, mut app, mut adapter) = navigation_fixture();
        runtime.page = "torrent-sources";
        adapter.ui_request(&mut runtime, Request::Indexer(IndexerRequest::List), Target::Sources).unwrap();
        let generation = adapter.generation;
        let cancel = adapter.pending[0].cancelled.clone();
        let deadline = adapter.pending[0].deadline;
        let transport_deadline = adapter.pending[0].transport_deadline;
        let saved = json!(runtime.saved);
        runtime.companion_adapter = Some(adapter);
        super::super::handle_action(&mut runtime, &mut app, action).unwrap();
        runtime.accepted_navigation(false);
        let destination = runtime.page;
        runtime.status = "Destination status".into();

        tick(&mut runtime, &mut app);

        let mut adapter = runtime.companion_adapter.take().unwrap();
        assert!(cancel.load(Ordering::Acquire));
        assert!(adapter.pending.is_empty());
        assert_eq!(adapter.generation, generation, "catalog cancellation must preserve media generation");
        // A completion already removed from transport still carries its old
        // page authority and cannot re-open the retired Sources screen.
        let late = Pending { id: Some(7), request: Request::Indexer(IndexerRequest::List),
            target: Target::Sources, deadline, transport_deadline, generation, cancelled: cancel };
        let now = runtime.now_ms();
        adapter.complete_ui_reply(&mut runtime, &mut app, late, Ok(Reply::Sources(Vec::new())), now);
        assert_eq!(runtime.page, destination);
        assert_eq!(runtime.status, "Destination status");
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[test]
fn generic_navigation_cancels_pending_search_but_keeps_remote_requests() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = navigation_fixture();
    adapter.search(&mut runtime, false).unwrap();
    let cancel = adapter.pending[0].cancelled.clone();
    let params = json!({"version":1,"runtime":runtime.remote.runtime(),"op":"list"});
    let mut invocation = runtime.remote.request_for_test("torrent_indexer", params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(UI_TIMEOUT);
    assert!(adapter.command(&mut runtime, &mut app, &invocation).is_none());
    let remote_cancel = adapter.pending[1].cancelled.clone();
    runtime.companion_adapter = Some(adapter);
    super::super::handle_action(&mut runtime, &mut app, OttAction::ShowProviders).unwrap();
    runtime.accepted_navigation(false);
    runtime.status = "Choose a provider".into();

    tick(&mut runtime, &mut app);

    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert!(!remote_cancel.load(Ordering::Acquire));
    assert_eq!(adapter.pending.len(), 1);
    assert!(matches!(adapter.pending[0].target, Target::Remote { .. }));
    assert!(adapter.items.is_empty());
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.status, "Choose a provider");
}

#[test]
fn catalog_navigation_preserves_submitted_remote_plugin_epoch() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = navigation_fixture();
    let mut submitted = runtime.companion.ready_mailbox_for_test();
    assert!(runtime.companion.snapshot().ready, "fixture must publish a current helper lifetime");
    adapter.search(&mut runtime, false).unwrap();
    let catalog_cancel = adapter.pending[0].cancelled.clone();
    let params = json!({"version":1,"runtime":runtime.remote.runtime(),
        "op":"resolve_plugin","id":"fixture","input":{}});
    let mut invocation = runtime.remote.request_for_test("native_companion", params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(UI_TIMEOUT);
    let response = adapter.command(&mut runtime, &mut app, &invocation);
    assert!(response.is_none(), "plugin admission failed: {response:?}");
    assert!(adapter.pending[1].id.is_some(), "plugin request reached the supervisor mailbox");
    let remote_cancel = adapter.pending[1].cancelled.clone();
    runtime.companion_adapter = Some(adapter);
    super::super::handle_action(&mut runtime, &mut app, OttAction::ShowProviders).unwrap();
    runtime.accepted_navigation(false);
    runtime.status = "Choose a provider".into();

    tick(&mut runtime, &mut app);

    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(catalog_cancel.load(Ordering::Acquire));
    assert!(!remote_cancel.load(Ordering::Acquire));
    assert_eq!(adapter.pending.len(), 1);
    assert!(matches!(&adapter.pending[0].request, Request::ResolvePlugin { .. }));
    assert!(matches!(submitted().unwrap().0, Request::Indexer(IndexerRequest::Search { .. })));
    let (request, captured_epoch, current_epoch) = submitted().unwrap();
    assert!(matches!(request, Request::ResolvePlugin { .. }));
    assert_eq!(captured_epoch, current_epoch,
        "retaining the Remote entry is insufficient if the supervisor cancels its media epoch");
    assert!(submitted().is_none());
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.status, "Choose a provider");
}

#[test]
fn abandoned_catalog_is_retired_without_cancelling_an_active_session_poll() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = navigation_fixture();
    let mut submitted = runtime.companion.ready_mailbox_for_test();
    runtime.page = "torrent-sources";
    adapter.ui_request(&mut runtime, Request::Indexer(IndexerRequest::List), Target::Sources).unwrap();
    let catalog_cancel = adapter.pending[0].cancelled.clone();
    let generation = adapter.generation;
    let deadline = adapter.pending[0].deadline;
    let transport_deadline = adapter.pending[0].transport_deadline;
    adapter.playing = true;
    adapter.media_generation = runtime.play_generation;
    // Isolate catalog retirement from the pre-existing player lifecycle gate.
    adapter.ui_request(&mut runtime, Request::Session { id: "lease-1".into() }, Target::Session).unwrap();
    let poll_cancel = adapter.pending[1].cancelled.clone();
    let media_cancel = adapter.cancelled.clone();
    runtime.companion_adapter = Some(adapter);
    super::super::handle_action(&mut runtime, &mut app, OttAction::ShowProviders).unwrap();
    runtime.accepted_navigation(false);
    runtime.status = "Choose a provider".into();

    tick(&mut runtime, &mut app);

    let mut adapter = runtime.companion_adapter.take().unwrap();
    assert!(catalog_cancel.load(Ordering::Acquire));
    assert!(!poll_cancel.load(Ordering::Acquire));
    assert!(!media_cancel.load(Ordering::Acquire));
    assert!(adapter.playing);
    assert_eq!(adapter.generation, generation);
    assert_eq!(adapter.pending.len(), 1);
    assert!(matches!(adapter.pending[0].target, Target::Session));
    assert!(matches!(submitted().unwrap().0, Request::Indexer(IndexerRequest::List)));
    let (request, captured_epoch, current_epoch) = submitted().unwrap();
    assert!(matches!(request, Request::Session { .. }));
    assert_eq!(captured_epoch, current_epoch);
    let late = Pending { id: Some(7), request: Request::Indexer(IndexerRequest::List),
        target: Target::Sources, deadline, transport_deadline, generation, cancelled: catalog_cancel };
    let now = runtime.now_ms();
    adapter.complete_ui_reply(&mut runtime, &mut app, late, Ok(Reply::Sources(Vec::new())), now);
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.status, "Choose a provider");
}

#[test]
fn generic_navigation_retires_pending_create_and_queued_direct_inspection() {
    let _serial = plx_base::testlock::serial();
    for direct in [false, true] {
        let (mut runtime, mut app, mut adapter) = navigation_fixture();
        if direct {
            runtime.page = "stremio-streams";
            adapter.media_provider = "stremio";
            adapter.direct_source = Some(plx_media::ff::ExternalSource::Http(
                "https://media.example/film.mp4".into()));
            adapter.media_generation = runtime.play_generation;
            adapter.media_deadline = runtime.now_ms().saturating_add(120_000);
            adapter.route_ticket = Some(plx_media::route::worker_ticket());
            adapter.inspecting = true;
            adapter.inspected_tx.send(Completed::Media(Inspected {
                generation: adapter.generation,
                result: Err("retired inspector reached admission"),
            })).unwrap();
        } else {
            adapter.begin_media(&mut runtime, &mut app, "Torrent".into(), Request::Create {
                source: CreateSource::Direct("https://media.example/film.torrent".into()),
                file_index: None,
            }, Target::Session).unwrap();
            assert!(adapter.pending[0].id.is_none(), "the helper has not started yet");
        }
        let cancel = adapter.cancelled.clone();
        let saved = json!(runtime.saved);
        let play_generation = runtime.play_generation;
        runtime.companion_adapter = Some(adapter);
        super::super::handle_action(&mut runtime, &mut app, OttAction::ShowSettings).unwrap();
        runtime.accepted_navigation(false);
        runtime.status = "Settings destination".into();

        tick(&mut runtime, &mut app);

        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(cancel.load(Ordering::Acquire));
        assert!(adapter.pending.is_empty() && adapter.session.is_none());
        assert!(!adapter.inspecting && !adapter.playing);
        assert!(adapter.direct_source.is_none());
        assert_eq!(adapter.media_deadline, 0);
        assert_eq!(runtime.play_generation, play_generation);
        assert_eq!(runtime.page, "settings");
        assert_eq!(runtime.status, "Settings destination");
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[test]
fn pending_stremio_torrent_and_direct_media_keep_their_actual_page_owner() {
    let _serial = plx_base::testlock::serial();
    for direct in [false, true] {
        let (mut runtime, mut app, mut adapter) = navigation_fixture();
        runtime.page = if direct { "stremio-streams" } else { "torrent-files" };
        adapter.media_provider = "stremio";
        adapter.media_generation = runtime.play_generation;
        adapter.media_deadline = runtime.now_ms().saturating_add(120_000);
        adapter.route_ticket = Some(plx_media::route::worker_ticket());
        adapter.inspecting = true;
        if direct {
            adapter.direct_source = Some(plx_media::ff::ExternalSource::Http(
                "https://media.example/film.mp4".into()));
        }
        let cancel = adapter.cancelled.clone();
        runtime.companion_adapter = Some(adapter);

        tick(&mut runtime, &mut app);

        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(!cancel.load(Ordering::Acquire));
        assert!(adapter.inspecting);
        assert!(adapter.media_deadline > 0);
    }
}

#[test]
fn generic_navigation_does_not_retire_an_active_media_poll() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = navigation_fixture();
    runtime.page = "torrent-files";
    adapter.playing = true;
    adapter.media_generation = runtime.play_generation;
    // Keep the pre-existing player lifecycle check out of this navigation-only
    // fixture. The request is the same Session poll used by active playback.
    adapter.ui_request(&mut runtime, Request::Session { id: "lease-1".into() }, Target::Session).unwrap();
    let cancel = adapter.cancelled.clone();
    let poll_cancel = adapter.pending[0].cancelled.clone();
    runtime.companion_adapter = Some(adapter);
    super::super::handle_action(&mut runtime, &mut app, OttAction::ShowProviders).unwrap();
    runtime.accepted_navigation(false);

    tick(&mut runtime, &mut app);

    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.playing);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(!poll_cancel.load(Ordering::Acquire));
    assert_eq!(adapter.pending.len(), 1);
}

#[test]
fn file_click_is_bound_to_session_and_generation_not_just_file_index() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.session = Some(select_session());
    let (mut adapter, old_action) = capture_file_action(&mut runtime, adapter);
    adapter.session.as_mut().unwrap().id = "lease-2".into();
    assert!(adapter.action(&mut runtime, &mut app, &old_action).is_err());
    assert!(
        adapter.pending.is_empty(),
        "old file 0 must not select file 0 from the new session"
    );
    adapter.session.as_mut().unwrap().id = "lease-1".into();
    adapter.generation += 1;
    assert!(adapter.action(&mut runtime, &mut app, &old_action).is_err());
    assert!(adapter.pending.is_empty());
}
#[test]
fn one_file_choice_enqueues_once_and_preserves_its_session() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.session = Some(select_session());
    let (mut adapter, action) = capture_file_action(&mut runtime, adapter);
    adapter.action(&mut runtime, &mut app, &action).unwrap();
    assert!(
        matches!(&adapter.pending[0].request, Request::Select { id, file_index: 0 } if id == "lease-1")
    );
    assert!(adapter.action(&mut runtime, &mut app, &action).is_err());
    assert_eq!(adapter.pending.len(), 1);
}
#[test]
fn scoped_search_keeps_exact_child_query_category_and_language_order_across_pages() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, app, mut adapter) = fixture();
    adapter.indexer = Some("ChildA".into());
    runtime
        .saved
        .settings
        .insert(LANGUAGE_KEY.into(), json!("Russian"));
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    assert!(
        matches!(&pending.request, Request::Indexer(IndexerRequest::ScopedSearch {
        source_id, indexer_id, query, category_ids, offset: 0, limit: 50,
    }) if source_id == "home" && indexer_id == "ChildA" && query == "open film" && category_ids == &[2000])
    );
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::ScopedSearch {
                indexer_id: "ChildA".into(),
                page: page(vec![item(1, "en"), item(2, "ru")], Some(50)),
            }),
        )
        .unwrap();
    assert_eq!(
        adapter
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect::<Vec<_>>(),
        ["Film 2", "Film 1"]
    );
    adapter.search(&mut runtime, true).unwrap();
    let pending = adapter.pending.pop().unwrap();
    assert!(
        matches!(&pending.request, Request::Indexer(IndexerRequest::ScopedSearch {
        indexer_id, query, category_ids, offset: 50, ..
    }) if indexer_id == "ChildA" && query == "open film" && category_ids == &[2000])
    );
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::ScopedSearch {
                indexer_id: "ChildA".into(),
                page: page(vec![item(3, "ru")], Some(100)),
            }),
        )
        .unwrap();
    assert_eq!(
        adapter
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect::<Vec<_>>(),
        ["Film 2", "Film 1", "Film 3"]
    );
}
#[test]
fn wrong_or_aggregate_scope_cannot_publish_into_child_results() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, app, mut adapter) = fixture();
    adapter.indexer = Some("ChildA".into());
    for reply in [
        Reply::Search(page(vec![item(1, "en")], None)),
        Reply::ScopedSearch {
            indexer_id: "childa".into(),
            page: page(vec![item(1, "en")], None),
        },
    ] {
        adapter.search(&mut runtime, false).unwrap();
        let pending = adapter.pending.pop().unwrap();
        assert!(adapter
            .ui_reply(&mut runtime, &app, pending.target, Ok(reply))
            .is_err());
        assert!(adapter.items.is_empty());
    }
}
#[test]
fn refreshed_result_cannot_reuse_previous_result_action_even_with_same_reference() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], None))),
        )
        .unwrap();
    let action = OttAction::Play(format!("torrent:{}", adapter.items[0].id));
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], None))),
        )
        .unwrap();
    assert!(adapter.action(&mut runtime, &mut app, &action).is_err());
    assert!(adapter.pending.is_empty());
}
#[test]
fn repeated_page_ends_pagination_instead_of_offering_infinite_load_more() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], Some(50)))),
        )
        .unwrap();
    adapter.search(&mut runtime, true).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], Some(100)))),
        )
        .unwrap();
    assert_eq!(adapter.items.len(), 1);
    assert_eq!(adapter.next, None);
}
#[test]
fn back_cancels_pending_search_and_preserves_the_scope_for_retry() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.indexer = Some("ChildA".into());
    adapter.search(&mut runtime, false).unwrap();
    let cancel = adapter.pending[0].cancelled.clone();
    adapter
        .action(&mut runtime, &mut app, &OttAction::Back)
        .unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert!(adapter.pending.is_empty());
    assert_eq!(runtime.page, "torrent-categories");
    assert_eq!(adapter.indexer.as_deref(), Some("ChildA"));
    assert_eq!(adapter.query, "open film");
    assert_eq!(adapter.category, [2000]);
    adapter
        .action(&mut runtime, &mut app, &OttAction::Refresh)
        .unwrap();
    assert!(
        matches!(&adapter.pending[0].request, Request::Indexer(IndexerRequest::ScopedCapabilities {source_id,indexer_id})
        if source_id == "home" && indexer_id == "ChildA")
    );
}
#[test]
fn child_enumeration_failure_retries_children_without_aggregate_fallback() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.children_request(&mut runtime).unwrap();
    let pending = adapter.pending.pop().unwrap();
    assert!(adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Err(Request::Session { id: String::new() }
                .validate()
                .unwrap_err())
        )
        .is_err());
    assert!(adapter.pending.is_empty());
    assert!(adapter.children.is_none());
    adapter
        .action(&mut runtime, &mut app, &OttAction::Refresh)
        .unwrap();
    assert!(
        matches!(&adapter.pending[0].request, Request::Indexer(IndexerRequest::Indexers {source_id}) if source_id == "home")
    );
}
#[test]
fn child_choice_is_generation_bound_and_keeps_exact_identifier() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    runtime.page = "torrent-children";
    adapter.children = Some(IndexerChildren {
        source_id: "home".into(),
        supported: true,
        indexers: vec![IndexerChild {
            id: "ChildA".into(),
            label: "Films".into(),
            language: Some("ru".into()),
            kind: None,
        }],
    });
    adapter
        .action(
            &mut runtime,
            &mut app,
            &OttAction::EditSetting("torrent.indexer:0:ChildA".into()),
        )
        .unwrap();
    assert!(
        matches!(&adapter.pending[0].request, Request::Indexer(IndexerRequest::ScopedCapabilities {indexer_id,..}) if indexer_id == "ChildA")
    );
    assert!(adapter
        .action(
            &mut runtime,
            &mut app,
            &OttAction::EditSetting("torrent.indexer:0:ChildA".into())
        )
        .is_err());
}

#[test]
fn remote_children_contract_requires_exact_nonempty_child_and_feature() {
    let params = json!({"version":1,"runtime":"native","op":"search","sourceId":"home",
        "indexerId":"ChildA","query":"film","categoryIds":[2000],"offset":50,"limit":20});
    let (_, request, _) = indexer_request(&params, "native").unwrap();
    assert!(
        matches!(&request, Request::Indexer(IndexerRequest::ScopedSearch {indexer_id,offset:50,limit:20,..}) if indexer_id == "ChildA")
    );
    assert!(request_capability(true, false, &request).is_err());
    assert!(request_capability(true, true, &request).is_ok());
    assert!(
        request_capability(false, false, &request).is_ok(),
        "unstarted helper is checked again when ready"
    );
    for child in [
        json!(null),
        json!(""),
        json!("all"),
        json!("../ChildA"),
        json!("A&B"),
    ] {
        let mut params = params.clone();
        params["indexerId"] = child;
        assert!(indexer_request(&params, "native").is_err());
    }
    let list = json!({"version":1,"runtime":"native","op":"indexers","sourceId":"home"});
    assert!(matches!(
        indexer_request(&list, "native").unwrap().1,
        Request::Indexer(IndexerRequest::Indexers { .. })
    ));
}
#[test]
fn remote_scoped_reply_preserves_child_identity() {
    let reply = reply_value(Reply::ScopedSearch {
        indexer_id: "ChildA".into(),
        page: page(vec![item(1, "en")], None),
    })
    .unwrap();
    assert_eq!(reply["indexerId"], "ChildA");
    assert_eq!(reply["sourceId"], "home");
    assert!(reply.get("endpoint").is_none());
}

#[test]
fn reply_at_deadline_ends_loading_without_publishing_rows() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    let deadline = pending.deadline;
    adapter.complete_ui_reply(
        &mut runtime,
        &mut app,
        pending,
        Ok(Reply::Search(page(vec![item(1, "en")], None))),
        deadline,
    );
    assert_eq!(runtime.status, "Request timed out");
    assert!(adapter.items.is_empty());
    assert!(adapter.pending.is_empty());
}
#[test]
fn late_prepare_cancels_media_without_creating_a_session() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter
        .ui_request(
            &mut runtime,
            Request::Indexer(IndexerRequest::Prepare {
                source_id: "home".into(),
                result_ref: "a".repeat(32),
            }),
            Target::Prepare,
        )
        .unwrap();
    adapter.media_deadline = 120_000;
    let cancel = adapter.cancelled.clone();
    let pending = adapter.pending.pop().unwrap();
    let deadline = pending.deadline;
    adapter.complete_ui_reply(
        &mut runtime,
        &mut app,
        pending,
        Ok(Reply::Prepared(
            crate::app::ott_companion::protocol::PreparedSource {
                source_id: "home".into(),
                source_ref: "a".repeat(32),
            },
        )),
        deadline,
    );
    assert_eq!(runtime.status, "Request timed out");
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(adapter.media_deadline, 0);
    assert!(adapter.pending.is_empty() && adapter.session.is_none());
}
#[test]
fn cancelled_generation_cannot_replace_a_newer_screen_status() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter.navigate(&mut runtime, &mut app);
    runtime.status = "New screen".into();
    let deadline = pending.deadline;
    adapter.complete_ui_reply(
        &mut runtime,
        &mut app,
        pending,
        Ok(Reply::Search(page(vec![item(1, "en")], None))),
        deadline,
    );
    assert_eq!(runtime.status, "New screen");
    assert!(adapter.items.is_empty());
}

#[test]
fn direct_torrent_retry_keeps_original_source_and_does_not_open_aggregate_search() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    let url = "https://media.example/open-film.torrent?ticket=opaque";
    adapter
        .begin_media(
            &mut runtime,
            &mut app,
            "Open film".into(),
            Request::Create {
                source: CreateSource::Direct(url.into()),
                file_index: None,
            },
            Target::Session,
        )
        .unwrap();
    let first_cancel = adapter.pending[0].cancelled.clone();
    adapter.cancel_media(&mut runtime);
    adapter
        .action(&mut runtime, &mut app, &OttAction::Refresh)
        .unwrap();
    assert!(first_cancel.load(Ordering::Acquire));
    assert_eq!(runtime.page, "torrent-files");
    assert_eq!(adapter.pending.len(), 1);
    assert!(matches!(&adapter.pending[0].request,Request::Create {
        source:CreateSource::Direct(source),file_index:None,
    } if source == url));
    adapter
        .action(&mut runtime, &mut app, &OttAction::Back)
        .unwrap();
    assert!(
        adapter.retry_media.is_none(),
        "explicit Back retires transient source input"
    );
}

#[test]
fn leaving_results_rejects_queued_play_without_losing_restorable_rows() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], None))),
        )
        .unwrap();
    let catalog_generation = adapter.catalog_generation;
    runtime.companion_adapter = Some(adapter);
    let old_play = view(&runtime)
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.title.contains("Film 1"))
        .unwrap()
        .action;
    super::super::handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "torrent-categories");
    let before = runtime.status.clone();
    assert!(super::super::handle_action(&mut runtime, &mut app, old_play).is_err());
    assert_eq!(runtime.page, "torrent-categories");
    assert_eq!(runtime.status, before);
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.catalog_generation, catalog_generation);
    assert_eq!(
        adapter.items.len(),
        1,
        "Back preserves rows, but not their screen authority"
    );
    assert!(adapter.pending.is_empty() && adapter.session.is_none());
    assert_eq!(adapter.media_deadline, 0);
}

#[test]
fn entering_stremio_rejects_old_torrent_rows_and_editor_submission() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, mut adapter) = fixture();
    adapter.search(&mut runtime, false).unwrap();
    let pending = adapter.pending.pop().unwrap();
    adapter
        .ui_reply(
            &mut runtime,
            &app,
            pending.target,
            Ok(Reply::Search(page(vec![item(1, "en")], Some(50)))),
        )
        .unwrap();
    let catalog_generation = adapter.catalog_generation;
    runtime.companion_adapter = Some(adapter);
    let rows = view(&runtime).unwrap().rows;
    let old_play = rows
        .iter()
        .find(|row| row.title.contains("Film 1"))
        .unwrap()
        .action
        .clone();
    let old_query = rows
        .iter()
        .find(|row| row.title == "Search")
        .unwrap()
        .action
        .clone();
    let old_more = rows
        .iter()
        .find(|row| row.title == "Load more")
        .unwrap()
        .action
        .clone();
    let OttAction::EditSetting(query_key) = &old_query else {
        panic!("query editor");
    };
    // Enter through the real adapter navigation rather than changing the page
    // alone: navigation retires generation, while keeping the catalog for Back.
    super::super::handle_action(&mut runtime, &mut app, OttAction::ShowSettings).unwrap();
    super::super::handle_action(
        &mut runtime,
        &mut app,
        OttAction::EditSetting("stremio.settings".into()),
    )
    .unwrap();
    assert_eq!(runtime.page, "stremio-settings");
    let before = runtime.status.clone();
    let rejected = [
        old_play,
        old_query.clone(),
        old_more,
        OttAction::EditSetting(format!("torrent.source:{catalog_generation}:home")),
        OttAction::EditSetting(format!("torrent.category:{catalog_generation}:2000")),
        OttAction::SubmitSetting {
            session: 0,
            key: query_key.clone(),
            value: "stale search".into(),
        },
        OttAction::SubmitSetting {
            session: 0,
            key: "torrent.link".into(),
            value: "https://media.example/old.torrent".into(),
        },
    ];
    for stale in rejected {
        assert!(super::super::handle_action(&mut runtime, &mut app, stale).is_err());
        assert_eq!(runtime.page, "stremio-settings");
        assert_eq!(runtime.status, before);
    }
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.catalog_generation, catalog_generation);
    assert_eq!(adapter.query, "open film");
    assert_eq!(adapter.items.len(), 1);
    assert!(adapter.pending.is_empty() && adapter.session.is_none());
    assert_eq!(adapter.media_deadline, 0);
}


// Real companion admission (also used by Stremio), without decoder startup or IO.
struct ResetSubtitleAdmission;
impl Drop for ResetSubtitleAdmission {
    fn drop(&mut self) {
        let mut idle = plx_media::route::PlaybackSession::IDLE;
        plx_media::route::cancel_play(&mut idle);
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn subtitle_admission_media() -> plx_media::ff::inspect::MediaInfo {
    plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 60_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: plx_data::metadata::Dovi::NONE,
    }
}
fn saved_subtitle_admission_preferences(runtime: &mut Runtime)
    -> (plx_media::route::NativeSubtitlePreferences, plx_media::player::text_style::TextAppearance)
{
    use super::super::subtitle_settings::Edit;
    use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};
    use plx_media::player::text_style::{TextAppearance, TextColor};
    let geometry = plx_media::route::NativeSubtitlePreferences {
        size: SubtitleSize::Huge,
        position: SubtitlePosition::High,
        tone: SubtitleTone::Silver,
    };
    let appearance = TextAppearance { color: TextColor::Yellow, ..Default::default() };
    for edit in [Edit::Size(geometry.size), Edit::Position(geometry.position),
        Edit::Tone(geometry.tone), Edit::Appearance(appearance)] {
        runtime.saved = edit.candidate(&runtime.saved).unwrap();
    }
    (geometry, appearance)
}
fn arm_direct_subtitle_admission(adapter: &mut Adapter, url: &str)
    -> plx_media::ff::ExternalSource
{
    let source = plx_media::ff::ExternalSource::Http(url.into());
    adapter.direct_source = Some(source.clone());
    adapter.route_ticket = Some(plx_media::route::worker_ticket());
    adapter.media_provider = "stremio";
    adapter.title = "Subtitle fixture".into();
    source
}

#[test]
fn companion_cold_start_admission_restores_saved_subtitle_geometry() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetSubtitleAdmission;
    let (mut runtime, mut app, mut adapter) = fixture();
    // Reset the native acquisition slot as well as the fresh per-App session.
    plx_media::route::cancel_play(&mut app.player.session);
    assert!(!plx_media::route::play_pending());
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let (geometry, appearance) = saved_subtitle_admission_preferences(&mut runtime);
    let saved = runtime.saved.settings.clone();
    // This is the same verified-load publication as the real Runtime. With no
    // external source yet, geometry cannot be published into a native session.
    super::super::commands::settings_after_saved(&mut runtime, &mut app).unwrap();
    assert!(plx_media::route::native_subtitle_preferences(&app.player.session).is_none());
    assert_eq!(plx_media::route::subtitle_text_appearance(&app.player.session), appearance);
    let source = arm_direct_subtitle_admission(&mut adapter, "https://media.invalid:443/movie.mp4");
    assert_eq!(adapter.admit_playback(&mut runtime, &mut app, source.clone(), subtitle_admission_media()), Ok(()));
    assert_eq!(plx_media::route::external_source(&app.player.session), Some(source));
    assert_eq!(plx_media::route::native_subtitle_preferences(&app.player.session), Some(geometry));
    assert_eq!(plx_media::route::subtitle_text_appearance(&app.player.session), appearance);
    assert_eq!(runtime.saved.settings, saved, "admission restores without another save");
    assert!(!runtime.saving);
}

#[test]
fn companion_invalid_saved_subtitles_reject_before_replacing_current_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetSubtitleAdmission;
    let (mut runtime, mut app, mut adapter) = fixture();
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let previous = plx_media::ff::ExternalSource::Http("https://media.invalid:443/previous.mp4".into());
    plx_media::route::request_external_source(&mut app.player.session, &subtitle_admission_media(),
        previous.clone(), "Previous", "Fixture", plx_platform::devcaps::assumed_shared()).unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    runtime.saved.settings.insert("nativeSubtitleSize".into(), json!("invalid"));
    let source = arm_direct_subtitle_admission(&mut adapter, "https://media.invalid:443/replacement.mp4");
    assert_eq!(adapter.admit_playback(&mut runtime, &mut app, source, subtitle_admission_media()),
        Err("Invalid stored subtitle settings"));
    assert_eq!(plx_media::route::external_source(&app.player.session), Some(previous));
    assert!(plx_media::route::playback_identity(&app.player.session) == identity);
    assert!(app.bridge.metadata_view().now_playing().is_none());
    assert!(!runtime.saving);
}

#[test]
fn companion_rejected_media_does_not_publish_saved_subtitle_preferences() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetSubtitleAdmission;
    let (mut runtime, mut app, mut adapter) = fixture();
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let (_, appearance) = saved_subtitle_admission_preferences(&mut runtime);
    let before = plx_media::route::subtitle_text_appearance(&app.player.session);
    assert_ne!(before, appearance);
    let saved = runtime.saved.settings.clone();
    let source = arm_direct_subtitle_admission(&mut adapter, "https://media.invalid:443/unsupported.mp4");
    let mut invalid = subtitle_admission_media();
    invalid.width = 0;
    assert_eq!(adapter.admit_playback(&mut runtime, &mut app, source, invalid),
        Err("The native decoder cannot open this torrent file"));
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(plx_media::route::native_subtitle_preferences(&app.player.session).is_none());
    assert_eq!(plx_media::route::subtitle_text_appearance(&app.player.session), before);
    assert_eq!(runtime.saved.settings, saved);
    assert!(!runtime.saving);
}

// Provider form regressions use the production adapter and the existing inert
// supervisor mailbox. Replies below model the helper; no helper or disk runs.
fn form_action(runtime: &Runtime, field: &str) -> OttAction {
    view(runtime).unwrap().rows.into_iter()
        .find(|row| row.id.starts_with("torrent.configure:") && row.id.ends_with(&format!(":{field}")))
        .unwrap_or_else(|| panic!("missing form field {field}")).action
}
fn form_submit(runtime: &mut Runtime, app: &mut App, field: &str, value: &str) {
    let OttAction::EditSetting(key) = form_action(runtime, field) else { panic!("field action") };
    action(runtime, app, &OttAction::SubmitSetting { session: 0, key, value: value.into() })
        .unwrap().unwrap();
}
fn open_form(runtime: &mut Runtime, app: &mut App, mut adapter: Adapter) {
    runtime.page = "torrent-sources";
    // Model a completed authoritative List, not merely a hand-filled cache.
    let sources = adapter.sources.clone();
    adapter.ui_reply(runtime, app, Target::Sources, Ok(Reply::Sources(sources))).unwrap();
    runtime.companion_adapter = Some(adapter);
    action(runtime, app, &OttAction::EditSetting("torrent.add".into())).unwrap().unwrap();
    assert_eq!(runtime.page, "torrent-configure");
}
fn assert_form_redacted(runtime: &Runtime, secret: &str) {
    let screen = view(runtime).unwrap();
    assert!(!screen.title.contains(secret) && !screen.description.contains(secret)
        && !screen.status.contains(secret));
    for row in screen.rows {
        assert!(!row.id.contains(secret) && !row.title.contains(secret)
            && !row.detail.contains(secret) && !row.description.contains(secret));
    }
}

#[test]
fn torrent_entry_stays_visible_and_unready_open_only_explains_service_setup() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    runtime.page = "providers";
    runtime.companion_adapter = Some(adapter);
    let entry = rows(&runtime).into_iter().find(|row| row.id == "torrent.open").unwrap();
    assert!(entry.enabled && !entry.detail.is_empty());
    let before = json!(runtime.saved);
    let capabilities_before = capabilities(&runtime);
    action(&mut runtime, &mut app, &entry.action).unwrap().unwrap();
    assert_eq!(runtime.page, "torrent-settings");
    assert!(!runtime.status.is_empty());
    assert_eq!(json!(runtime.saved), before);
    assert_eq!(capabilities(&runtime), capabilities_before);
    assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
    assert!(!runtime.companion.snapshot().ready);
}

#[test]
fn explicit_load_is_the_only_form_effect_then_source_choice_reaches_real_search() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    let before = json!(runtime.saved);
    open_form(&mut runtime, &mut app, adapter);
    form_submit(&mut runtime, &mut app, "id", "films");
    form_submit(&mut runtime, &mut app, "endpoint", "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/api");
    form_submit(&mut runtime, &mut app, "key", "private-api-key");
    form_submit(&mut runtime, &mut app, "label", "");
    assert_form_redacted(&runtime, "private-api-key");
    assert!(mailbox().is_none());
    assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
    action(&mut runtime, &mut app, &OttAction::Refresh).unwrap().unwrap();
    assert!(mailbox().is_none(), "Refresh must not submit a settings form");
    let load = form_action(&runtime, "load");
    action(&mut runtime, &mut app, &load).unwrap().unwrap();
    let (request, _, _) = mailbox().unwrap();
    assert!(matches!(request, Request::Indexer(IndexerRequest::Configure { id, label, api_key, .. })
        if id == "films" && label == "films" && api_key == "private-api-key"));
    assert!(action(&mut runtime, &mut app, &load).unwrap().is_err());
    assert!(mailbox().is_none(), "duplicate Load cannot configure twice");
    assert_eq!(json!(runtime.saved), before, "credentials never enter OTT Saved");
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let pending = adapter.pending.remove(0);
    let now = runtime.now_ms();
    adapter.complete_ui_reply(&mut runtime, &mut app, pending,
        Ok(Reply::Configured(IndexerSource { id: "films".into(), label: "films".into() })), now);
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::List)));
    let pending = adapter.pending.remove(0);
    adapter.complete_ui_reply(&mut runtime, &mut app, pending,
        Ok(Reply::Sources(vec![IndexerSource { id: "films".into(), label: "films".into() }])), now);
    runtime.companion_adapter = Some(adapter);
    let source = view(&runtime).unwrap().rows.into_iter().find(|row| row.title == "films").unwrap().action;
    action(&mut runtime, &mut app, &source).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Capabilities { id }) if id == "films"));
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let pending = adapter.pending.remove(0);
    adapter.complete_ui_reply(&mut runtime, &mut app, pending, Ok(Reply::Categories(IndexerCapabilities {
        source_id: "films".into(), search_available: true, max_limit: 50, categories: vec![],
    })), now);
    runtime.companion_adapter = Some(adapter);
    let all = view(&runtime).unwrap().rows.into_iter().find(|row| row.title == "All").unwrap().action;
    action(&mut runtime, &mut app, &all).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Search { source_id, query, category_ids, .. })
        if source_id == "films" && query.is_empty() && category_ids.is_empty()));
}

#[test]
fn cancel_discards_secret_and_old_form_key_cannot_edit_the_next_form() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    open_form(&mut runtime, &mut app, adapter);
    form_submit(&mut runtime, &mut app, "key", "abandoned-secret");
    let OttAction::EditSetting(old_key) = form_action(&runtime, "key") else { panic!("key") };
    action(&mut runtime, &mut app, &OttAction::Back).unwrap().unwrap();
    assert_eq!(runtime.page, "torrent-sources");
    action(&mut runtime, &mut app, &OttAction::EditSetting("torrent.add".into())).unwrap().unwrap();
    assert!(action(&mut runtime, &mut app, &OttAction::SubmitSetting {
        session: 0, key: old_key, value: "late-secret".into(),
    }).unwrap().is_err());
    form_submit(&mut runtime, &mut app, "endpoint", "http://127.0.0.1:9117/api");
    let load = form_action(&runtime, "load");
    action(&mut runtime, &mut app, &load).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Configure { api_key, .. }) if api_key.is_empty()));
}

#[test]
fn form_rejects_invalid_endpoint_and_existing_id_without_a_configure_effect() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    open_form(&mut runtime, &mut app, adapter);
    form_submit(&mut runtime, &mut app, "endpoint", "file:///private/settings");
    let load = form_action(&runtime, "load");
    assert!(action(&mut runtime, &mut app, &load).unwrap().is_err());
    form_submit(&mut runtime, &mut app, "endpoint", "http://127.0.0.1:9117/api");
    form_submit(&mut runtime, &mut app, "id", "home");
    assert!(action(&mut runtime, &mut app, &load).unwrap().is_err());
    assert!(mailbox().is_none());
    assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
}

#[test]
fn cancelled_load_cannot_reopen_source_list_from_its_late_completion() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    open_form(&mut runtime, &mut app, adapter);
    form_submit(&mut runtime, &mut app, "endpoint", "http://127.0.0.1:9117/api");
    let load = form_action(&runtime, "load");
    action(&mut runtime, &mut app, &load).unwrap().unwrap();
    let (request, _, _) = mailbox().unwrap();
    let Request::Indexer(IndexerRequest::Configure { id, label, .. }) = request else { panic!("configure") };
    // Hold the actual pending completion as the transport can do during Back.
    let pending = runtime.companion_adapter.as_mut().unwrap().pending.remove(0);
    action(&mut runtime, &mut app, &OttAction::Back).unwrap().unwrap();
    let page = runtime.page;
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let now = runtime.now_ms();
    adapter.complete_ui_reply(&mut runtime, &mut app, pending,
        Ok(Reply::Configured(IndexerSource { id: id.clone(), label: label.clone() })), now);
    assert_eq!(runtime.page, page);
    assert!(adapter.pending.is_empty() && mailbox().is_none());
    runtime.companion_adapter = Some(adapter);
    let add = OttAction::EditSetting("torrent.add".into());
    assert!(!view(&runtime).unwrap().rows.iter().find(|row| row.id == "torrent.add").unwrap().enabled);
    assert!(action(&mut runtime, &mut app, &add).unwrap().is_err());
    assert!(runtime.status.contains("Refresh"));
    action(&mut runtime, &mut app, &OttAction::Refresh).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::List)));
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let pending = adapter.pending.remove(0);
    adapter.complete_ui_reply(&mut runtime, &mut app, pending,
        Ok(Reply::Sources(vec![IndexerSource { id: id.clone(), label }])), now);
    runtime.companion_adapter = Some(adapter);
    action(&mut runtime, &mut app, &add).unwrap().unwrap();
    assert_ne!(view(&runtime).unwrap().rows.iter().find(|row| row.id.ends_with(":id")).unwrap().detail, id,
        "the helper may have committed the canceled request; its ID cannot be reused");
}

#[test]
fn generic_navigation_and_suspend_retire_form_credentials_and_editor_epoch() {
    let _serial = plx_base::testlock::serial();
    for suspend_first in [false, true] {
        let (mut runtime, mut app, mut adapter) = fixture();
        runtime.saved.settings.insert(SETTINGS_KEY.into(), json!(adapter.applied));
        adapter.applied_revision = Some(0);
        let mut mailbox = runtime.companion.ready_mailbox_for_test();
        open_form(&mut runtime, &mut app, adapter);
        form_submit(&mut runtime, &mut app, "key", "lifecycle-secret");
        let OttAction::EditSetting(old_key) = form_action(&runtime, "key") else { panic!("key") };
        if suspend_first { suspend(&mut runtime, &mut app); }
        else {
            super::super::handle_action(&mut runtime, &mut app, OttAction::ShowProviders).unwrap();
            runtime.accepted_navigation(false);
            tick(&mut runtime, &mut app);
        }
        runtime.page = "torrent-sources";
        action(&mut runtime, &mut app, &OttAction::EditSetting("torrent.add".into())).unwrap().unwrap();
        assert!(action(&mut runtime, &mut app, &OttAction::SubmitSetting {
            session: 0, key: old_key, value: "late-secret".into(),
        }).unwrap().is_err());
        form_submit(&mut runtime, &mut app, "endpoint", "http://127.0.0.1:9117/api");
        let load = form_action(&runtime, "load");
        action(&mut runtime, &mut app, &load).unwrap().unwrap();
        assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Configure { api_key, .. }) if api_key.is_empty()));
    }
}

#[test]
fn unavailable_indexer_search_is_explained_and_not_offered_as_enabled_action() {
    let _serial = plx_base::testlock::serial();
    for available in [false, true] {
        let (mut runtime, app, mut adapter) = fixture();
        let mut caps = adapter.categories.clone().unwrap();
        caps.search_available = available;
        adapter.ui_reply(&mut runtime, &app,
            Target::Categories { source_id: "home".into(), indexer_id: None }, Ok(Reply::Categories(caps))).unwrap();
        runtime.companion_adapter = Some(adapter);
        let screen = view(&runtime).unwrap();
        for row in screen.rows.iter().filter(|row| row.id.starts_with("torrent.category:") || row.id.starts_with("torrent.query:")) {
            assert_eq!(row.enabled, available);
            if !available { assert!(!row.detail.is_empty()); }
        }
        assert!(screen.rows.iter().any(|row| row.id == "torrent.back" && row.enabled));
    }
}

#[test]
fn add_waits_for_successful_source_list_including_empty_list_and_refuses_failed_list() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, adapter) = fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    runtime.page = "providers";
    runtime.companion_adapter = Some(adapter);
    action(&mut runtime, &mut app, &OttAction::EditSetting("torrent.open".into())).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::List)));
    let add = OttAction::EditSetting("torrent.add".into());
    assert!(!view(&runtime).unwrap().rows.iter().find(|row| row.id == "torrent.add").unwrap().enabled);
    assert!(action(&mut runtime, &mut app, &add).unwrap().is_err());
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let pending = adapter.pending.remove(0);
    let now = runtime.now_ms();
    adapter.complete_ui_reply(&mut runtime, &mut app, pending,
        Err(Request::Session { id: String::new() }.validate().unwrap_err()), now);
    runtime.companion_adapter = Some(adapter);
    assert!(action(&mut runtime, &mut app, &add).unwrap().is_err());
    action(&mut runtime, &mut app, &OttAction::Refresh).unwrap().unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::List)));
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let pending = adapter.pending.remove(0);
    adapter.complete_ui_reply(&mut runtime, &mut app, pending, Ok(Reply::Sources(vec![])), now);
    runtime.companion_adapter = Some(adapter);
    assert!(view(&runtime).unwrap().rows.iter().find(|row| row.id == "torrent.add").unwrap().enabled);
    action(&mut runtime, &mut app, &add).unwrap().unwrap();
    assert_eq!(runtime.page, "torrent-configure");
    assert!(mailbox().is_none());
}

#[test]
fn remote_source_mutation_invalidates_ui_inventory_and_retires_older_list() {
    let _serial = plx_base::testlock::serial();
    for remove in [false, true] {
        for pending_list in [false, true] {
            let (mut runtime, mut app, mut adapter) = fixture();
            let mut mailbox = runtime.companion.ready_mailbox_for_test();
            let sources = adapter.sources.clone();
            adapter.ui_reply(&mut runtime, &app, Target::Sources, Ok(Reply::Sources(sources))).unwrap();
            let old_cancel = if pending_list {
                adapter.ui_request(&mut runtime, Request::Indexer(IndexerRequest::List), Target::Sources).unwrap();
                assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::List)));
                Some(adapter.pending[0].cancelled.clone())
            } else { None };
            runtime.companion_adapter = Some(adapter);
            let params = if remove {
                json!({"version":1,"runtime":runtime.remote.runtime(),"op":"remove","sourceId":"home"})
            } else {
                json!({"version":1,"runtime":runtime.remote.runtime(),"op":"configure","sourceId":"remote",
                    "label":"Remote","endpoint":"http://127.0.0.1:9117/api","apiKey":"remote-private"})
            };
            let mut invocation = runtime.remote.request_for_test("torrent_indexer", params);
            invocation.deadline_ms = runtime.now_ms().saturating_add(UI_TIMEOUT);
            assert!(command(&mut runtime, &mut app, &invocation).is_none());
            assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Configure { .. } | IndexerRequest::Remove { .. })));
            if let Some(cancel) = old_cancel { assert!(cancel.load(Ordering::Acquire)); }
            assert!(!runtime.companion_adapter.as_ref().unwrap().pending.iter().any(|pending| matches!(pending.target, Target::Sources)));
            assert!(!view(&runtime).unwrap().rows.iter().find(|row| row.id == "torrent.add").unwrap().enabled);
            assert!(action(&mut runtime, &mut app, &OttAction::EditSetting("torrent.add".into())).unwrap().is_err());
            assert!(action(&mut runtime, &mut app, &OttAction::Refresh).unwrap().is_err(),
                "a fresh UI List must not race an in-flight remote mutation");
            assert!(mailbox().is_none());
        }
    }
}

// These fixtures model a committed configuration at the existing tick boundary;
// no protected writer is executed. The missing-helper case below starts only the
// existing supervisor worker and uses its real validation failure, not a child.
fn waiting_lifecycle_fixture() -> (Runtime, App) {
    let (mut runtime, app, mut adapter) = fixture();
    runtime.saved.settings.insert(SETTINGS_KEY.into(), json!(adapter.applied));
    adapter.applied_revision = Some(0);
    runtime.companion_adapter = Some(adapter);
    (runtime, app)
}

fn queue_remote_configuration(runtime: &mut Runtime, app: &mut App) -> Invocation {
    let mut invocation = runtime.remote.request_for_test("torrent_indexer", json!({
        "version":1,"runtime":runtime.remote.runtime(),"op":"configure","sourceId":"queued",
        "label":"Queued","endpoint":"http://127.0.0.1:9117/api","apiKey":"private-key"
    }));
    invocation.deadline_ms = runtime.now_ms().saturating_add(UI_TIMEOUT);
    assert!(command(runtime, app, &invocation).is_none());
    assert!(runtime.companion_adapter.as_ref().unwrap().pending[0].id.is_none());
    invocation
}

#[test]
fn queued_configuration_waits_for_initial_ready_without_losing_its_deadline() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app) = waiting_lifecycle_fixture();
    let invocation = queue_remote_configuration(&mut runtime, &mut app);
    let cancel = runtime.companion_adapter.as_ref().unwrap().pending[0].cancelled.clone();
    tick(&mut runtime, &mut app);
    let pending = &runtime.companion_adapter.as_ref().unwrap().pending[0];
    assert!(pending.id.is_none());
    assert_eq!(pending.deadline, invocation.deadline_ms);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.remote.response_for_id_for_test(&invocation.id).is_none());
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    tick(&mut runtime, &mut app);
    assert!(matches!(mailbox().unwrap().0, Request::Indexer(IndexerRequest::Configure { id, .. }) if id == "queued"));
    assert!(runtime.companion_adapter.as_ref().unwrap().pending[0].id.is_some());
    assert_eq!(runtime.companion_adapter.as_ref().unwrap().pending[0].deadline, invocation.deadline_ms);
    tick(&mut runtime, &mut app);
    assert!(mailbox().is_none(), "ready transition submits the accepted request only once");
}

#[test]
fn queued_configuration_is_retired_before_stop_or_invalid_config_can_restart_helper() {
    let _serial = plx_base::testlock::serial();
    for invalid in [false, true] {
        let (mut runtime, mut app) = waiting_lifecycle_fixture();
        let invocation = queue_remote_configuration(&mut runtime, &mut app);
        let cancel = runtime.companion_adapter.as_ref().unwrap().pending[0].cancelled.clone();
        if invalid {
            runtime.saved.settings.insert(SETTINGS_KEY.into(), json!({"enabled":"invalid"}));
        } else {
            let mut config = runtime.companion_adapter.as_ref().unwrap().applied.clone().unwrap();
            config.enabled = false;
            save_configuration(&mut runtime.saved, &config).unwrap();
        }
        tick(&mut runtime, &mut app);
        assert!(cancel.load(Ordering::Acquire));
        assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
        let response = runtime.remote.response_for_id_for_test(&invocation.id).expect("retired response");
        assert_eq!(response["id"], invocation.id);
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"]["op"], "configure");
        assert_eq!(response["data"]["runtime"], invocation.params["runtime"]);
        assert_eq!(response["data"]["reason"], "companion_cancelled");
        let config = Configuration { enabled: true, ..Configuration::default() };
        save_configuration(&mut runtime.saved, &config).unwrap();
        tick(&mut runtime, &mut app);
        let mut mailbox = runtime.companion.ready_mailbox_for_test();
        tick(&mut runtime, &mut app);
        assert!(mailbox().is_none(), "the new helper must not receive the old Configure");
        assert_eq!(runtime.remote.response_for_id_for_test(&invocation.id), Some(response));
    }
}

#[test]
fn real_missing_helper_failure_completes_waiting_prepare_and_remote_before_timeout() {
    use plx_base::task::time::Budget;
    use std::time::Duration;
    let _serial = plx_base::testlock::serial();
    for remote in [false, true] {
        let (mut runtime, mut app) = waiting_lifecycle_fixture();
        let invocation = if remote {
            Some(queue_remote_configuration(&mut runtime, &mut app))
        } else {
            let mut adapter = runtime.companion_adapter.take().unwrap();
            adapter.begin_media(&mut runtime, &mut app, "Pending film".into(),
                Request::Indexer(IndexerRequest::Prepare { source_id: "home".into(), result_ref: "a".repeat(32) }),
                Target::Prepare).unwrap();
            runtime.companion_adapter = Some(adapter);
            None
        };
        let pending = &runtime.companion_adapter.as_ref().unwrap().pending[0];
        assert!(pending.id.is_none());
        let deadline = pending.deadline;
        let cancel = pending.cancelled.clone();
        // Replace only the fixture's inert transport with the real producer of a
        // missing-binary error. This tests error consumption, not startup racing.
        let root = plx_base::testscratch::process_dir("torrent-queued-missing-helper").canonicalize().unwrap();
        let config = runtime.companion_adapter.as_ref().unwrap().applied.clone().unwrap();
        runtime.companion = Supervisor::new(root.clone(), root.join("ott-companion"), config);
        let off = plx_base::task::OffFrame::for_test();
        let budget = Budget::start(&off, Duration::from_secs(3));
        while runtime.companion.snapshot().error.is_none() && !budget.expired(&off) {
            budget.wait(&off, Duration::from_millis(5), Duration::from_millis(5), &|| false);
        }
        let error = runtime.companion.snapshot().error;
        runtime.clock_sample = Some(ClockSample::capture(&off).unwrap());
        tick(&mut runtime, &mut app);
        let pending_empty = runtime.companion_adapter.as_ref().unwrap().pending.is_empty();
        let response = invocation.as_ref().and_then(|inv| runtime.remote.response_for_id_for_test(&inv.id));
        let retry = if remote { false } else { view(&runtime).unwrap().rows.iter().any(|row| row.id == "torrent.retry" && row.enabled) };
        let status = runtime.status.clone();
        let no_session = runtime.companion_adapter.as_ref().unwrap().session.is_none();
        // Existing retirement owns and joins the supervisor worker before its
        // private root is removed, including the no-worker failure path.
        let mut stop = runtime.companion.retire_for_erasure();
        let cleanup = Budget::start(&off, Duration::from_secs(3));
        let clean = loop {
            let result = stop.finish(cleanup.remaining(&off));
            if result.is_ok() || cleanup.expired(&off) { break result; }
            cleanup.wait(&off, Duration::from_millis(5), Duration::from_millis(5), &|| false);
        };
        assert!(clean.is_ok(), "existing supervisor retirement must finish: {clean:?}");
        plx_base::testscratch::remove_tree(&root);
        let error = error.expect("real missing-binary validation or worker-unavailable error");
        assert!(matches!(error, "companion_not_installed" | "companion_worker_unavailable"));
        assert!(runtime.now_ms() < deadline, "this must not be the ordinary 40-second timeout");
        assert!(pending_empty && cancel.load(Ordering::Acquire));
        assert!(no_session);
        if let Some(invocation) = invocation {
            let response = response.expect("terminal failure response");
            assert_eq!(response["id"], invocation.id);
            assert_eq!(response["data"]["runtime"], invocation.params["runtime"]);
            assert_eq!(response["data"]["op"], "configure");
            assert_eq!(response["data"]["reason"], error);
        } else {
            assert_eq!(status, error);
            assert!(retry, "the failed pending start must expose its existing explicit retry");
        }
    }
}

#[test]
fn queued_remote_is_not_skipped_when_media_retirement_removes_an_earlier_entry() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app) = waiting_lifecycle_fixture();
    let mut mailbox = runtime.companion.ready_mailbox_for_test();
    runtime.page = "torrent-files";
    let mut adapter = runtime.companion_adapter.take().unwrap();
    adapter.ui_request(&mut runtime, Request::Session { id: "lease-1".into() }, Target::Session).unwrap();
    assert!(matches!(mailbox().unwrap().0, Request::Session { .. }));
    let supervised_cancel = adapter.pending[0].cancelled.clone();
    runtime.companion.stop();
    adapter.ui_request(&mut runtime, Request::Indexer(IndexerRequest::Prepare {
        source_id: "home".into(), result_ref: "a".repeat(32),
    }), Target::Prepare).unwrap();
    runtime.companion_adapter = Some(adapter);
    let mut invocation = runtime.remote.request_for_test("torrent_indexer", json!({
        "version":1,"runtime":runtime.remote.runtime(),"op":"configure","sourceId":"queued",
        "label":"Queued","endpoint":"http://127.0.0.1:9117/api","apiKey":"private-key"
    }));
    invocation.deadline_ms = runtime.now_ms().saturating_add(UI_TIMEOUT);
    assert!(command(&mut runtime, &mut app, &invocation).is_none());
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.pending.len(), 3);
    assert!(adapter.pending[0].id.is_some());
    assert!(adapter.pending[1].id.is_none() && adapter.pending[2].id.is_none());
    let remote_cancel = adapter.pending[2].cancelled.clone();
    let config = Configuration { enabled: false, ..Configuration::default() };
    save_configuration(&mut runtime.saved, &config).unwrap();
    tick(&mut runtime, &mut app);
    assert!(supervised_cancel.load(Ordering::Acquire));
    assert!(remote_cancel.load(Ordering::Acquire));
    assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
    let response = runtime.remote.response_for_id_for_test(&invocation.id).expect("queued remote retired");
    assert_eq!(response["data"]["reason"], "companion_cancelled");
    assert_eq!(response["data"]["runtime"], invocation.params["runtime"]);
    assert!(mailbox().is_none());
}

fn sparse_file_session(state: &str, selected: Option<usize>) -> Session {
    let mut value = json!({"id":"lease-1", "state":state, "selected":selected,
        "files":[{"index":7,"name":"First.mp4","length":4096},
            {"index":23,"name":"Second.mp4","length":8192}]});
    if state == "ready" { value["streamPath"] = json!("sessions/lease-1/stream"); }
    crate::app::ott_companion::protocol::session_for_test(value).unwrap()
}

// Keep the large UI owner on the heap: nested by-value fixture returns plus
// the real numeric-input dispatch exceed the default test thread stack in
// unoptimized builds. Runtime and all input paths remain the production types.
fn generic_file_fixture(provider: &'static str) -> (Runtime, Box<App>, String) {
    let (mut runtime, app, mut adapter) = fixture();
    let app = Box::new(app);
    runtime.page = "torrent-files";
    adapter.media_provider = provider;
    adapter.session = Some(sparse_file_session("select", None));
    adapter.retry_media = Some(("Original source".into(), Request::Create {
        source: CreateSource::Direct("magnet:?xt=urn:btih:0123456789012345678901234567890123456789&tr=https%3A%2F%2Ftracker.example".into()),
        file_index: None,
    }, false));
    let key = format!("torrent.file:{}:lease-1:23", adapter.generation);
    runtime.companion_adapter = Some(adapter);
    (runtime, app, key)
}

#[test]
fn generic_torrent_file_rows_preserve_sparse_identity_without_invented_metadata() {
    let _serial = plx_base::testlock::serial();
    let (runtime, _app, key) = generic_file_fixture("torrent");
    let view = view(&runtime).unwrap();
    let files: Vec<_> = view.rows.iter().filter(|row| row.id.starts_with("torrent.file:")).collect();
    assert_eq!(files.len(), 2);
    let row = files[1];
    assert_eq!(row.id, key);
    assert!(matches!(&row.action, OttAction::Play(id) if id == &key));
    assert_eq!(row.title, "2  Second.mp4");
    assert_eq!(row.content_title.as_deref(), Some("Second.mp4"));
    assert_eq!(row.description, row.detail);
    assert!(row.enabled && !row.favorite);
    assert!(row.media.is_none() && row.poster.is_none() && row.guide.is_none());
}

#[test]
fn generic_torrent_file_play_routes_once_for_torrent_and_stremio_without_replacing_source() {
    let _serial = plx_base::testlock::serial();
    for provider in ["torrent", "stremio"] {
        let (mut runtime, mut app, key) = generic_file_fixture(provider);
        let saved = json!(runtime.saved);
        let generation = runtime.play_generation;
        let selected = runtime.selected.clone();
        let route = plx_media::route::worker_ticket();
        super::super::apply_ui_action(&mut runtime, &mut app, OttAction::Play(key.clone()), true, true, false);
        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert_eq!(adapter.pending.len(), 1);
        assert!(matches!(&adapter.pending[0].request, Request::Select { id, file_index: 23 } if id == "lease-1"));
        assert_eq!(adapter.session.as_ref().unwrap().selected, None);
        assert_eq!(adapter.media_provider, provider);
        assert!(matches!(&adapter.retry_media, Some((_, Request::Create {
            source: CreateSource::Direct(source), file_index: None }, false))
            if source.ends_with("&tr=https%3A%2F%2Ftracker.example")));
        assert_eq!(json!(runtime.saved), saved);
        assert_eq!(runtime.play_generation, generation);
        assert_eq!(runtime.selected, selected);
        assert_eq!(plx_media::route::worker_ticket(), route);
        assert!(!runtime.saving && !adapter.inspecting);
        assert!(super::super::handle_action(&mut runtime, &mut app, OttAction::Play(key)).is_err());
        assert_eq!(runtime.companion_adapter.as_ref().unwrap().pending.len(), 1);
    }
}

#[test]
fn generic_torrent_file_play_preserves_legacy_action() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, key) = generic_file_fixture("torrent");
    super::super::handle_action(&mut runtime, &mut app, OttAction::EditSetting(key)).unwrap();
    assert!(matches!(&runtime.companion_adapter.as_ref().unwrap().pending[0].request,
        Request::Select { file_index: 23, .. }));
}

// A separate test frame keeps the legacy App/Runtime fixture out of these cases.
#[test]
fn generic_torrent_file_play_preserves_all_file_scope_guards() {
    let _serial = plx_base::testlock::serial();
    for case in 0..5 {
        let (mut runtime, mut app, mut key) = generic_file_fixture("torrent");
        match case {
            0 => runtime.page = "torrent-results",
            1 => runtime.companion_adapter.as_mut().unwrap().generation += 1,
            2 => runtime.companion_adapter.as_mut().unwrap().session.as_mut().unwrap().id = "lease-2".into(),
            3 => key = key.strip_suffix("23").unwrap().to_owned() + "99",
            _ => runtime.companion_adapter.as_mut().unwrap().session.as_mut().unwrap().state = SessionState::Preparing,
        }
        let saved = json!(runtime.saved);
        assert!(super::super::handle_action(&mut runtime, &mut app, OttAction::Play(key)).is_err(), "case {case}");
        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(adapter.pending.is_empty() && !adapter.inspecting);
        assert_eq!(adapter.media_deadline, 0);
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[test]
fn generic_torrent_file_play_cannot_bypass_normal_kiosk_source_replacement_fence() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, key) = generic_file_fixture("torrent");
    let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
    runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":false,
        "origin":origin,"target":{"kind":"item","entry":{"title":"Fixture channel",
            "origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
    let saved = json!(runtime.saved);
    assert!(input_policy::ui(&runtime, &OttAction::Play(key.clone())).is_ok(), "normal pin allows navigation");
    let error = input_policy::replace_source(&runtime).unwrap_err();
    assert_eq!(super::super::handle_action(&mut runtime, &mut app, OttAction::Play(key)), Err(error));
    assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
    assert_eq!(json!(runtime.saved), saved);
}

#[test]
fn generic_torrent_file_play_preserves_the_parental_settings_gate() {
    let _serial = plx_base::testlock::serial();
    for legacy in [false, true] {
        let (mut runtime, mut app, key) = generic_file_fixture("torrent");
        runtime.entropy = Some([7; 16]);
        runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        runtime.saved.settings.insert("psChannels".into(), json!(0));
        runtime.saved.settings.insert("psOptions".into(), json!(1));
        assert!(runtime.require_parental(parental::Target::Channel { marked: true }).is_ok());
        assert!(runtime.require_parental(parental::Target::Settings).is_err());
        let saved = json!(runtime.saved);
        // No screen owner is fabricated: both routes must reach the same real
        // Settings gate, which refuses to open an unowned PIN editor.
        let action = if legacy { OttAction::EditSetting(key) } else { OttAction::Play(key) };
        assert_eq!(super::super::handle_action(&mut runtime, &mut app, action),
            Err("Open parental access from the current Settings screen."));
        assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[test]
fn generic_torrent_favorite_refusal_is_explicit_and_does_not_write_saved() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, mut app, key) = generic_file_fixture("torrent");
    for id in [key, "torznab:1:home:result".into(), "torrent:torznab:1:home:result".into()] {
        let saved = json!(runtime.saved);
        assert_eq!(super::super::handle_action(&mut runtime, &mut app, OttAction::ToggleFavorite(id)),
            Err("Favorites are unavailable for Torrent sources"));
        assert_eq!(json!(runtime.saved), saved);
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
        assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
    }
}

#[test]
fn generic_torrent_select_reply_rejects_wrong_file_identity_before_inspect() {
    let _serial = plx_base::testlock::serial();
    for case in 0..5 {
        let (mut runtime, mut app, key) = generic_file_fixture("torrent");
        super::super::handle_action(&mut runtime, &mut app, OttAction::Play(key)).unwrap();
        let mut adapter = runtime.companion_adapter.take().unwrap();
        let pending = adapter.pending.pop().unwrap();
        let now = runtime.now_ms();
        let mut reply = sparse_file_session("ready", Some(23));
        match case {
            0 => reply.selected = Some(7),
            1 => reply.id = "lease-2".into(),
            2 => reply.files[1].name = "Different.mp4".into(),
            3 => reply.files[1].length += 1,
            _ => reply.files[1].index = 24,
        }
        let saved = json!(runtime.saved);
        adapter.complete_ui_reply(&mut runtime, &mut app, pending, Ok(Reply::Session(reply)), now);
        assert_eq!(runtime.status, "Torrent file selection changed", "case {case}");
        assert!(!adapter.inspecting && adapter.session.is_none());
        assert!(adapter.worker_done.load(Ordering::Acquire));
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[test]
fn generic_torrent_select_reply_accepts_exact_file_and_preserves_backend_errors() {
    let _serial = plx_base::testlock::serial();
    for failed in [false, true] {
        let (mut runtime, mut app, key) = generic_file_fixture("torrent");
        super::super::handle_action(&mut runtime, &mut app, OttAction::Play(key)).unwrap();
        let mut adapter = runtime.companion_adapter.take().unwrap();
        let pending = adapter.pending.pop().unwrap();
        let now = runtime.now_ms();
        let reply = if failed {
            crate::app::ott_companion::protocol::session_for_test(json!({
                "id":"lease-1","state":"error","files":[],"error":"torrent_unavailable"
            })).unwrap()
        } else { sparse_file_session("preparing", Some(23)) };
        let expected_error = reply.error.map(Error::code);
        adapter.complete_ui_reply(&mut runtime, &mut app, pending, Ok(Reply::Session(reply)), now);
        if failed {
            assert_eq!(runtime.status, expected_error.unwrap());
            assert!(adapter.session.is_none());
        } else {
            assert_eq!(adapter.session.as_ref().unwrap().selected, Some(23));
            assert_eq!(adapter.session.as_ref().unwrap().files[1].name, "Second.mp4");
        }
        assert!(!adapter.inspecting && adapter.worker_done.load(Ordering::Acquire));
    }
}

#[test]
fn generic_torrent_file_digit_dispatches_the_rendered_choice_once_and_rejects_changed_session() {
    let _serial = plx_base::testlock::serial();
    for changed in [false, true] {
        let (mut runtime, mut app, _) = generic_file_fixture("torrent");
        let session = runtime.companion_adapter.as_mut().unwrap().session.as_mut().unwrap();
        session.files[0].index = 4;
        session.files[1].index = 19;
        runtime.resume_source_work();
        app.running = true;
        app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
        bridge::show_page(&mut app.pages, AppArg::Ott);
        for n in 1..=24 {
            bridge::frame(&mut app.pages, &mut app.bridge, plx_machine::machine::Tick {
                ms: n * 16, dt_us: 16_000,
            }, Vec::new());
        }
        runtime.reply = app.pages.top_page();
        assert!(runtime.reply.is_some());
        runtime.publish(&mut app);
        for n in 25..=48 {
            bridge::frame(&mut app.pages, &mut app.bridge, plx_machine::machine::Tick {
                ms: n * 16, dt_us: 16_000,
            }, Vec::new());
        }
        let mut mailbox = runtime.companion.ready_mailbox_for_test();
        let saved = json!(runtime.saved);
        let now = runtime.now_ms();
        app.ott = Some(runtime);
        app.ev = crate::app::events::encode_key(u32::from(b'2'), 0, true);
        assert!(numeric_input::ingest(&mut app));
        app.ev = crate::app::events::encode_key(u32::from(b'2'), 0, false);
        assert!(numeric_input::ingest(&mut app));
        let mut runtime = app.ott.take().unwrap();
        assert!(mailbox().is_none(), "a digit is not an immediate selection");
        if changed {
            runtime.companion_adapter.as_mut().unwrap().session.as_mut().unwrap().id = "lease-2".into();
        }
        numeric_input::tick(&mut runtime, &mut app, now + 2_000);
        if changed {
            assert!(mailbox().is_none(), "a new session cannot inherit the rendered digit");
            assert!(runtime.companion_adapter.as_ref().unwrap().pending.is_empty());
        } else {
            assert!(matches!(mailbox().unwrap().0, Request::Select { id, file_index: 19 } if id == "lease-1"));
            assert_eq!(runtime.companion_adapter.as_ref().unwrap().pending.len(), 1);
        }
        numeric_input::tick(&mut runtime, &mut app, now + 4_000);
        assert!(mailbox().is_none());
        assert_eq!(json!(runtime.saved), saved);
        assert!(!runtime.saving);
    }
}

#[test]
fn companion_native_load_requires_its_verified_receipt_and_current_source_scope() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetSubtitleAdmission;
    for mutation in 0..8 {
        let (mut runtime, mut app, mut adapter) = fixture();
        app.running = true;
        runtime.entropy = Some([7; 16]);
        adapter.media_generation = runtime.play_generation;
        plx_media::route::cancel_play(&mut app.player.session);
        plx_media::route::reset_player_control_for_test(&app.player.session);
        let source = arm_direct_subtitle_admission(&mut adapter, "https://media.invalid:443/receipt.mp4");
        adapter.admit_playback(&mut runtime, &mut app, source.clone(), subtitle_admission_media()).unwrap();
        let identity = plx_media::route::playback_identity(&app.player.session).unwrap();
        let ticket = plx_media::route::play_request_ticket();
        runtime.companion_adapter = Some(adapter);
        assert_eq!(native_start_admission(&mut runtime, &app), Some(Ok(())));
        match mutation {
            0 => { runtime.saved.settings.insert(REVISION_KEY.into(), json!(1)); }
            1 => { runtime.companion_adapter.as_mut().unwrap().generation += 1; }
            2 => { runtime.play_generation += 1; }
            3 => { runtime.companion_adapter.as_mut().unwrap().cancelled.store(true, Ordering::Relaxed); }
            4 => { runtime.companion_adapter.as_mut().unwrap().direct_source = Some(
                plx_media::ff::ExternalSource::Http("https://media.invalid:443/replaced.mp4".into())); }
            5 => { runtime.saved.settings.insert("psOptions".into(), json!(1)); }
            6 => { runtime.companion_adapter.as_mut().unwrap().session = Some(select_session()); }
            _ => { runtime.companion_adapter.as_mut().unwrap().route_ticket = None;
                runtime.companion_adapter.as_mut().unwrap().native_admission = None; }
        }
        assert!(!matches!(native_start_admission(&mut runtime, &app), Some(Ok(()))), "mutation {mutation}");
        retire_native_start(&mut runtime, &app, &identity, ticket);
        assert!(runtime.companion_adapter.as_ref().unwrap().native_admission.is_none());
    }
}

#[test]
fn arbitrary_external_video_cannot_acquire_companion_native_load_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetSubtitleAdmission;
    let (mut runtime, mut app, adapter) = fixture();
    plx_media::route::cancel_play(&mut app.player.session);
    plx_media::route::request_external_source(&mut app.player.session, &subtitle_admission_media(),
        plx_media::ff::ExternalSource::Http("https://media.invalid:443/unowned.mp4".into()),
        "Fixture", "Video", playback::start_caps()).unwrap();
    runtime.companion_adapter = Some(adapter);
    assert!(native_start_admission(&mut runtime, &app).is_none());
}

#[test]
fn generic_play_ui_ownership_requires_an_actual_current_adapter_row() {
    let _serial = plx_base::testlock::serial();
    let (mut runtime, _app, key) = generic_file_fixture("torrent");
    assert!(owns_generic_play(&runtime, &OttAction::Play(key.clone())));
    let stale = format!("torrent.file:999:lease-1:23");
    assert!(!owns_generic_play(&runtime, &OttAction::Play(stale)));
    runtime.companion_adapter.as_mut().unwrap().session = None;
    assert!(!owns_generic_play(&runtime, &OttAction::Play(key)));
}

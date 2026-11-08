//! Explicit Select after a cold missing drive. Real directory handles and the
//! production browser/Local validators are used with synthetic mount evidence.
//! No worker thread, protected writer, native playback or device is started.
use super::*;

fn request(r: &mut Runtime, id: u64, action: &str, mut params: serde_json::Value) -> Invocation {
    params["runtime"] = json!(r.remote.runtime());
    let now = r.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":action,"params":params,"expires_at":1020
    }]})
    .to_string();
    let mut admitted = r.remote.raw_poll_for_test(&body, now);
    assert_eq!(admitted.len(), 1);
    let invocation = admitted.pop().unwrap();
    assert!(r.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now.saturating_add(20_000));
    invocation
}

fn execute(
    directory: &Directory,
    task: usb::Task,
    cancel: &AtomicBool,
) -> Result<usb::Answer, usb::Error> {
    let off = OffFrame::for_test();
    let mount = directory.mount(&off);
    usb::execute_with_identity(
        &off,
        task,
        cancel,
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, rows, _| {
            Ok(rows
                .iter()
                .map(|row| {
                    identity::IdentityObservation::for_test(
                        row.clone(),
                        identity::StableIdentity::new("fixture-uuid", row.filesystem()).unwrap(),
                    )
                })
                .collect())
        },
    )
}

fn browser_request(
    r: &mut Runtime,
    id: u64,
    params: serde_json::Value,
) -> (Invocation, ott_catalog::Task) {
    let invocation = request(r, id, "local_browser", params);
    r.usb_browser_prepare_for_test(&invocation).unwrap();
    let task = r
        .take_catalog_task()
        .expect("admitted browser task has priority");
    assert!(matches!(&task, ott_catalog::Task::UsbRemote { .. }));
    (invocation, task)
}

fn browse(
    app: &mut App,
    r: &mut Runtime,
    directory: &Directory,
    id: u64,
    params: serde_json::Value,
) -> serde_json::Value {
    let (invocation, task) = browser_request(r, id, params);
    let ott_catalog::Task::UsbRemote {
        serial,
        task,
        cancel,
    } = task
    else {
        unreachable!()
    };
    let result = execute(directory, task, &cancel);
    r.usb_remote_result(app, serial, result);
    let response = r.remote.response_for_id_for_test(&invocation.id).unwrap();
    assert_eq!(response["status"], "ok");
    response
}

fn child(page: &serde_json::Value, name: &str) -> serde_json::Value {
    page["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .expect("exact displayed folder")["id"]
        .clone()
}

fn selection_page(app: &mut App, r: &mut Runtime, directory: &Directory) -> serde_json::Value {
    let roots = browse(app, r, directory, 0xc410, json!({"version":1,"op":"roots"}));
    assert_eq!(roots["data"]["entries"].as_array().unwrap().len(), 1);
    let root = browse(
        app,
        r,
        directory,
        0xc411,
        json!({"version":1,"op":"list",
        "generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}),
    );
    let movies = browse(
        app,
        r,
        directory,
        0xc412,
        json!({"version":1,"op":"list",
        "generation":root["data"]["generation"],"entry_id":child(&root,"Movies")}),
    );
    browse(
        app,
        r,
        directory,
        0xc413,
        json!({"version":1,"op":"list",
        "generation":movies["data"]["generation"],"entry_id":child(&movies,"夏")}),
    )
}

fn select_params(page: &serde_json::Value) -> serde_json::Value {
    json!({"version":1,"op":"select","generation":page["data"]["generation"],
        "select_id":page["data"]["select_id"]})
}

#[test]
fn cold_select_publishes_only_confirmed_relocated_root_before_handle_followup() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for operation in ["play", "connect"] {
        let directory = Directory::new();
        let (mut app, mut r) = idle_cold_owner(true);
        let saved = serde_json::to_value(&r.saved).unwrap();
        let old_path = RememberedFolders::read(&r.saved.settings)
            .unwrap()
            .folders()
            .iter()
            .find(|row| row.id == "root")
            .unwrap()
            .path
            .clone();
        let native = plx_media::route::play_request_ticket();
        let generation = r.generation;
        let provider = r.provider;
        let page_name = r.page;
        let page = selection_page(&mut app, &mut r, &directory);
        assert!(matches!(
            r.local_inventory.phase,
            transient::InventoryPhase::Unloaded
        ));
        let response = browse(&mut app, &mut r, &directory, 0xc414, select_params(&page));
        let handle = response["data"]["source"]["handle"].as_str().unwrap();
        let source = r.usb_remote_handle(handle).unwrap();
        assert_eq!(source.descriptor().id, "root");
        assert_eq!(
            source.descriptor().canonical_path.as_deref(),
            directory.0.join("Movies/夏").to_str()
        );
        assert_ne!(
            source.descriptor().canonical_path.as_deref(),
            Some(old_path.as_str())
        );
        assert_eq!(response["data"]["persisted"], false);
        assert_eq!(response["data"]["activated"], false);
        assert!(
            matches!(r.local_inventory.phase, transient::InventoryPhase::Ready(g) if g == generation)
        );
        assert_eq!(r.local_inventory.sources.len(), 1);
        assert!(r.local_inventory.sources[0].same_grant(&source));
        // This is the disputed boundary: the real scheduler runs before the
        // next handle command. It must not validate every remembered root.
        assert!(
            r.take_catalog_task().is_none(),
            "Select cannot schedule all-root bindings or automatic playback"
        );
        assert!(
            r.local_inventory.usb.delayed_mount.is_none()
                && r.local_inventory.usb.pending.is_none()
        );
        assert_eq!(r.generation, generation);
        assert_eq!(r.provider, provider);
        assert_eq!(r.page, page_name);
        assert!(r.play_target.is_none() && r.pending_play.is_none() && r.playing.is_none());
        assert!(r.transient.accepted.is_none() && r.transient.candidate.is_none());
        assert!(plx_media::route::play_request_is_current(native));
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);

        let invocation = request(
            &mut r,
            0xc415,
            "local_media",
            json!({"op":operation,"source_handle":handle}),
        );
        r.usb_handle_prepare_for_test(&invocation, source.clone())
            .unwrap();
        let Some(ott_catalog::Task::UsbRemote {
            serial,
            task,
            cancel,
        }) = r.take_catalog_task()
        else {
            panic!("handle verification task")
        };
        let result = execute(&directory, task, &cancel);
        assert!(
            matches!(&result, Ok(usb::Answer::Verified(verified)) if verified.same_grant(&source))
        );
        r.catalog_done.store(false, Ordering::Release); // keep the real command's launcher held
        r.usb_remote_result(&mut app, serial, result);
        let Some(ott_catalog::Task::LocalBindings(binding)) = transient::take_binding_task(&mut r)
        else {
            panic!("verified handle reaches ordinary Local binding")
        };
        assert!(!binding.allow_new && !binding.allow_reconnect && !binding.restore_remembered);
        let moved = binding
            .remembered
            .iter()
            .find(|row| row.id == "root")
            .unwrap();
        assert_eq!(moved.path, directory.0.join("Movies/夏").to_str().unwrap());
        let validated = ott_catalog::validate_local_bindings_observed_for_test(
            &OffFrame::for_test(),
            binding,
            &r.cancel,
            &mut |off, _| Ok(vec![directory.mount(off)]),
        )
        .unwrap();
        assert!(validated.operation_error.is_none());
        assert_eq!(
            validated
                .sources
                .iter()
                .filter(|source| source.is_live())
                .count(),
            1
        );
        assert!(validated
            .sources
            .iter()
            .any(|checked| checked.same_grant(&source)));
        assert!(
            validated
                .sources
                .iter()
                .find(|source| source.descriptor().id == "unrelated")
                .is_some_and(|source| !source.is_live()),
            "remembered metadata is not another grant"
        );
        assert_eq!(
            serde_json::to_value(&r.saved).unwrap(),
            saved,
            "only a subsequent durable ACK may persist relocation"
        );
        assert!(plx_media::route::play_request_is_current(native));
        assert!(r.play_target.is_none() && r.playing.is_none());
        // Deliberately stop before binding_result: Connect would admit the
        // protected writer there; this source-only fixture claims no disk ACK.
    }
}

#[test]
fn failed_stale_or_expired_cold_select_cannot_publish_ready_inventory() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for failure in ["failed", "stale", "expired"] {
        let directory = Directory::new();
        let (mut app, mut r) = idle_cold_owner(true);
        let saved = serde_json::to_value(&r.saved).unwrap();
        let page = selection_page(&mut app, &mut r, &directory);
        let (invocation, task) = browser_request(&mut r, 0xc414, select_params(&page));
        let ott_catalog::Task::UsbRemote {
            serial,
            task,
            cancel,
        } = task
        else {
            unreachable!()
        };
        let result = if failure == "failed" {
            Err(usb::Error::Unavailable)
        } else {
            let result = execute(&directory, task, &cancel);
            assert!(matches!(&result, Ok(usb::Answer::ConfirmedFolder(_))));
            if failure == "stale" {
                r.accepted_navigation(false);
            } else {
                r.expire_usb_select_for_test();
                r.usb_remote_poll();
            }
            result
        };
        r.usb_remote_result(&mut app, serial, result);
        let response = r.remote.response_for_id_for_test(&invocation.id).unwrap();
        assert_eq!(response["status"], "rejected");
        assert!(matches!(
            r.local_inventory.phase,
            transient::InventoryPhase::Unloaded
        ));
        assert!(r.local_inventory.sources.is_empty());
        assert!(r.play_target.is_none() && r.playing.is_none());
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    }
}

#[test]
fn noncold_select_preserves_existing_inventory_phase() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for phase in 0..4 {
        let directory = Directory::new();
        let mut saved = saved_intent();
        saved.favorites_selected = true; // real initial-load exclusion, no cold authority
        let (mut app, mut r) = loaded(saved);
        release_slots(&mut r);
        r.catalog_next = None;
        r.catalog_loading = false; // model settled non-cold catalogue work
        assert!(!r.local_inventory.usb.waiting && r.local_inventory.usb.pending.is_none());
        let generation = r.generation;
        r.local_inventory.phase = match phase {
            0 => transient::InventoryPhase::Unloaded,
            1 => transient::InventoryPhase::Ready(generation),
            2 => transient::InventoryPhase::Failed,
            _ => transient::InventoryPhase::Loading(generation),
        };
        let page = selection_page(&mut app, &mut r, &directory);
        let response = browse(&mut app, &mut r, &directory, 0xc414, select_params(&page));
        assert_eq!(response["data"]["activated"], false);
        assert!(match (phase, &r.local_inventory.phase) {
            (0, transient::InventoryPhase::Unloaded) | (2, transient::InventoryPhase::Failed) =>
                true,
            (1, transient::InventoryPhase::Ready(g))
            | (3, transient::InventoryPhase::Loading(g)) => *g == generation,
            _ => false,
        });
    }
}

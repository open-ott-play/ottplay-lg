//! Existing-command boundary regressions for receiver-owned USB browsing.
//! Temporary-directory fixtures use the existing filesystem validator/walker.
//! No live mount probe, decoder, persistence writer or network is started.
use super::*;
use crate::app::ott_local_session as local;

fn fixture() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.remote = Remote::disabled_for_test("dev_browser", "browser-fixture");
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    let mut profile = synthetic_profile();
    profile.kind = ProviderKind::M3u;
    profile.source = "https://browser.invalid/television.m3u".into();
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles = vec![profile];
    runtime.provider = ProviderKind::M3u;
    runtime.page = "channels";
    runtime.generation = 7;
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    app
}

fn state(runtime: &Runtime) -> Value {
    json!({
        "saved": serde_json::to_value(&runtime.saved).unwrap(),
        "provider": format!("{:?}", runtime.provider),
        "page": runtime.page,
        "generation": runtime.generation,
        "play_generation": runtime.play_generation,
        "playback_epoch": runtime.playback_epoch,
        "navigation_epoch": runtime.navigation_epoch,
        "catalog_loading": runtime.catalog_loading,
        "catalog_complete": runtime.catalog_complete,
        "saving": runtime.saving,
    })
}

fn reject(app: &mut App, params: Value, expired: bool, code: &str) {
    let mut runtime = app.ott.take().unwrap();
    let before = state(&runtime);
    let cancel = runtime.cancel.clone();
    let cancelled = cancel.load(Ordering::Acquire);
    let mut invocation = runtime.remote.request_for_test("local_browser", params);
    invocation.deadline_ms = if expired {
        runtime.now_ms()
    } else {
        runtime.now_ms().saturating_add(20_000)
    };
    let response = commands::command(&mut runtime, app, &invocation)
        .expect("rejected browsing must settle synchronously");
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["version"], 1);
    assert_eq!(response["data"]["runtime"], invocation.params["runtime"]);
    assert_eq!(response["data"]["state"], "error");
    assert_eq!(response["data"]["error"], code);
    assert_eq!(
        state(&runtime),
        before,
        "rejected browsing cannot mutate the active source"
    );
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert_eq!(cancel.load(Ordering::Acquire), cancelled);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(
        !runtime.catalog_done.load(Ordering::Acquire),
        "rejection cannot fabricate worker retirement"
    );
    app.ott = Some(runtime);
}

#[test]
fn local_browser_unknown_paths_and_fields_cannot_grant_receiver_access() {
    let _serial = plx_base::testlock::serial();
    for fields in [
        json!({"path":"/tmp/usb/fixture/Movies"}),
        json!({"source":"/private/Movies"}),
        json!({"mount_id":"fixture"}),
        json!({"uuid":"fixture"}),
        json!({"limit":65}),
    ] {
        let mut app = fixture();
        let mut request = json!({"version":1,"runtime":"browser-fixture","op":"roots"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        reject(&mut app, request, false, "invalid_request");
    }
}

#[test]
fn local_browser_list_requires_exactly_one_issued_entry_or_cursor() {
    let _serial = plx_base::testlock::serial();
    for fields in [
        json!({}),
        json!({"entry_id":"a".repeat(64),"cursor":"b".repeat(64)}),
        json!({"entry_id":42}),
        json!({"entry_id":"a".repeat(64),"index":0}),
    ] {
        let mut app = fixture();
        let mut request = json!({"version":1,"runtime":"browser-fixture","op":"list","generation":"c".repeat(64)});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        reject(&mut app, request, false, "invalid_request");
    }
}

#[test]
fn local_browser_wrong_runtime_and_expired_request_cannot_start_discovery() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    reject(
        &mut app,
        json!({"version":1,"runtime":"previous-runtime","op":"roots"}),
        false,
        "stale",
    );
    let mut app = fixture();
    reject(
        &mut app,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
        true,
        "expired",
    );
}

#[test]
fn local_browser_roots_does_not_interrupt_an_unrelated_catalogue_worker() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    reject(
        &mut app,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
        false,
        "busy",
    );
}

#[test]
fn local_browser_unloaded_receiver_does_not_advertise_browser_capability() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.loaded = false;
    let mut invocation = runtime.remote.request_for_test("capabilities", json!({}));
    invocation.deadline_ms = runtime.now_ms().saturating_add(20_000);
    let response = commands::command(&mut runtime, &mut app, &invocation).unwrap();
    assert_eq!(response["status"], "ok");
    assert!(response["data"].get("local_browser").is_none());
}

struct Disk(std::path::PathBuf, bool);
impl Disk {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "local-browser-rpc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap(), false)
    }
    fn mount(&self) -> plx_platform::tv::removable::MountCandidate {
        use plx_platform::tv::removable::{identity, MountCandidate};
        let file = std::fs::File::open(&self.0).unwrap();
        let device =
            identity::directory_device(&plx_base::task::OffFrame::for_test(), &file).unwrap();
        MountCandidate::directory_device_for_test(self.0.to_str().unwrap(), 8, device).unwrap()
    }
    fn execute(
        &self,
        task: crate::app::ott_usb::Task,
        cancel: &AtomicBool,
    ) -> Result<crate::app::ott_usb::Answer, crate::app::ott_usb::Error> {
        crate::app::ott_usb::execute_with_identity(
            &plx_base::task::OffFrame::for_test(),
            task,
            cancel,
            &mut |_, _| Ok(vec![self.mount()]),
            &mut |_, _, _| {
                use plx_platform::tv::removable::identity::{
                    IdentityError, IdentityObservation, StableIdentity,
                };
                if self.1 {
                    let mount = self.mount();
                    Ok(vec![IdentityObservation::for_test(
                        mount.clone(),
                        StableIdentity::new("fixture-volume-A", mount.filesystem()).unwrap(),
                    )])
                } else {
                    Err(IdentityError::Unsupported)
                }
            },
        )
    }
}
impl Drop for Disk {
    fn drop(&mut self) {
        plx_base::testscratch::remove_tree(&self.0);
    }
}
fn ready() -> App {
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.catalog_loading = false;
    r.catalog_complete = true;
    r.resolver_busy = false;
    r.catalog_done.store(true, Ordering::Release);
    r.metadata_done.store(true, Ordering::Release);
    app
}
fn invocation(r: &mut Runtime, id: u64, action: &str, params: Value) -> Invocation {
    let now = r.now_ms();
    let body=json!({"commands":[],"server_time":1000,"requests":[{"id":format!("{id:032x}"),"action":action,"params":params,"expires_at":1020}]}).to_string();
    let mut inv = r.remote.raw_poll_for_test(&body, now).remove(0);
    inv.deadline_ms = now + 20_000;
    inv
}
fn browse(app: &mut App, disk: &Disk, id: u64, params: Value) -> Value {
    let mut r = app.ott.take().unwrap();
    let before = state(&r);
    let inv = invocation(&mut r, id, "local_browser", params);
    r.usb_browser_prepare_for_test(&inv).unwrap();
    let ott_catalog::Task::UsbRemote {
        serial,
        task,
        cancel,
    } = r.take_usb_remote_task().unwrap()
    else {
        panic!("existing USB worker task")
    };
    assert!(r.usb_remote_waiting_result());
    let result = disk.execute(task, &cancel);
    r.usb_remote_result(app, serial, result);
    let response = r.remote.response_for_id_for_test(&inv.id).unwrap();
    assert_eq!(
        state(&r),
        before,
        "browsing cannot change saved settings, active catalogue or playback generation"
    );
    assert!(!r.usb_remote_waiting_result());
    app.ott = Some(r);
    response
}
fn selected(app: &mut App, disk: &Disk) -> Value {
    let roots = browse(
        app,
        disk,
        1,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
    );
    let list = browse(
        app,
        disk,
        2,
        json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}),
    );
    browse(
        app,
        disk,
        3,
        json!({"version":1,"runtime":"browser-fixture","op":"select","generation":list["data"]["generation"],"select_id":list["data"]["select_id"]}),
    )
}
#[test]
fn local_browser_pages_are_bounded_opaque_and_never_silent_rescans() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    for n in 0..130 {
        std::fs::create_dir(disk.0.join(format!("Folder {n:03}"))).unwrap();
    }
    let mut app = ready();
    let roots = browse(
        &mut app,
        &disk,
        1,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
    );
    let first = browse(
        &mut app,
        &disk,
        2,
        json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}),
    );
    assert_eq!(first["data"]["entries"].as_array().unwrap().len(), 64);
    assert_eq!(first["data"]["partial"], false);
    assert!(!first["data"]["next_cursor"].is_null());
    std::fs::create_dir(disk.0.join("Folder newly added")).unwrap();
    let second = browse(
        &mut app,
        &disk,
        3,
        json!({"version":1,"runtime":"browser-fixture","op":"list","generation":first["data"]["generation"],"cursor":first["data"]["next_cursor"]}),
    );
    assert_ne!(first["data"]["generation"], second["data"]["generation"]);
    assert_eq!(second["data"]["entries"][0]["name"], "Folder 064");
    let last = browse(
        &mut app,
        &disk,
        4,
        json!({"version":1,"runtime":"browser-fixture","op":"list","generation":second["data"]["generation"],"cursor":second["data"]["next_cursor"]}),
    );
    assert_eq!(
        last["data"]["entries"].as_array().unwrap().len(),
        2,
        "cursor uses the accepted prefix, not a new directory scan"
    );
    assert!(last["data"]["next_cursor"].is_null());
    assert!(serde_json::to_vec(&last).unwrap().len() < 64 * 1024);
    let r = app.ott.as_mut().unwrap();
    let stale = invocation(
        r,
        5,
        "local_browser",
        json!({"version":1,"runtime":"browser-fixture","op":"list","generation":first["data"]["generation"],"cursor":first["data"]["next_cursor"]}),
    );
    assert_eq!(r.usb_browser_prepare_for_test(&stale), Err("stale"));
}
#[test]
fn local_browser_select_is_ephemeral_and_survives_browser_replacement() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    let mut app = ready();
    let result = selected(&mut app, &disk);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["persisted"], false);
    assert_eq!(result["data"]["activated"], false);
    assert_eq!(result["data"]["source"]["stable_identity"], false);
    let handle = result["data"]["source"]["handle"].as_str().unwrap();
    assert!(app.ott.as_ref().unwrap().usb_remote_handle(handle).is_ok());
    browse(
        &mut app,
        &disk,
        4,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
    );
    assert!(
        app.ott.as_ref().unwrap().usb_remote_handle(handle).is_ok(),
        "the picker root is not the independently confirmed source grant"
    );
    app.ott.as_mut().unwrap().local_inventory.sources[0].withdraw();
    assert!(app.ott.as_ref().unwrap().usb_remote_handle(handle).is_err());
}
#[test]
fn local_handle_detached_during_passive_preflight_cannot_stop_or_replace_playback() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    let mut app = ready();
    let selected = selected(&mut app, &disk);
    let handle = selected["data"]["source"]["handle"].as_str().unwrap();
    let mut r = app.ott.take().unwrap();
    let before = state(&r);
    let inv = invocation(
        &mut r,
        4,
        "local_media",
        json!({"runtime":"browser-fixture","op":"shuffle","source_handle":handle}),
    );
    let source = r.usb_remote_handle(handle).unwrap();
    r.usb_handle_prepare_for_test(&inv, source).unwrap();
    let ott_catalog::Task::UsbRemote {
        serial,
        task,
        cancel,
    } = r.take_usb_remote_task().unwrap()
    else {
        panic!()
    };
    r.local_inventory.sources[0].withdraw();
    let result = disk.execute(task, &cancel);
    r.usb_remote_result(&mut app, serial, result);
    let response = r.remote.response_for_id_for_test(&inv.id).unwrap();
    assert_eq!(response["status"], "rejected");
    assert_eq!(state(&r), before);
    assert!(r.play_target.is_none() && r.pending_play.is_none());
    assert!(r.transient.candidate.is_none());
}
#[test]
fn local_handle_unknown_replaced_and_expired_requests_fail_before_effects() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    let mut app = ready();
    let selected = selected(&mut app, &disk);
    let handle = selected["data"]["source"]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut r = app.ott.take().unwrap();
    let before = state(&r);
    for (n, token, expired) in [(4, "missing".to_owned(), false), (5, handle.clone(), true)] {
        let mut inv = invocation(
            &mut r,
            n,
            "local_media",
            json!({"runtime":"browser-fixture","op":"play","source_handle":token}),
        );
        if expired {
            inv.deadline_ms = r.now_ms();
        }
        let response = commands::command(&mut r, &mut app, &inv).unwrap();
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"].as_object().unwrap().len(), 5);
        assert_eq!(state(&r), before);
    }
    let old = r.local_inventory.sources.remove(0);
    r.local_inventory
        .sources
        .push(crate::app::ott_local_root::LocalSource::unavailable(
            old.descriptor().clone(),
        ));
    let inv = invocation(
        &mut r,
        6,
        "local_media",
        json!({"runtime":"browser-fixture","op":"play","source_handle":handle}),
    );
    assert_eq!(
        commands::command(&mut r, &mut app, &inv).unwrap()["data"]["error"],
        "source_unavailable"
    );
    assert_eq!(state(&r), before);
    assert!(r.play_target.is_none() && r.pending_play.is_none() && r.transient.candidate.is_none());
}

#[test]
fn local_handle_connect_preserves_binding_and_requires_verified_saved_completion() {
    use crate::app::ott_local_session as local;
    let _serial = plx_base::testlock::serial();
    for saved in [false, true] {
        let mut disk = Disk::new();
        disk.1 = true;
        let mut app = ready();
        let response = selected(&mut app, &disk);
        assert_eq!(response["data"]["source"]["stable_identity"], true);
        let handle = response["data"]["source"]["handle"].as_str().unwrap();
        let mut r = app.ott.take().unwrap();
        let source = r.usb_remote_handle(handle).unwrap();
        let descriptor = source.descriptor().clone();
        let binding = source.usb_witness().unwrap().stable_binding().cloned();
        let remembered = transient::connection_descriptor(
            &descriptor,
            descriptor.canonical_path.clone().unwrap(),
            binding.clone(),
        );
        assert!(remembered.usb == binding);
        assert!(remembered.usb.is_some());
        assert_eq!(remembered.id, descriptor.id);
        let inv = invocation(
            &mut r,
            4,
            "local_media",
            json!({"runtime":"browser-fixture","op":"connect","source_handle":handle}),
        );
        let mut request = local::LocalRequest::parse(&inv.params).unwrap();
        if let Some(local::Selector::Handle { source_id, .. }) = &mut request.selector {
            *source_id = Some(descriptor.id.clone());
        }
        r.transient.candidate = Some(transient::Candidate::Local(transient::LocalCandidate {
            inventory: r.local_inventory.sources.clone(),
            admission_ticket: plx_media::route::play_request_ticket(),
            owner: local::OwnerToken(r.generation),
            pending: local::PendingLocal::new(
                request.clone(),
                local::OwnerToken(r.generation),
                inv.deadline_ms,
            ),
            request,
            purpose: transient::LocalPurpose::Remote(inv.clone()),
            interrupted: transient::InterruptedScan {
                owner: None,
                profile: r.effective_profile().cloned(),
                restart: false,
            },
            phase: transient::CandidatePhase::SavingConnection(descriptor),
        }));
        let permanent = serde_json::to_value(&r.saved).unwrap();
        let provider = r.provider;
        let active = r.saved.profiles.active_profile.clone();
        let result = transient::connection_saved(&mut r, &app, &inv, saved).unwrap();
        assert_eq!(result["data"]["state"] == "ready", saved);
        if saved {
            assert_eq!(result["data"]["source_handle"], handle);
        } else {
            assert_eq!(result["data"].as_object().unwrap().len(), 5);
        }
        assert_eq!(
            serde_json::to_value(&r.saved).unwrap(),
            permanent,
            "only the protected writer may commit Saved"
        );
        assert_eq!(r.provider, provider);
        assert_eq!(r.saved.profiles.active_profile, active);
        assert!(r.play_target.is_none() && r.pending_play.is_none());
    }
}

#[test]
fn local_browser_explicit_more_is_required_for_a_partial_scan() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();
    for n in 0..4100 {std::fs::write(disk.0.join(format!("{n:05}.mp4")),b"").unwrap();}
    let mut app=ready();let roots=browse(&mut app,&disk,1,json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    let first=browse(&mut app,&disk,2,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}));
    assert_eq!(first["data"]["partial"],true);assert_eq!(first["data"]["limit_reached"],false);assert!(first["data"]["next_cursor"].is_null());assert_eq!(first["data"]["entries"].as_array().unwrap().len(),0);
    let more=browse(&mut app,&disk,3,json!({"version":1,"runtime":"browser-fixture","op":"more","generation":first["data"]["generation"],"select_id":first["data"]["select_id"]}));
    assert_eq!(more["data"]["partial"],false);assert_ne!(more["data"]["generation"],first["data"]["generation"]);
    assert!(!more.to_string().contains(disk.0.to_str().unwrap()),"wire rows never disclose the mount path");
}

#[test]
fn local_handle_preflight_completion_is_fenced_by_current_catalogue_owner() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();let mut app=ready();let selected=selected(&mut app,&disk);let handle=selected["data"]["source"]["handle"].as_str().unwrap();
    let mut r=app.ott.take().unwrap();let inv=invocation(&mut r,4,"local_media",json!({"runtime":"browser-fixture","op":"play","source_handle":handle}));
    let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else {panic!()};
    let checked=disk.execute(task,&cancel);r.generation+=1;let before=state(&r);r.usb_remote_result(&mut app,serial,checked);
    assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["error"],"runtime_changed");assert_eq!(state(&r),before);assert!(r.transient.candidate.is_none()&&r.play_target.is_none()&&r.pending_play.is_none());
}

#[test]
fn local_browser_async_stale_owner_keeps_original_request_and_runtime_correlation() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();let mut app=ready();let mut r=app.ott.take().unwrap();
    let inv=invocation(&mut r,7,"local_browser",json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    r.usb_browser_prepare_for_test(&inv).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else {panic!()};
    // A newer accepted owner invalidates this completed scan, but the reply is
    // still addressed to the original request instead of another invocation.
    let result=disk.execute(task,&cancel);r.generation+=1;r.usb_remote_result(&mut app,serial,result);
    let response=r.remote.response_for_id_for_test(&inv.id).unwrap();assert_eq!(response["id"],inv.id);assert_eq!(response["data"]["runtime"],inv.params["runtime"]);assert_eq!(response["data"]["error"],"stale");
    assert!(r.local_inventory.sources.is_empty());
}

#[test]
fn local_browser_respects_the_live_ui_root_picker_after_discovery_completes() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();let mut app=ready();let mut r=app.ott.take().unwrap();
    r.catalog_done.store(false,Ordering::Release);
    r.usb_action(&app,"local.usb").unwrap().unwrap();let generation=r.generation;
    let ott_catalog::Task::Usb(task)=r.take_usb_task().unwrap() else {panic!()};
    let result=disk.execute(task,&AtomicBool::new(false));r.usb_result(&mut app,generation,result);
    r.catalog_done.store(true,Ordering::Release);
    assert!(r.usb_rows().iter().any(|row|row.id.starts_with("local.usb.select:")));
    let before=state(&r);let inv=invocation(&mut r,8,"local_browser",json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    assert_eq!(r.usb_browser_prepare_for_test(&inv),Err("busy"));assert_eq!(state(&r),before);
    // A retained old choice is not forever active after its UI owner leaves.
    r.page="channels";r.provider=ProviderKind::M3u;r.navigation_epoch+=1;
    assert_eq!(r.usb_browser_prepare_for_test(&inv),Ok(()));
}

#[test]
fn local_handle_does_not_schedule_failed_cleanup_before_passive_verification() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();let mut app=ready();let response=selected(&mut app,&disk);let handle=response["data"]["source"]["handle"].as_str().unwrap();let mut r=app.ott.take().unwrap();
    r.local_inventory.proof_cleanup=transient::ProofCleanup::Failed;
    r.catalog_done.store(false,Ordering::Release);
    let inv=invocation(&mut r,4,"local_media",json!({"runtime":"browser-fixture","op":"play","source_handle":handle}));
    assert_eq!(commands::command(&mut r,&mut app,&inv).unwrap()["data"]["error"],"operation_in_progress");
    assert!(matches!(r.local_inventory.proof_cleanup,transient::ProofCleanup::Failed));
    r.catalog_done.store(true,Ordering::Release);let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source).unwrap();
    let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else {panic!()};
    r.local_inventory.sources[0].withdraw();let result=disk.execute(task,&cancel);r.usb_remote_result(&mut app,serial,result);
    assert!(matches!(r.local_inventory.proof_cleanup,transient::ProofCleanup::Failed));assert!(r.transient.candidate.is_none()&&r.play_target.is_none());
}

#[test]
fn local_browser_runtime_drop_cancels_its_private_worker_token() {
    let _serial=plx_base::testlock::serial();let mut app=ready();let mut r=app.ott.take().unwrap();let inv=invocation(&mut r,8,"local_browser",json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    r.usb_browser_prepare_for_test(&inv).unwrap();let ott_catalog::Task::UsbRemote{cancel,..}=r.take_usb_remote_task().unwrap() else {panic!()};
    assert!(!cancel.load(Ordering::Acquire));drop(r);assert!(cancel.load(Ordering::Acquire));
}

#[test]
fn local_handle_early_errors_echo_requested_runtime() {
    let _serial=plx_base::testlock::serial();
    for expired in [false,true] {
        let mut app=ready();let mut r=app.ott.take().unwrap();
        let mut inv=invocation(&mut r,4,"local_media",json!({"runtime":"old-runtime","op":"play","source_handle":"old-handle"}));
        if expired {inv.deadline_ms=r.now_ms();}else{r.saved.settings.insert("nativeKiosk".into(),json!({"malformed":true}));}
        let response=commands::command(&mut r,&mut app,&inv).unwrap();assert_eq!(response["status"],"rejected");assert_eq!(response["data"]["runtime"],"old-runtime");assert_eq!(response["data"].as_object().unwrap().len(),5);
        assert_eq!(response["data"]["error"],if expired{"runtime_changed"}else{"restricted"});
    }
}

struct NativeReset;
impl Drop for NativeReset {fn drop(&mut self){plx_media::player::SHARED.reset_session();plx_media::player::TX.reset();plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);}}

#[test]
fn local_handle_observed_detach_stays_withdrawn_after_directory_returns() {
    let _serial=plx_base::testlock::serial();let _reset=NativeReset;
    for stable in [false,true] {
        let mut disk=Disk::new();disk.1=stable;std::fs::create_dir(disk.0.join("Movies")).unwrap();let mut app=ready();
        let roots=browse(&mut app,&disk,1,json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
        let dirs=browse(&mut app,&disk,2,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}));
        let movies=browse(&mut app,&disk,3,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":dirs["data"]["generation"],"entry_id":dirs["data"]["entries"][0]["id"]}));
        let selected=browse(&mut app,&disk,4,json!({"version":1,"runtime":"browser-fixture","op":"select","generation":movies["data"]["generation"],"select_id":movies["data"]["select_id"]}));
        let handle=selected["data"]["source"]["handle"].as_str().unwrap();
        super::local_integration::install(&mut app,None,"live");
        let identity=plx_media::route::playback_identity(&app.player.session);let native_source=plx_media::route::external_source(&app.player.session);let ticket=plx_media::route::play_request_ticket();
        let mut r=app.ott.take().unwrap();let before=state(&r);let playing=r.playing.clone().unwrap();
        let inv=invocation(&mut r,5,"local_media",json!({"runtime":"browser-fixture","op":"play","source_handle":handle}));let source=r.usb_remote_handle(handle).unwrap();
        r.usb_handle_prepare_for_test(&inv,source).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
        std::fs::rename(disk.0.join("Movies"),disk.0.join("Away")).unwrap();
        let result=disk.execute(task,&cancel);assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::SourceLost(_))));r.usb_remote_result(&mut app,serial,result);
        std::fs::rename(disk.0.join("Away"),disk.0.join("Movies")).unwrap();
        assert!(r.usb_remote_handle(handle).is_err());assert!(!r.local_inventory.sources[0].is_live());assert_eq!(state(&r),before);
        assert_eq!(plx_media::route::playback_identity(&app.player.session),identity);assert_eq!(plx_media::route::external_source(&app.player.session),native_source);assert!(plx_media::route::play_request_is_current(ticket));assert_eq!(r.playing.as_ref().unwrap().identity,playing.identity);assert_eq!(r.playing.as_ref().unwrap().selection_id,playing.selection_id);
    }
}

fn selected_movies(app:&mut App,disk:&Disk)->Value {
    let roots=browse(app,disk,1,json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    let dirs=browse(app,disk,2,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}));
    let movies=browse(app,disk,3,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":dirs["data"]["generation"],"entry_id":dirs["data"]["entries"][0]["id"]}));
    browse(app,disk,4,json!({"version":1,"runtime":"browser-fixture","op":"select","generation":movies["data"]["generation"],"select_id":movies["data"]["select_id"]}))
}
#[test]
fn local_handle_old_loss_cannot_withdraw_a_replacement_or_cancelled_grant() {
    let _serial=plx_base::testlock::serial();
    for replace in [false,true] {
        let disk=Disk::new();std::fs::create_dir(disk.0.join("Movies")).unwrap();let mut app=ready();let response=selected_movies(&mut app,&disk);let handle=response["data"]["source"]["handle"].as_str().unwrap();let mut r=app.ott.take().unwrap();
        let inv=invocation(&mut r,5,"local_media",json!({"runtime":"browser-fixture","op":"shuffle","source_handle":handle}));let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source.clone()).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
        std::fs::rename(disk.0.join("Movies"),disk.0.join("Away")).unwrap();let result=disk.execute(task,&cancel);assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::SourceLost(_))));std::fs::rename(disk.0.join("Away"),disk.0.join("Movies")).unwrap();
        if replace {
            let replacement=crate::app::ott_local_root::LocalSource::acquire_authorized(&plx_base::task::OffFrame::for_test(),source.descriptor().clone(),&mut||true).unwrap().with_usb_witness(source.usb_witness().unwrap().clone());assert!(!replacement.same_grant(&source));r.local_inventory.sources[0]=replacement;
        } else {cancel.store(true,Ordering::Release);}
        r.usb_remote_result(&mut app,serial,result);assert!(r.local_inventory.sources[0].is_live());assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap()["status"],"rejected");
        if replace {assert!(r.usb_remote_handle(handle).is_err());}else{assert!(r.usb_remote_handle(handle).is_ok());}
    }
}
#[test]
fn local_handle_native_playback_replacement_invalidates_completed_preflight() {
    let _serial=plx_base::testlock::serial();let _reset=NativeReset;let disk=Disk::new();let mut app=ready();let response=selected(&mut app,&disk);let handle=response["data"]["source"]["handle"].as_str().unwrap();let mut r=app.ott.take().unwrap();
    let inv=invocation(&mut r,4,"local_media",json!({"runtime":"browser-fixture","op":"play","source_handle":handle}));let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};let result=disk.execute(task,&cancel);
    app.ott=Some(r);super::local_integration::install(&mut app,None,"new-native");let identity=plx_media::route::playback_identity(&app.player.session);r=app.ott.take().unwrap();let before=state(&r);r.usb_remote_result(&mut app,serial,result);
    assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["error"],"runtime_changed");assert_eq!(state(&r),before);assert_eq!(plx_media::route::playback_identity(&app.player.session),identity);assert!(r.transient.candidate.is_none());
}


#[test]
fn local_handle_expiry_poll_keeps_current_loss_evidence_but_never_admits_playback() {
    let _serial=plx_base::testlock::serial(); let _reset=NativeReset;
    for loss in [false,true] {
        let disk=Disk::new(); std::fs::create_dir(disk.0.join("Movies")).unwrap();
        let mut app=ready(); let selected=selected_movies(&mut app,&disk);
        let handle=selected["data"]["source"]["handle"].as_str().unwrap();
        super::local_integration::install(&mut app,None,"live");
        let identity=plx_media::route::playback_identity(&app.player.session);
        let native_source=plx_media::route::external_source(&app.player.session);
        let ticket=plx_media::route::play_request_ticket();
        let mut r=app.ott.take().unwrap(); let before=state(&r);
        let inv=invocation(&mut r,5,"local_media",json!({"runtime":"browser-fixture","op":"play","source_handle":handle}));
        let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source).unwrap();
        let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
        if loss {std::fs::rename(disk.0.join("Movies"),disk.0.join("Away")).unwrap();}
        let result=disk.execute(task,&cancel);
        assert!(if loss {matches!(&result,Ok(crate::app::ott_usb::Answer::SourceLost(_)))}else{matches!(&result,Ok(crate::app::ott_usb::Answer::Verified(_)))});
        r.expire_usb_handle_for_test();r.usb_remote_poll();
        let expired=r.remote.response_for_id_for_test(&inv.id).unwrap();
        assert_eq!(expired["data"]["error"],"runtime_changed");
        assert!(r.usb_remote_waiting_result());assert!(!cancel.load(Ordering::Acquire));
        r.usb_remote_poll();assert!(r.take_usb_remote_task().is_none());
        r.usb_remote_result(&mut app,serial,result);
        if loss {std::fs::rename(disk.0.join("Away"),disk.0.join("Movies")).unwrap();}
        assert_eq!(r.usb_remote_handle(handle).is_ok(),!loss);
        assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap(),expired);
        assert!(!r.usb_remote_waiting_result()); assert!(r.transient.candidate.is_none());
        assert_eq!(state(&r),before);
        assert_eq!(plx_media::route::playback_identity(&app.player.session),identity);
        assert_eq!(plx_media::route::external_source(&app.player.session),native_source);
        assert!(plx_media::route::play_request_is_current(ticket));
    }
}

#[test]
fn local_handle_queued_expiry_does_not_start_or_retain_a_worker() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();let mut app=ready();
    let selected=selected(&mut app,&disk);let handle=selected["data"]["source"]["handle"].as_str().unwrap();
    let mut r=app.ott.take().unwrap();let before=state(&r);
    let inv=invocation(&mut r,4,"local_media",json!({"runtime":"browser-fixture","op":"shuffle","source_handle":handle}));
    let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source).unwrap();
    r.expire_usb_handle_for_test();r.usb_remote_poll();
    assert!(r.take_usb_remote_task().is_none());assert!(!r.usb_remote_waiting_result());
    assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["error"],"runtime_changed");
    assert_eq!(state(&r),before);assert!(r.usb_remote_handle(handle).is_ok());
}

#[test]
fn local_handle_expired_observation_cannot_withdraw_a_replacement() {
    let _serial=plx_base::testlock::serial();let disk=Disk::new();std::fs::create_dir(disk.0.join("Movies")).unwrap();let mut app=ready();
    let selected=selected_movies(&mut app,&disk);let handle=selected["data"]["source"]["handle"].as_str().unwrap();let mut r=app.ott.take().unwrap();
    let inv=invocation(&mut r,5,"local_media",json!({"runtime":"browser-fixture","op":"shuffle","source_handle":handle}));
    let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source.clone()).unwrap();
    let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
    std::fs::rename(disk.0.join("Movies"),disk.0.join("Away")).unwrap();let result=disk.execute(task,&cancel);
    assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::SourceLost(_))));
    r.expire_usb_handle_for_test();r.usb_remote_poll();let response=r.remote.response_for_id_for_test(&inv.id).unwrap();
    std::fs::rename(disk.0.join("Away"),disk.0.join("Movies")).unwrap();
    let replacement=crate::app::ott_local_root::LocalSource::acquire_authorized(&plx_base::task::OffFrame::for_test(),source.descriptor().clone(),&mut||true).unwrap().with_usb_witness(source.usb_witness().unwrap().clone());
    assert!(!replacement.same_grant(&source));r.local_inventory.sources[0]=replacement;
    r.usb_remote_poll();assert!(cancel.load(Ordering::Acquire));
    r.usb_remote_result(&mut app,serial,result);assert!(r.local_inventory.sources[0].is_live());
    assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap(),response);
}


#[test]
fn local_handle_play_and_shuffle_use_exact_grant_and_never_persist_temporary_progress() {
    let _serial=plx_base::testlock::serial();let _reset=NativeReset;
    for operation in ["play","shuffle"] {
        let disk=Disk::new();for name in ["one.mp4","two.mp4","three.mp4"] {std::fs::write(disk.0.join(name),b"listing-only fixture").unwrap();}
        let mut app=ready();let selected=selected(&mut app,&disk);
        let handle=selected["data"]["source"]["handle"].as_str().unwrap();
        super::local_integration::install(&mut app,None,"previous-live");
        let old_identity=plx_media::route::playback_identity(&app.player.session);
        let mut r=app.ott.take().unwrap();
        // Existing permanent offsets must neither select a file nor leak into
        // the independently admitted one-shot Local command.
        let active=r.active().unwrap().clone();
        r.saved.resume.insert(active.id.clone(),SavedResume{entry_id:"old-file".into(),position_ms:47000,source_fingerprint:profile_fingerprint(&active)});
        let favorite=plx_data::ott::favorites::Favorite {
            title:"Saved film".into(),
            origin:plx_data::ott::favorites::Origin{provider:"plex".into(),profile:"Home".into(),subprofile:None,source_alias:"Fixture".into()},
            locator:plx_data::ott::favorites::Locator::Plex{server_id:"synthetic-server".into(),rating_key:"123".into()},
        };
        let identity=plx_data::ott::favorites_wire::hash(favorite.identity().as_bytes());
        r.saved.favorites.toggle(favorite).unwrap();
        r.saved.favorite_items=Some(serde_json::from_value(json!({"version":1,"rows":[{"identity":identity,"id":"11111111-1111-4111-8111-111111111111"}]})).unwrap());
        assert_eq!(favorite_item_ids::ordered(&r.saved).unwrap().len(),1);
        let saved=serde_json::to_value(&r.saved).unwrap();let checkpoints=r.checkpoints.clone();
        let provider=r.provider;let receiver=r.remote.runtime().to_owned();
        let source=r.usb_remote_handle(handle).unwrap();let original=source.clone();
        let inv=invocation(&mut r,4,"local_media",json!({"runtime":"browser-fixture","op":operation,"source_handle":handle}));
        r.usb_handle_prepare_for_test(&inv,source).unwrap();
        let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
        let result=disk.execute(task,&cancel);assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::Verified(_))));
        // Own the synthetic runner slot before result dispatch. Each following
        // real task is consumed below; no background process can steal it.
        r.catalog_done.store(false,Ordering::Release);
        r.usb_remote_result(&mut app,serial,result);
        assert!(r.transient.candidate.is_some());assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);
        assert_eq!(plx_media::route::playback_identity(&app.player.session),old_identity);
        let ott_catalog::Task::LocalBindings(request)=transient::take_binding_task(&mut r).unwrap() else{panic!()};
        assert!(!request.allow_new && !request.allow_reconnect && !request.restore_remembered);
        let off=plx_base::task::OffFrame::for_test();let mut observations=0;
        let binding=ott_catalog::validate_local_bindings_observed_for_test(&off,request,&r.cancel,&mut|_,_|{observations+=1;Ok(vec![disk.mount()])}).unwrap();
        assert_eq!(observations,1);assert!(binding.operation_error.is_none());
        assert!(binding.sources.iter().any(|root|root.same_grant(&original)));
        let generation=r.generation;transient::binding_result(&mut r,&mut app,generation,Ok(binding));
        let profile=r.catalog_next.take().unwrap();
        let ott_catalog::Task::LocalFolder{profile,source}=r.local_catalog_task(profile).unwrap() else{panic!()};
        assert!(source.same_grant(&original));let mut rows=Vec::new();
        let report=ott_catalog::visit_local_folder_for_test(&off,&profile,&source,&r.cancel,&mut|batch|{rows.extend(batch);Ok(())}).unwrap();
        assert_eq!(report,(3,false));assert_eq!(rows.len(),3);
        r.resolver_busy=true;let mut incoming=Catalog{hierarchy:None,entries:rows};let mut finished=Some(Ok(()));
        assert!(transient::scan_result(&mut r,&mut app,&mut incoming,&mut finished));
        let request=r.play_target.as_ref().unwrap().clone();
        assert_eq!(request.position_ms,0);assert!(request.kiosk_selection.is_none());assert!(request.transient_owner.is_some());
        let (profile,entry)=transient::staged_native_binding(&r,&request).unwrap().unwrap();
        let MediaLocator::Local{relative_path,..}=&entry.media else{panic!("held Local file expected")};
        let external=plx_media::ff::ExternalSource::Local(disk.0.join(relative_path));
        assert!(transient::before_native(&mut r,&app).is_ok());
        let mut info=unsupported_info();info.video_codec="h264".into();info.audio_codec="aac".into();info.audio_channels=2;info.audio_sample_rate=48000;info.audio_bit_depth=16;
        assert_eq!(plx_media::route::playback_identity(&app.player.session),old_identity);
        assert!(plx_media::route::has_url(&app.player.session));
        let before=plx_media::route::play_request_ticket();
        // Match start_resolved_with: admission preserves the old playback until
        // this accepted successor retires it through the production seam.
        playback::replace_player_in_place(
            &mut app.player,
            &mut app.adapters.player,
            &mut app.repause_at,
            &mut app.pages,
        );
        assert!(!plx_media::route::has_url(&app.player.session));
        plx_media::route::request_external_source(&mut app.player.session,&info,external.clone(),"fixture","Local videos",plx_platform::devcaps::assumed_shared()).unwrap();
        let after=plx_media::route::play_request_ticket();assert_ne!(before,after);
        r.playing=Some(Playing{channel_capture:None,archive:None,kiosk_selection:None,transient_owner:request.transient_owner,local_selection:request.local_selection,identity:plx_media::route::playback_identity(&app.player.session),profile,entry,selection_id:request.id.clone(),source:Some(external),advance_on_failure:false,requires_server_lease:false});
        let native=NativeStartReceipt{generation:r.play_generation,before,after};
        let response=transient::finish_native(&mut r,&mut app,Ok(()),Some(&native)).unwrap();
        assert_eq!(response["status"],"ok");assert_eq!(response["data"]["state"],"ready");assert_eq!(response["data"]["source_handle"],handle);
        assert_eq!(response["data"]["source"]["id"],selected["data"]["source"]["id"]);
        assert!(r.transient.candidate.is_none());assert!(r.transient.accepted.is_some());
        let owner=r.playing.as_ref().unwrap().transient_owner.unwrap();
        r.play_target=None;r.resolver_next=None;r.resolver_busy=false;
        plx_media::player::SHARED.playpos_ns.store(31_000_000_000,Ordering::Relaxed);
        r.checkpoint(&app,true);
        assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);assert_eq!(serde_json::to_value(&r.checkpoints).unwrap(),serde_json::to_value(&checkpoints).unwrap());
        assert_eq!(r.provider,provider);assert_eq!(r.remote.runtime(),receiver);assert!(!r.saving);
        assert!(r.usb_remote_handle(handle).unwrap().same_grant(&original));
        let Some(transient::TransientMedia::Local(session))=r.transient.accepted.as_ref() else{panic!()};
        assert!(session.context.owner()==owner);assert_eq!(session.context.position_ms(),31_000);
        assert!(session.kiosk_resume.is_none());
    }
}


fn select_root_again(app:&mut App,disk:&Disk,base:u64)->Value {
    let roots=browse(app,disk,base,json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    let list=browse(app,disk,base+1,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}));
    browse(app,disk,base+2,json!({"version":1,"runtime":"browser-fixture","op":"select","generation":list["data"]["generation"],"select_id":list["data"]["select_id"]}))
}

fn connect_completion(app:&mut App,handle:&str,id:u64,saved:bool)->Value {
    let mut r=app.ott.take().unwrap();let source=r.usb_remote_handle(handle).unwrap();
    let candidate=r.usb_remote_connect_candidate(handle,&source).unwrap();
    let inv=invocation(&mut r,id,"local_media",json!({"runtime":"browser-fixture","op":"connect","source_handle":handle}));
    let mut request=local::LocalRequest::parse(&inv.params).unwrap();
    let Some(local::Selector::Handle{source_id,..})=&mut request.selector else{panic!()};*source_id=Some(source.descriptor().id.clone());
    // Storage's protected write/ACK is tested separately. Deliver only its
    // terminal result at the actual Local connection owner boundary here.
    r.transient.candidate=Some(transient::Candidate::Local(transient::LocalCandidate{
        inventory:r.local_inventory.sources.clone(),admission_ticket:plx_media::route::play_request_ticket(),owner:local::OwnerToken(r.generation),
        pending:local::PendingLocal::new(request.clone(),local::OwnerToken(r.generation),inv.deadline_ms),purpose:transient::LocalPurpose::Remote(inv.clone()),request,
        interrupted:transient::InterruptedScan{owner:None,profile:r.effective_profile().cloned(),restart:false},phase:transient::CandidatePhase::SavingConnection(source.descriptor().clone())}));
    if saved {candidate.put_into(&mut r.saved.settings).unwrap();}
    let result=transient::connection_saved(&mut r,app,&inv,saved).unwrap();app.ott=Some(r);result
}

#[test]
fn local_handle_reselection_and_connect_keep_one_identity_and_do_not_activate() {
    let _serial=plx_base::testlock::serial();let mut disk=Disk::new();disk.1=true;
    let mut app=ready();let initial=select_root_again(&mut app,&disk,1);assert_eq!(initial["status"],"ok");
    let handle=initial["data"]["source"]["handle"].as_str().unwrap().to_owned();
    let source=app.ott.as_ref().unwrap().usb_remote_handle(&handle).unwrap();
    let profiles=serde_json::to_value(&app.ott.as_ref().unwrap().saved.profiles).unwrap();
    let state_before = state(app.ott.as_ref().unwrap());
    let completion_generation = app.ott.as_ref().unwrap().generation;
    let completion_cancel = app.ott.as_ref().unwrap().cancel.clone();
    assert!(!completion_cancel.load(Ordering::Acquire));
    let failed = connect_completion(&mut app, &handle, 4, false);
    assert_eq!(failed["status"], "rejected");
    // This helper enters at Saved completion, after Connect owns a candidate.
    // Rejection retires that worker epoch without changing the active source.
    let mut state_after_rejection = state_before.clone();
    state_after_rejection["generation"] = json!(completion_generation.wrapping_add(1).max(1));
    let rejected = app.ott.as_ref().unwrap();
    assert_eq!(state(rejected), state_after_rejection);
    assert!(completion_cancel.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&completion_cancel, &rejected.cancel));
    assert!(!rejected.cancel.load(Ordering::Acquire));
    assert!(rejected.transient.candidate.is_none());
    assert!(rejected.catalog_next.is_none());
    let accepted=connect_completion(&mut app,&handle,5,true);assert_eq!(accepted["data"]["state"],"ready");
    let saved=serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let reselected=select_root_again(&mut app,&disk,10);assert_eq!(reselected["status"],"ok");
    assert_eq!(reselected["data"]["source"]["id"],initial["data"]["source"]["id"]);
    assert_eq!(reselected["data"]["source"]["handle"],handle);
    assert!(app.ott.as_ref().unwrap().usb_remote_handle(&handle).unwrap().same_grant(&source));
    let accepted=connect_completion(&mut app,&handle,13,true);assert_eq!(accepted["data"]["state"],"ready");
    let r=app.ott.as_mut().unwrap();assert_eq!(r.local_inventory.sources.len(),1);
    let remembered=crate::app::ott_local_sources::RememberedFolders::read(&r.saved.settings).unwrap();assert_eq!(remembered.folders().len(),1);assert!(remembered.folders()[0].usb.is_some());
    assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);assert_eq!(serde_json::to_value(&r.saved.profiles).unwrap(),profiles);
    assert_eq!(r.provider,ProviderKind::M3u);assert!(r.play_target.is_none()&&r.pending_play.is_none()&&r.transient.accepted.is_none());
    // Another remembered image is not the receipt's own durable ACK.
    let mut changed=remembered.folders()[0].clone();changed.name="different".into();
    crate::app::ott_local_sources::RememberedFolders::default().with_connected(changed).unwrap().put_into(&mut r.saved.settings).unwrap();
    assert!(r.usb_remote_connect_candidate(&handle,&source).is_err());
}

#[test]
fn local_select_expiry_keeps_observed_old_grant_loss_without_installing_a_replacement() {
    let _serial=plx_base::testlock::serial();let mut disk=Disk::new();disk.1=true;
    std::fs::create_dir(disk.0.join("Movies")).unwrap();let mut app=ready();let initial=selected_movies(&mut app,&disk);
    let handle=initial["data"]["source"]["handle"].as_str().unwrap().to_owned();assert_eq!(connect_completion(&mut app,&handle,5,true)["status"],"ok");
    std::fs::rename(disk.0.join("Movies"),disk.0.join("Away")).unwrap();std::fs::create_dir(disk.0.join("Movies")).unwrap();
    let roots=browse(&mut app,&disk,10,json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    let dirs=browse(&mut app,&disk,11,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}));
    let movies=dirs["data"]["entries"].as_array().unwrap().iter().find(|entry|entry["name"]=="Movies").unwrap()["id"].clone();
    let page=browse(&mut app,&disk,12,json!({"version":1,"runtime":"browser-fixture","op":"list","generation":dirs["data"]["generation"],"entry_id":movies}));
    let mut r=app.ott.take().unwrap();let before=state(&r);
    let inv=invocation(&mut r,13,"local_browser",json!({"version":1,"runtime":"browser-fixture","op":"select","generation":page["data"]["generation"],"select_id":page["data"]["select_id"]}));
    r.usb_browser_prepare_for_test(&inv).unwrap();let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};
    let result=disk.execute(task,&cancel);assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::SourceLost(_))));
    r.expire_usb_select_for_test();r.usb_remote_poll();let response=r.remote.response_for_id_for_test(&inv.id).unwrap();assert_eq!(response["data"]["error"],"expired");assert!(!cancel.load(Ordering::Acquire));
    r.usb_remote_result(&mut app,serial,result);
    std::fs::remove_dir(disk.0.join("Movies")).unwrap();std::fs::rename(disk.0.join("Away"),disk.0.join("Movies")).unwrap();
    assert!(r.usb_remote_handle(&handle).is_err());assert_eq!(r.local_inventory.sources.len(),1);assert!(!r.local_inventory.sources[0].is_live());
    assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap(),response);assert_eq!(state(&r),before);assert!(r.transient.candidate.is_none());
}


#[test]
fn local_handle_moved_mount_uses_receipt_scoped_binding_input_for_every_mutation() {
    let _serial=plx_base::testlock::serial();let _reset=NativeReset;
    for operation in ["connect","play","shuffle"] {
        for stale_saved in [false,true] {
            let mut old_disk=Disk::new();old_disk.1=true;let mut app=ready();
            let selected=select_root_again(&mut app,&old_disk,1);
            let old_handle=selected["data"]["source"]["handle"].as_str().unwrap();
            assert_eq!(connect_completion(&mut app,old_handle,4,true)["status"],"ok");
            app.ott.as_ref().unwrap().usb_remote_handle(old_handle).unwrap().withdraw();
            let mut moved_disk=Disk::new();moved_disk.1=true;
            let selected=select_root_again(&mut app,&moved_disk,10);assert_eq!(selected["status"],"ok");
            let handle=selected["data"]["source"]["handle"].as_str().unwrap();
            if stale_saved {
                let r=app.ott.as_mut().unwrap();let prior=crate::app::ott_local_sources::RememberedFolders::read(&r.saved.settings).unwrap();let mut row=prior.folders()[0].clone();row.name="other saved intent".into();
                crate::app::ott_local_sources::RememberedFolders::default().with_connected(row).unwrap().put_into(&mut r.saved.settings).unwrap();
            }
            super::local_integration::install(&mut app,None,"previous-live");
            let identity=plx_media::route::playback_identity(&app.player.session);let ticket=plx_media::route::play_request_ticket();
            let mut r=app.ott.take().unwrap();let before=state(&r);let saved=serde_json::to_value(&r.saved).unwrap();
            let inv=invocation(&mut r,13,"local_media",json!({"runtime":"browser-fixture","op":operation,"source_handle":handle}));
            let source=r.usb_remote_handle(handle).unwrap();r.usb_handle_prepare_for_test(&inv,source.clone()).unwrap();
            let ott_catalog::Task::UsbRemote{serial,task,cancel}=r.take_usb_remote_task().unwrap() else{panic!()};let result=moved_disk.execute(task,&cancel);
            assert!(matches!(&result,Ok(crate::app::ott_usb::Answer::Verified(_))));
            r.catalog_done.store(false,Ordering::Release);r.usb_remote_result(&mut app,serial,result);
            if stale_saved {
                assert_eq!(r.remote.response_for_id_for_test(&inv.id).unwrap()["status"],"rejected");assert_eq!(state(&r),before);assert!(r.transient.candidate.is_none());
            } else {
                let ott_catalog::Task::LocalBindings(request)=transient::take_binding_task(&mut r).unwrap() else{panic!()};
                assert_eq!(request.remembered.len(),1);assert_eq!(request.remembered[0].path,moved_disk.0.to_str().unwrap());
                let remembered=crate::app::ott_local_sources::RememberedFolders::read(&r.saved.settings).unwrap();assert_eq!(remembered.folders()[0].path,old_disk.0.to_str().unwrap());
                let validated=ott_catalog::validate_local_bindings_observed_for_test(&plx_base::task::OffFrame::for_test(),request,&r.cancel,&mut|_,_|Ok(vec![moved_disk.mount()])).unwrap();
                assert!(validated.operation_error.is_none());assert_eq!(validated.sources.len(),1);assert!(validated.sources[0].same_grant(&source));
            }
            assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);assert_eq!(plx_media::route::playback_identity(&app.player.session),identity);assert!(plx_media::route::play_request_is_current(ticket));
        }
    }
}

#[path = "local_browser_controller252.rs"]
mod controller252;


#[test]
fn local_browser_pump_ignores_stale_serial_before_current_completion() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let mut runtime = app.ott.take().unwrap();
    let inv = invocation(&mut runtime, 901, "local_browser",
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}));
    runtime.usb_browser_prepare_for_test(&inv).unwrap();
    let ott_catalog::Task::UsbRemote { serial, task, cancel } = runtime.take_usb_remote_task().unwrap()
        else { panic!("the admitted request must own a USB task"); };
    // The worker result is the only injected boundary; real pending identity,
    // dispatch, policy and response delivery remain in the runtime.
    drop(task);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.usb_remote_waiting_result());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.catalog_tx.try_send(CatalogEvent::UsbRemote(serial + 1,
        Err(crate::app::ott_usb::Error::Changed))).is_ok());
    assert!(runtime.catalog_tx.try_send(CatalogEvent::UsbRemote(serial,
        Err(crate::app::ott_usb::Error::Unavailable))).is_ok());
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let response = runtime.remote.response_for_id_for_test(&inv.id).expect("current completion was dispatched");
    assert_eq!(response["id"], inv.id);
    assert_eq!(response["data"]["runtime"], inv.params["runtime"]);
    assert_eq!(response["data"]["error"], "unavailable");
    assert!(!runtime.usb_remote_waiting_result());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.local_inventory.sources.is_empty());
    assert!(runtime.play_target.is_none() && runtime.pending_play.is_none());
}

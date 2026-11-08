//! Real action/task/result boundaries with temporary grants. The actual shared
//! worker slot is held; execute_with_identity runs its IO body synchronously.
//! No catalogue worker, protected write, decoder, mount or network is started.
//! One control uses the existing writer with a revoked permit and drains it.
use super::*;
use plx_base::task::OffFrame;
use plx_platform::tv::removable::{identity, MountCandidate};
use std::fs::{self, File};
use std::path::Path;

fn fixture() -> (
    crate::app::ott_runtime::tests::local_integration::TempDir,
    Box<App>,
    MountCandidate,
) {
    let (directory, mut app, root) = crate::app::ott_runtime::tests::local_integration::setup();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = false;
    runtime.resume_source_work();
    runtime.provider = ProviderKind::Local;
    runtime.page = "profiles";
    let path = root.canonical_path.as_deref().unwrap();
    fs::create_dir_all(Path::new(path).join("Movies/Season")).unwrap();
    let file = File::open(path).unwrap();
    let device = identity::directory_device(&OffFrame::for_test(), &file).unwrap();
    let mount = MountCandidate::directory_device_for_test(path, 8, device).unwrap();
    (directory, app, mount)
}
fn act(app: &mut App, action: OttAction) {
    let mut runtime = app.ott.take().unwrap();
    crate::app::ott_runtime::handle_action(&mut runtime, app, action).unwrap();
    app.ott = Some(runtime);
}
fn key(app: &App, id: &str) -> String {
    let view = app.ott.as_ref().unwrap().usb_browser_view().unwrap();
    let row = view.rows.iter().find(|row| row.id == id).unwrap();
    assert!(row.enabled, "requested control must be available");
    let OttAction::EditSetting(key) = &row.action else {
        panic!("USB action")
    };
    key.clone()
}
fn press(app: &mut App, id: &str) {
    act(app, OttAction::EditSetting(key(app, id)));
}
fn task(app: &mut App) -> (u64, usb::Task) {
    let runtime = app.ott.as_mut().unwrap();
    assert!(
        !runtime.catalog_done.load(Ordering::Acquire),
        "held slot prevents thread creation"
    );
    let ott_catalog::Task::Usb(task) = runtime.take_usb_task().unwrap() else {
        panic!("USB worker task")
    };
    (runtime.generation, task)
}
fn run(task: usb::Task, mount: &MountCandidate) -> Result<usb::Answer, usb::Error> {
    usb::execute_with_identity(
        &OffFrame::for_test(),
        task,
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, _, _| Err(identity::IdentityError::Unsupported),
    )
}
fn deliver(app: &mut App, generation: u64, result: Result<usb::Answer, usb::Error>) {
    let mut runtime = app.ott.take().unwrap();
    runtime.usb_result(app, generation, result);
    app.ott = Some(runtime);
}
fn settle(app: &mut App, mount: &MountCandidate) {
    let (generation, task) = task(app);
    deliver(app, generation, run(task, mount));
}
fn open(app: &mut App, mount: &MountCandidate) {
    act(app, OttAction::EditSetting("local.usb".into()));
    settle(app, mount);
    let generation = app.ott.as_ref().unwrap().generation;
    act(
        app,
        OttAction::EditSetting(format!("local.usb.select:{generation}:0")),
    );
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .usb_browser_view()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.id == "usb.cancel" && row.enabled));
    settle(app, mount);
}

#[test]
fn browsing_and_cancel_preserve_saved_accepted_source_and_interrupted_scan() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let source = app.ott.as_ref().unwrap().local_inventory.sources[0].clone();
    open(&mut app, &mount);
    let browse_root = app
        .ott
        .as_ref()
        .unwrap()
        .local_inventory
        .usb
        .browser
        .clone()
        .unwrap();
    assert!(browse_root.is_live());
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
    let child = key(&app, "usb.folder:/Movies");
    act(&mut app, OttAction::EditSetting(child));
    settle(&mut app, &mount);
    act(&mut app, OttAction::Back);
    settle(&mut app, &mount);
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .local_inventory
        .usb
        .browser
        .as_ref()
        .unwrap()
        .is_root());
    press(&mut app, "usb.cancel");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "profiles");
    assert!(!runtime.usb_browsing() && !browse_root.is_live());
    assert!(source.is_live() && runtime.local_inventory.sources[0].same_grant(&source));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(
        runtime.catalog_next.is_some(),
        "original interrupted catalogue is requeued"
    );
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
}

#[test]
fn pending_back_keeps_prior_page_and_discards_late_result_without_releasing_slot() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    press(&mut app, "usb.folder:/Movies");
    let (old_generation, work) = task(&mut app);
    let answer = run(work, &mount);
    let cancelled = app.ott.as_ref().unwrap().cancel.clone();
    act(&mut app, OttAction::Back);
    assert!(cancelled.load(Ordering::Acquire));
    deliver(&mut app, old_generation, answer);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime
        .local_inventory
        .usb
        .browser
        .as_ref()
        .unwrap()
        .is_root());
    assert!(!runtime.local_inventory.usb.waiting && !runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime
        .usb_browser_view()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.id == "usb.use" && row.enabled));
}

#[test]
fn paging_and_refresh_keep_folder_identity_and_reject_old_action_epoch() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    for i in 0..70 {
        fs::create_dir(Path::new(mount.mount_point()).join(format!("Folder-{i:03}"))).unwrap();
    }
    open(&mut app, &mount);
    let initial = app.ott.as_ref().unwrap().usb_browser_view().unwrap();
    let row = initial
        .rows
        .iter()
        .find(|row| row.id == "usb.folder:/Folder-005")
        .unwrap();
    let old_action = row.action.clone();
    press(&mut app, "usb.next");
    assert_eq!(
        app.ott.as_ref().unwrap().local_inventory.usb.browser_page,
        1
    );
    press(&mut app, "usb.previous");
    press(&mut app, "usb.refresh");
    let waiting = app.ott.as_ref().unwrap().usb_browser_view().unwrap();
    assert!(waiting
        .rows
        .iter()
        .any(|row| row.id == "usb.folder:/Folder-005" && !row.enabled));
    assert!(waiting
        .rows
        .iter()
        .any(|row| row.id == "usb.cancel" && row.enabled));
    fs::create_dir(Path::new(mount.mount_point()).join("000-new")).unwrap();
    settle(&mut app, &mount);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.usb_browser_view().unwrap().page_id, initial.page_id);
    assert!(runtime
        .usb_browser_view()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.id == "usb.folder:/Folder-005"));
    let mut runtime = app.ott.take().unwrap();
    assert!(crate::app::ott_runtime::handle_action(&mut runtime, &mut app, old_action).is_err());
    app.ott = Some(runtime);
}

#[test]
fn use_queues_real_revalidation_before_staging_any_saved_change() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    press(&mut app, "usb.folder:/Movies");
    settle(&mut app, &mount);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    press(&mut app, "usb.use");
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
    let (_, work) = task(&mut app);
    let usb::Answer::Selected(selected) = run(work, &mount).unwrap() else {
        panic!("confirmed folder")
    };
    let runtime = app.ott.as_mut().unwrap();
    let target = runtime.local_inventory.usb.target.clone().unwrap();
    let staged = runtime.stage_usb_install(target, selected).unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(staged
        .profiles
        .profiles
        .iter()
        .any(|profile| profile.source == format!("{}/Movies", mount.mount_point())));
    assert!(
        !runtime.saving,
        "the test stages only; no protected writer is started"
    );
}

#[test]
fn suspend_retires_private_browser_without_revoking_accepted_root_or_releasing_worker() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    press(&mut app, "usb.refresh");
    let old = app
        .ott
        .as_ref()
        .unwrap()
        .local_inventory
        .usb
        .browser
        .clone()
        .unwrap();
    let source = app.ott.as_ref().unwrap().local_inventory.sources[0].clone();
    let mut runtime = app.ott.take().unwrap();
    runtime.suspend_source_work(&mut app);
    assert!(!runtime.usb_browsing() && !old.is_live());
    assert!(source.is_live());
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    runtime.resume_source_work();
    runtime.rearm_source_work(&mut app);
    assert!(!runtime.usb_browsing() && !runtime.catalog_done.load(Ordering::Acquire));
    app.ott = Some(runtime);
}

#[test]
fn browser_only_navigation_restores_queued_scan_without_waiting_for_a_reply() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    let expected = app.ott.as_ref().unwrap().effective_profile().cloned();
    press(&mut app, "usb.refresh");
    let old_cancel = app.ott.as_ref().unwrap().cancel.clone();
    // Leave the task queued: there will be no completion callback to repair it.
    act(&mut app, OttAction::ShowCategories);
    let runtime = app.ott.as_mut().unwrap();
    runtime.accepted_navigation(false);
    assert_eq!(runtime.page, "categories");
    assert!(!runtime.usb_browsing() && runtime.local_inventory.usb.pending.is_none());
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(runtime.catalog_next == expected && runtime.catalog_loading);
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
}

#[test]
fn late_browser_result_and_back_cannot_cancel_a_new_local_command() {
    let _serial = plx_base::testlock::serial();
    for back in [false, true] {
        let (_directory, mut app, mount) = fixture();
        open(&mut app, &mount);
        press(&mut app, "usb.refresh");
        let (generation, work) = task(&mut app);
        let old_answer = run(work, &mount);
        let mut runtime = app.ott.take().unwrap();
        let source = runtime.local_inventory.sources[0].descriptor().id.clone();
        let now = runtime.now_ms();
        let request = json!({"commands":[],"server_time":1000,"requests":[{
            "id":"00000000000000000000000000000001","action":"local_media",
            "params":{"op":"play","runtime":runtime.remote.runtime(),"source":source,"session":true,"progressive":true},
            "expires_at":1020
        }]}).to_string();
        let mut admitted = runtime.remote.raw_poll_for_test(&request, now);
        assert_eq!(admitted.len(), 1);
        let invocation = admitted.pop().unwrap();
        assert!(runtime.remote.invocation_is_current(&invocation));
        assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
        assert!(runtime.transient.candidate.is_some());
        assert_ne!(runtime.generation, generation);
        let current_cancel = runtime.cancel.clone();
        let current_generation = runtime.generation;
        if back {
            runtime.usb_browser_back(&app).unwrap();
        }
        runtime.usb_result(&mut app, generation, old_answer);
        assert!(!current_cancel.load(Ordering::Acquire));
        assert_eq!(runtime.generation, current_generation);
        assert!(runtime.transient.candidate.is_some());
        assert!(!runtime.catalog_done.load(Ordering::Acquire));
        app.ott = Some(runtime);
    }
}

#[test]
fn favorites_navigation_keeps_preserve_writer_and_restores_interrupted_scan() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    press(&mut app, "usb.refresh");
    let runtime = app.ott.as_mut().unwrap();
    let expected = runtime.effective_profile().cloned();
    let playback_epoch = runtime.playback_epoch;
    runtime.write_permit.revoke();
    act(&mut app, OttAction::ShowFavorites);
    let runtime = app.ott.as_mut().unwrap();
    // Drain the real pre-IO refused writer before assertions can unwind. Its
    // result is deliberately not applied yet, modelling the pending ACK.
    let completion = runtime
        .rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(matches!(completion, Job::Saved(Err(_), None, false, _)));
    assert!(runtime.saving && runtime.pending_category_choice.is_some());
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    runtime.accepted_navigation(true);
    assert!(!runtime.usb_browsing());
    assert!(runtime.catalog_next == expected && runtime.catalog_loading);
    assert!(runtime.saving && runtime.pending_category_choice.is_some());
    assert_eq!(runtime.save_playback_epoch, playback_epoch);
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
}

#[test]
fn new_local_command_supersedes_the_scan_restored_by_picker_navigation() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, mount) = fixture();
    open(&mut app, &mount);
    press(&mut app, "usb.refresh");
    act(&mut app, OttAction::ShowCategories);
    let mut runtime = app.ott.take().unwrap();
    runtime.accepted_navigation(false);
    assert!(runtime.catalog_next.is_some() && runtime.catalog_loading);
    let restored_cancel = runtime.cancel.clone();
    let restored_generation = runtime.generation;
    let source = runtime.local_inventory.sources[0].descriptor().id.clone();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"00000000000000000000000000000001","action":"local_media",
        "params":{"op":"play","runtime":runtime.remote.runtime(),"source":source,"session":true,"progressive":true},
        "expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let invocation = requests.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    assert!(restored_cancel.load(Ordering::Acquire));
    assert_ne!(runtime.generation, restored_generation);
    assert!(runtime.transient.candidate.is_some());
    assert!(runtime.catalog_next.is_none() && !runtime.catalog_loading);
    assert!(!runtime.cancel.load(Ordering::Acquire));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    app.ott = Some(runtime);
}

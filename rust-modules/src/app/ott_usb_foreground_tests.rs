//! Child of the real USB install fixture. No mount or filesystem probes run:
//! a real queued task receives the same launch-failure event as the runtime.
use super::*;

fn failed_initial_restore(runtime: &mut Runtime) {
    assert!(!runtime.usb_catalog_allowed());
    assert!(matches!(runtime.take_usb_task(), Some(ott_catalog::Task::Usb(usb::Task::Restore { .. }))));
    runtime.usb_launch_failed();
    assert!(runtime.catalog_next.is_none());
    assert!(!runtime.catalog_loading);
}
fn suspend_and_resume(runtime: &mut Runtime, app: &mut App) {
    runtime.suspend_source_work(app);
    runtime.resume_source_work();
}

#[test]
fn foreground_retries_failed_initial_usb_only_after_old_catalogue_finishes() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut runtime = runtime();
    let expected = runtime.active().unwrap().clone();
    failed_initial_restore(&mut runtime);
    suspend_and_resume(&mut runtime, &mut app);
    runtime.rearm_source_work(&mut app);
    assert!(runtime.catalog_next.is_none(), "old catalogue slot is still held");
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert_eq!(runtime.catalog_next.as_ref(), Some(&expected));
    assert!(runtime.catalog_loading);
    assert!(!runtime.catalog_complete);

    // Fail the new queued attempt too. A normal frame is not another foreground
    // edge and must not immediately retry a terminally failed source again.
    failed_initial_restore(&mut runtime);
    for _ in 0..3 { runtime.rearm_source_work(&mut app); }
    assert!(runtime.catalog_next.is_none());
    assert!(!runtime.catalog_loading);
    assert!(runtime.take_usb_task().is_none());
}

#[test]
fn foreground_does_not_rearm_usb_after_new_native_ticket() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut runtime = runtime();
    failed_initial_restore(&mut runtime);
    plx_media::route::cancel_play(&mut app.player.session);
    suspend_and_resume(&mut runtime, &mut app);
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert!(runtime.catalog_next.is_none());
    assert!(runtime.take_usb_task().is_none());
}

#[test]
fn foreground_does_not_rearm_usb_after_new_navigation_playback_or_profile() {
    let _lock = plx_base::testlock::serial();
    for changed in 0..3 {
        let mut app = app();
        let mut runtime = runtime();
        failed_initial_restore(&mut runtime);
        match changed {
            0 => runtime.navigation_epoch = runtime.navigation_epoch.wrapping_add(1),
            1 => runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1),
            _ => runtime.saved.profiles.profiles[0].source = "/tmp/usb/different/part/Movies".into(),
        }
        suspend_and_resume(&mut runtime, &mut app);
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        assert!(runtime.catalog_next.is_none());
        assert!(runtime.take_usb_task().is_none());
    }
}

#[test]
fn foreground_rearms_cancelled_inflight_usb_once_and_ignores_late_old_result() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut runtime = runtime();
    let expected = runtime.active().unwrap().clone();
    assert!(!runtime.usb_catalog_allowed());
    assert!(matches!(runtime.take_usb_task(), Some(ott_catalog::Task::Usb(usb::Task::Restore { .. }))));
    let old_generation = runtime.generation;
    suspend_and_resume(&mut runtime, &mut app);
    runtime.usb_result(&mut app, old_generation, Err(usb::Error::Cancelled));
    runtime.rearm_source_work(&mut app);
    assert!(runtime.catalog_next.is_none(), "cancellation is not worker-slot completion");
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert_eq!(runtime.catalog_next.as_ref(), Some(&expected));
    runtime.usb_result(&mut app, old_generation, Err(usb::Error::Cancelled));
    assert_eq!(runtime.catalog_next.as_ref(), Some(&expected), "late old callback cannot clear new intent");
    assert!(runtime.catalog_loading);
}

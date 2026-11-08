//! Real M3U catalogue and Job::Saved regressions for the two source identities.
use super::*;

fn catalogue_app() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([3; 16]);
    runtime.write_permit.revoke();
    runtime.saved.profiles.profiles = vec![source_profile(ProviderKind::M3u, "a"), source_profile(ProviderKind::M3u, "b")];
    runtime.saved.profiles.active_profile = Some("a".into());
    runtime.provider = ProviderKind::M3u;
    runtime.page = "profiles";
    runtime.profile_focus = Some("a".into());
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"first\",First\nhttps://old.invalid/first.m3u8\n#EXTINF:-1 tvg-id=\"second\",Second\nhttps://old.invalid/second.m3u8\n",
        runtime.active().unwrap()).unwrap();
    runtime.catalog_complete = true;
    runtime.catalog_dirty = false;
    runtime.selected = Some(runtime.catalog.entries[0].id.clone());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let first = runtime.catalog.entries[0].id.clone();
    runtime.continuity = continuity::Continuity::new(runtime.generation,
        Some(continuity::ResumePoint { entry_id: first, position_ms: 3210 }));
    app
}

fn submit_value(app: &mut App, target: &str, value: Option<&str>) -> (Saved, EditorSession) {
    let offer = edit(app, target);
    action(app, offer);
    let runtime = app.ott.as_mut().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    let original = runtime.edit_profile_original.clone().unwrap();
    let mut next = original.clone();
    next.source = value.map(str::to_owned).unwrap_or_else(|| format!("https://new.invalid/{target}.m3u"));
    let (candidate, reload) = runtime.profile_editor_candidate(&editor, next.clone(), Some(&original)).unwrap();
    assert!(!reload);
    assert!(checked(app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key.clone(), value: next.source,
    }).is_ok());
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Job::Saved(Err(_), None, false, _)));
    (candidate, editor)
}

fn deliver(app: &mut App, job: Job) {
    app.ott.as_ref().unwrap().tx.send(job).ok().unwrap();
    runtime_tick(app);
    settle_pages(app);
}

fn submit_source(app: &mut App, target: &str) -> (Saved, EditorSession) {
    submit_value(app, target, None)
}

#[test]
fn save_only_ack_retires_active_catalogue_but_preserves_inactive_catalogue_without_fetch() {
    let _serial = plx_base::testlock::serial();
    for target in ["a", "b"] {
        let mut app = catalogue_app();
        let (candidate, _) = submit_source(&mut app, target);
        let runtime = app.ott.as_ref().unwrap();
        let generation = runtime.generation;
        let revision = runtime.catalog_revision;
        let entries = runtime.catalog.entries.clone();
        let restore = runtime.continuity.pending_restore().cloned();
        assert!(restore.is_some());
        deliver(&mut app, Job::Saved(Ok(candidate), None, false, json!({})));
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.catalog_complete);
        assert!(!runtime.catalog_loading);
        assert!(runtime.catalog_next.is_none());
        if target == "a" {
            assert_ne!(runtime.generation, generation);
            assert_ne!(runtime.catalog_revision, revision);
            assert!(runtime.catalog.entries.is_empty());
            assert!(runtime.continuity.pending_restore().is_none());
            assert!(runtime.prepare_play_request(PlaybackRequest::ordinary(&entries[0].id, 0)).is_err());
        } else {
            assert_eq!(runtime.generation, generation);
            assert_eq!(runtime.catalog_revision, revision);
            assert_eq!(runtime.catalog.entries, entries);
            assert_eq!(runtime.continuity.pending_restore(), restore.as_ref());
            let (_, profile, entry, _) = runtime.prepare_play_request(PlaybackRequest::ordinary(&entries[0].id, 0)).unwrap();
            assert_eq!(profile, source_profile(ProviderKind::M3u, "a"));
            assert_eq!(entry.as_ref(), Some(&entries[0]));
        }
        assert_eq!(runtime.saved.profiles.profiles.iter().find(|p| p.id == target).unwrap().source,
            format!("https://new.invalid/{target}.m3u"));
        runtime.catalog_done.store(true, Ordering::Release);
        assert!(runtime.take_catalog_task().is_none(), "field save never queues a catalogue worker");
        for _ in 0..2 { runtime_tick(&mut app); }
        assert!(app.ott.as_ref().unwrap().catalog_next.is_none());
        assert!(!app.ott.as_ref().unwrap().catalog_loading);
    }
}

#[test]
fn same_source_refresh_and_cancelled_editor_do_not_turn_save_ack_into_load() {
    let _serial = plx_base::testlock::serial();
    let mut app = catalogue_app();
    let (candidate, editor) = submit_source(&mut app, "a");
    action(&mut app, OttAction::CancelSetting { session: editor.id });
    let runtime = app.ott.as_mut().unwrap();
    let before = runtime.generation;
    runtime.refresh_catalog_preserving_playback();
    assert_ne!(runtime.generation, before);
    assert_eq!(runtime.catalog_next.as_ref(), runtime.active());
    deliver(&mut app, Job::Saved(Ok(candidate), None, false, json!({})));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.catalog_complete);
    assert!(runtime.catalog_next.is_none());
    assert!(!runtime.catalog_loading);
}

#[test]
fn explicit_same_id_load_after_field_save_queues_the_saved_new_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = catalogue_app();
    let (candidate, _) = submit_source(&mut app, "a");
    deliver(&mut app, Job::Saved(Ok(candidate), None, false, json!({})));
    assert!(app.ott.as_ref().unwrap().catalog_next.is_none());
    assert!(checked(&mut app, OttAction::SelectProfile("a".into())).is_ok());
    let runtime = app.ott.as_ref().unwrap();
    let _refused = runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let saved = runtime.saved.clone();
    deliver(&mut app, Job::Saved(Ok(saved), None, true, json!({})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog_next.as_ref(), runtime.active());
    assert_eq!(runtime.catalog_next.as_ref().unwrap().source, "https://new.invalid/a.m3u");
    assert!(runtime.catalog_loading);
}

#[test]
fn field_save_noop_or_failure_preserves_current_catalogue() {
    let _serial = plx_base::testlock::serial();
    for success in [true, false] {
        let mut app = catalogue_app();
        let old = source_profile(ProviderKind::M3u, "a");
        let (candidate, _) = if success { submit_value(&mut app, "a", Some(&old.source)) }
            else { submit_source(&mut app, "a") };
        let runtime = app.ott.as_ref().unwrap();
        let generation = runtime.generation;
        let entries = runtime.catalog.entries.clone();
        let job = if success { Job::Saved(Ok(candidate), None, false, json!({})) }
            else { Job::Saved(Err("Fixture refused"), None, false, json!({})) };
        deliver(&mut app, job);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.generation, generation);
        assert_eq!(runtime.catalog.entries, entries);
        assert_eq!(runtime.active(), Some(&old));
        assert!(runtime.catalog_next.is_none());
    }
}

#[test]
fn generic_source_editor_keeps_its_existing_save_and_load_contract() {
    let _serial = plx_base::testlock::serial();
    let mut app = catalogue_app();
    action(&mut app, OttAction::EditSetting("profile.source".into()));
    let runtime = app.ott.as_mut().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    let original = runtime.edit_profile_original.clone().unwrap();
    let mut changed = original.clone();
    changed.source = "https://generic.invalid/list.m3u".into();
    let (candidate, reload) = runtime.profile_editor_candidate(&editor, changed.clone(), Some(&original)).unwrap();
    assert!(reload);
    assert!(checked(&mut app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key, value: changed.source,
    }).is_ok());
    let _refused = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(5)).unwrap();
    deliver(&mut app, Job::Saved(Ok(candidate), None, true, json!({})));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.catalog_loading);
    assert_eq!(runtime.catalog_next.as_ref(), runtime.active());
}

#[test]
fn late_field_save_does_not_evict_a_new_accepted_temporary_catalogue() {
    let _serial = plx_base::testlock::serial();
    let mut app = catalogue_app();
    let (candidate, _) = submit_source(&mut app, "a");
    let (_directory, mut local_app, _) = local_integration::setup();
    let mut local = local_app.ott.take().unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.cancel_catalog();
    runtime.generation = local.generation;
    runtime.cancel = Arc::new(AtomicBool::new(false));
    std::mem::swap(&mut runtime.local_inventory, &mut local.local_inventory);
    runtime.transient = std::mem::take(&mut local.transient);
    runtime.catalog = std::mem::take(&mut local.catalog);
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.continuity = continuity::Continuity::new(runtime.generation, None);
    let owner = runtime.effective_profile().cloned();
    let entries = runtime.catalog.entries.clone();
    let generation = runtime.generation;
    deliver(&mut app, Job::Saved(Ok(candidate), None, false, json!({})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.effective_profile(), owner.as_ref());
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(runtime.generation, generation);
    assert!(runtime.transient.accepted.is_some());
    assert!(runtime.catalog_next.is_none());
}

#[test]
fn field_save_during_local_candidate_does_not_cancel_it_or_restart_old_scan_on_failure() {
    use crate::app::ott_local_session as local;
    let _serial = plx_base::testlock::serial();
    for (op, cancel) in [("sources", false), ("play", false), ("play", true)] {
        let mut app = catalogue_app();
        let (candidate, _) = submit_source(&mut app, "a");
        let mut runtime = app.ott.take().unwrap();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
        let params = if op == "sources" {
            json!({"op":op,"runtime":runtime.remote.runtime(),"path":"/media/fixture-next"})
        } else {
            json!({"op":op,"runtime":runtime.remote.runtime(),"source":"/media/fixture-next","progressive":true})
        };
        let body = json!({"commands":[],"server_time":1000,"requests":[{
            "id":"1234567890abcdef1234567890abcdef","action":"local_media",
            "params":params,"expires_at":1020
        }]}).to_string();
        let now = runtime.now_ms();
        let invocation = runtime.remote.raw_poll_for_test(&body, now).pop().unwrap();
        assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
        assert!(runtime.transient.candidate.is_some());
        let generation = runtime.generation;
        let token = runtime.cancel.clone();
        app.ott = Some(runtime);
        deliver(&mut app, Job::Saved(Ok(candidate), None, false, json!({})));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.generation, generation);
        assert!(Arc::ptr_eq(&runtime.cancel, &token));
        assert!(!token.load(Ordering::Acquire));
        assert!(runtime.transient.candidate.is_some());
        assert!(runtime.catalog.entries.is_empty());
        assert!(runtime.catalog_next.is_none());
        if cancel {
            let mut runtime = app.ott.take().unwrap();
            transient::cancel_play_candidate(&mut runtime, &mut app);
            app.ott = Some(runtime);
        } else {
            app.ott.as_ref().unwrap().catalog_tx.send(CatalogEvent::LocalBindings(
                generation, Err(local::LocalError::SourceUnavailable)))
                .unwrap_or_else(|_| panic!("catalogue receiver"));
        }
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.catalog.entries.is_empty());
        assert!(runtime.catalog_next.is_none());
        assert!(!runtime.catalog_loading);
    }
}

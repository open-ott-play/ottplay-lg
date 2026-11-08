//! Scalar Local profile edits through the real editor and existing binding validator.
//! Only temporary empty roots are inspected. Writer admission is modeled where
//! stated; actual Job completion runs, but no protected write/decoder starts.
use super::*;
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};

fn fixture() -> App {
    let mut app = app_fixture();
    let r = app.ott.as_mut().unwrap();
    r.loaded = true; r.entropy = Some([7;16]); r.provider = ProviderKind::Local;
    r.generation = 7; r.continuity.rebase(r.generation);
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true; r.plex_refresh_pending = true; r.write_permit.revoke();
    r.page = "profiles";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app); runtime_tick(&mut app); settle_pages(&mut app);
    app
}
fn open(app: &mut App, key: &str) -> EditorSession {
    let mut r = app.ott.take().unwrap(); r.reply = app.pages.top_page();
    handle_action(&mut r, app, OttAction::EditSetting(key.into())).unwrap();
    let editor = r.edit_session.clone().unwrap(); app.ott = Some(r);
    settle_pages(app); editor
}
fn submit(app: &mut App, editor: &EditorSession, value: &str) {
    let mut r = app.ott.take().unwrap();
    apply_ui_action(&mut r, app, OttAction::SubmitSetting {
        session:editor.id, key:editor.key.clone(), value:value.into(),
    }, true, false, false);
    app.ott = Some(r);
}
fn press_without_runtime(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
    settle_pages(app);
}
fn submit_screen(app: &mut App, value: &str) {
    let tick=next_tick(app);
    let input=[TextEdit::Clear,TextEdit::Commit(value.into())].into_iter().map(|edit| InputEvent {
        at:tick, source:Source::Script, kind:InputKind::Text(edit),
    }).collect();
    bridge::frame(&mut app.pages,&mut app.bridge,tick,input);
    runtime_tick(app); settle_pages(app);
    for key in [Key::Down,Key::Ok] { press_without_runtime(app,key); runtime_tick(app); settle_pages(app); }
}
fn validate(app: &mut App) -> ott_catalog::LocalBindings {
    let r=app.ott.as_mut().unwrap();
    let Some(ott_catalog::Task::LocalBindings(request))=transient::take_binding_task(r) else {panic!("existing binding task")};
    ott_catalog::validate_local_bindings_for_test(&plx_base::task::OffFrame::for_test(),request,&r.cancel).unwrap()
}
/// Reuse actual validation and candidate construction, then model only the
/// accepted writer slot. Job::Saved/Uncertain below use their real consumers.
fn staged(app:&mut App) -> (Saved,crate::app::ott_local_root::LocalSource) {
    let bound=validate(app); assert!(bound.operation_error.is_none());
    let source=bound.sources.iter().find(|s| s.descriptor().canonical_path.as_deref()==bound.lookup.as_deref()).unwrap().clone();
    assert!(source.is_live());
    let mut r=app.ott.take().unwrap(); let mut candidate=r.transient.take_local_candidate().unwrap();
    candidate.inventory=bound.sources;
    let saved=local_profile::prepare_bound(&mut r,app,&mut candidate,source.descriptor()).unwrap();
    candidate.phase=transient::CandidatePhase::SavingConnection(source.descriptor().clone());
    r.transient.put_local_candidate(candidate);
    r.saving=true; r.pending_editor_save=r.edit_session.clone();
    r.save_ui_owner=r.edit_session.as_ref().map(|e|e.owner);
    r.save_navigation=SaveNavigation::Activate(r.navigation_epoch); r.save_playback_epoch=r.playback_epoch;
    app.ott=Some(r); (saved,source)
}
fn complete(app:&mut App,job:Job) {
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_|panic!("receiver"));
    runtime_tick(app); settle_pages(app);
}

#[test]
fn local_profile_add_and_edit_publish_the_validated_grant_only_after_saved() {
    let _serial=plx_base::testlock::serial();
    let temp=local_integration::TempDir::new(); let first=temp.folder("first"); let second=temp.folder("second");
    let mut app=fixture();
    let mut ids=Vec::new();
    for (key,path) in [("profile.add",first.clone()),("profile.source",first),("profile.source",second)] {
        let editor=open(&mut app,key); let before=serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let inventory_before=app.ott.as_ref().unwrap().local_inventory.sources.clone();
        submit(&mut app,&editor,&path);
        assert!(app.ott.as_ref().unwrap().pending_editor_save.is_none());
        let (saved,held)=staged(&mut app);
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),before);
        let inventory=&app.ott.as_ref().unwrap().local_inventory.sources;
        assert_eq!(inventory.len(),inventory_before.len());
        assert!(inventory.iter().zip(&inventory_before).all(|(after,before)| {
            let a=after.descriptor(); let b=before.descriptor();
            a.id==b.id && a.name==b.name && a.kind==b.kind && a.available==b.available
                && a.authority_ref==b.authority_ref && a.generation==b.generation
                && a.canonical_path==b.canonical_path && after.same_grant(before)
        }),
            "validation does not publish or replace any live inventory entry");
        if !inventory_before.iter().any(|s|s.same_grant(&held)) {
            assert!(!inventory.iter().any(|s|s.same_grant(&held)),"new grant is still staged");
        }
        complete(&mut app,Job::Saved(Ok(saved),None,true,ok(json!({}))));
        let r=app.ott.as_ref().unwrap(); let profile=r.active().unwrap().clone();
        assert_eq!(profile.kind,ProviderKind::Local); assert_eq!(profile.source,path);
        assert_eq!(r.saved.profiles.profiles.len(),1,"edit replaces the same profile");
        assert!(r.transient.local_candidate().is_none() && r.pending_editor_save.is_none());
        let folders=RememberedFolders::read(&r.saved.settings).unwrap();
        assert!(folders.folders().iter().any(|f|f.path==path && f.id==held.descriptor().id));
        let ott_catalog::Task::LocalFolder{source,..}=r.local_catalog_task(profile).unwrap() else{panic!("ordinary grant-aware loader")};
        assert!(source.same_grant(&held)); ids.push(held.descriptor().id.clone());
    }
    assert_eq!(ids[0],ids[1],"the same canonical folder keeps its identity");
    assert_ne!(ids[1],ids[2],"editing the profile to another folder cannot rebind an old source ID");
}

#[test]
fn duplicate_local_submit_preserves_its_generation_navigation_and_binding_task() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path);
    let r=app.ott.as_ref().unwrap(); let state=(r.generation,r.navigation_epoch); let cancel=r.cancel.clone();
    submit(&mut app,&editor,&path);
    let r=app.ott.as_mut().unwrap(); assert_eq!((r.generation,r.navigation_epoch),state);
    assert!(Arc::ptr_eq(&cancel,&r.cancel)); assert!(!cancel.load(Ordering::Acquire));
    assert!(transient::take_binding_task(r).is_some()); assert!(transient::take_binding_task(r).is_none());
    assert!(r.pending_editor_save.is_none() && !r.saving);
}

#[test]
fn missing_folder_and_writer_refusal_leave_no_profile_or_live_inventory() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("exists");
    for missing in [true,false] {
        let mut app=fixture(); let editor=open(&mut app,"profile.add");
        submit(&mut app,&editor,&if missing {format!("{path}/missing")} else {path.clone()});
        let bound=validate(&mut app);
        let mut r=app.ott.take().unwrap(); let generation=r.generation;
        if !missing { r.saving=true; } // an unrelated writer owns the slot before validation finishes
        transient::binding_result(&mut r,&mut app,generation,Ok(bound));
        local_profile::drain(&mut r,&mut app);
        assert!(r.transient.local_candidate().is_none()); assert!(r.pending_editor_save.is_none());
        assert!(r.saved.profiles.profiles.is_empty() && r.local_inventory.sources.is_empty());
        assert!(r.rx.try_recv().is_err(),"neither failure starts a writer"); app.ott=Some(r);
    }
}

#[test]
fn cancelled_or_replaced_validation_cannot_save_or_retire_new_playback() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    for replacement in 0..3 {
        let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path); let bound=validate(&mut app);
        let mut r=app.ott.take().unwrap(); let generation=r.generation;
        match replacement {
            0 => r.cancel_editor(),
            1 => r.playback_epoch=r.playback_epoch.wrapping_add(1),
            _ => {
                let before=plx_media::route::play_request_ticket();
                plx_media::route::cancel_play(&mut app.player.session);
                assert!(!plx_media::route::play_request_is_current(before));
            }
        }
        let cancel=r.cancel.clone(); let playback=r.playback_epoch;
        transient::binding_result(&mut r,&mut app,generation,Ok(bound));
        assert!(r.transient.local_candidate().is_none() && !r.saving);
        assert!(r.saved.profiles.profiles.is_empty() && r.local_inventory.sources.is_empty());
        if replacement != 0 { assert_eq!(r.generation,generation); assert!(Arc::ptr_eq(&r.cancel,&cancel)); assert!(!cancel.load(Ordering::Acquire)); }
        assert_eq!(r.playback_epoch,playback); app.ott=Some(r);
    }
}

#[test]
fn failed_or_uncertain_save_releases_staging_and_uncertain_can_reconcile_then_retry() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    for uncertain in [false,true] {
        let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path); let (_saved,_held)=staged(&mut app);
        complete(&mut app,if uncertain {Job::Uncertain(None,None)} else {Job::Saved(Err("fixture refused"),None,true,ok(json!({})))});
        let r=app.ott.as_ref().unwrap(); assert!(r.transient.local_candidate().is_none()); assert!(r.pending_editor_save.is_none());
        assert!(r.local_inventory.sources.is_empty() && r.saved.profiles.profiles.is_empty());
        if uncertain {
            assert!(r.persistence_uncertain && !r.loaded);
            assert!(!app.ott.as_mut().unwrap().start_state_load_with(|_,_| {
                panic!("an uncertain runtime must not admit another state loader")
            }));
            // Even a delayed successful load cannot clear this process's quarantine.
            complete(&mut app,Job::Loaded(Ok((Saved::default(),[7;16]))));
            let r=app.ott.as_ref().unwrap();
            assert!(r.persistence_uncertain);
            assert_eq!(r.settings_write_refusal(),Some(
                "Settings write needs reconciliation; restart the player before another change"));

            // Restart creates a new owner. Model its verified old-state load,
            // not an in-process reset of the uncertain owner's safety latch.
            drop(app);
            app=app_fixture();
            {
                let r=app.ott.as_mut().unwrap();
                assert!(!r.loaded && !r.persistence_uncertain);
                r.catalog_done.store(false,Ordering::Release);
                r.metadata_done.store(false,Ordering::Release);
                r.resolver_busy=true; r.plex_refresh_pending=true; r.write_permit.revoke();
            }
            complete(&mut app,Job::Loaded(Ok((Saved::default(),[7;16]))));
            {
                let r=app.ott.as_mut().unwrap();
                assert!(r.loaded && !r.persistence_uncertain);
                assert!(r.transient.local_candidate().is_none() && r.pending_editor_save.is_none());
                assert!(r.local_inventory.sources.is_empty() && r.saved.profiles.profiles.is_empty());
                r.provider=ProviderKind::Local; r.page="profiles";
            }
            bridge::show_page(&mut app.pages,AppArg::Ott);
            settle_pages(&mut app); runtime_tick(&mut app); settle_pages(&mut app);
        }
        let retry=open(&mut app,"profile.add"); submit(&mut app,&retry,&path);
        assert!(app.ott.as_ref().unwrap().transient.local_candidate().is_some(),"terminal old write cannot block retry");
    }
}

#[test]
fn saved_with_changed_descriptor_kind_or_departing_editor_never_publishes_staging() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    for change in 0..4 {
        let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path); let (mut saved,held)=staged(&mut app);
        match change {
            0 => { let stored=saved.settings.get_mut("nativeLocalSources").unwrap(); stored["version"]=json!(3); stored["folders"][0]["kind"]=json!("file"); saved.validate().unwrap(); },
            1 => { let binding=crate::app::ott_local_sources::UsbBinding::from_observed("ext4","different-volume","/","source").unwrap();
                let stored=saved.settings.get_mut("nativeLocalSources").unwrap();stored["version"]=json!(2);stored["folders"][0]["usb"]=serde_json::to_value(binding).unwrap();saved.validate().unwrap(); },
            2 => { app.ott.as_mut().unwrap().cancel_editor(); },
            _ => { app.ott.as_mut().unwrap().playback_epoch+=1; },
        }
        complete(&mut app,Job::Saved(Ok(saved),None,true,ok(json!({}))));
        let r=app.ott.as_ref().unwrap(); assert!(r.transient.local_candidate().is_none());
        assert!(!r.local_inventory.sources.iter().any(|s|s.same_grant(&held)));
        assert!(r.play_target.is_none());
    }
}

#[test]
fn same_session_retry_discards_old_failure_before_screen_dispatch() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit_screen(&mut app,&path);
    assert!(app.ott.as_ref().unwrap().transient.local_candidate().is_some());
    let mut r=app.ott.take().unwrap(); let generation=r.generation;
    transient::binding_result(&mut r,&mut app,generation,Err(crate::app::ott_local_session::LocalError::SourceNotFound));
    assert!(r.transient.local_candidate().is_none());
    // An already queued retry arrives before the normal end-of-tick drain.
    apply_ui_action(&mut r,&mut app,OttAction::SubmitSetting{session:editor.id,key:editor.key.clone(),value:path},true,false,false);
    local_profile::drain(&mut r,&mut app); assert!(r.transient.local_candidate().is_some()); app.ott=Some(r);
    settle_pages(&mut app); let _ = app.bridge.take_ott_reqs();
    press_without_runtime(&mut app,Key::Ok);
    assert!(!app.bridge.take_ott_reqs().iter().any(|out| matches!(out,
        OttOut::Action{action:OttAction::SubmitSetting{..},..})),"pending screen must not be unlocked by A's stale error");
}

#[test]
fn local_ui_uses_collision_free_id_and_preserves_http_source_path() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let old=temp.folder("old"); let path=temp.folder("new");
    let mut app=fixture();
    {let r=app.ott.as_mut().unwrap(); RememberedFolders::read(&r.saved.settings).unwrap().with_connected(FolderDescriptor{
        id:"local-ui-0".into(),name:"old".into(),path:old,kind:LocalSourceKind::Folder,usb:None}).unwrap().put_into(&mut r.saved.settings).unwrap();}
    {let r=app.ott.as_mut().unwrap();let source=local_integration::acquire_root(crate::app::ott_local_session::BoundSource{
        id:"local-ui-1".into(),name:"live".into(),kind:crate::app::ott_local_session::SourceKind::Folder,
        available:true,authority_ref:crate::app::ott_local_session::AuthorityRef(9),generation:r.generation,
        canonical_path:Some(temp.folder("live")),});r.local_inventory.install(vec![source],r.generation);}
    let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path);
    let Some(ott_catalog::Task::LocalBindings(request))=transient::take_binding_task(app.ott.as_mut().unwrap()) else{panic!()};
    assert_eq!(request.new_id,"local-ui-2");
    let mut app=fixture(); let editor=open(&mut app,"profile.add");
    app.ott.as_mut().unwrap().saving=true; // force ordinary writer refusal, no actual thread
    submit(&mut app,&editor,"https://media.invalid/local.json");
    let r=app.ott.as_ref().unwrap(); assert!(r.transient.local_candidate().is_none());
    assert_eq!(r.status,"Another settings operation is in progress");
    assert!(r.saved.profiles.profiles.is_empty());
}

#[test]
fn usb_path_intent_retains_the_existing_save_then_verified_picker_route() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let remembered=temp.folder("usb-rebound");
    for (path,portable) in [("/tmp/usb/disk/part/Deep/Movies".to_owned(),false),(remembered.clone(),true)] {
        let mut app=fixture();
        if portable {
            let r=app.ott.as_mut().unwrap();
            RememberedFolders::read(&r.saved.settings).unwrap().with_connected(FolderDescriptor {
                id:"saved-usb".into(),name:"Movies".into(),path:path.clone(),kind:LocalSourceKind::Folder,
                usb:Some(crate::app::ott_local_sources::UsbBinding::from_observed("ext4","saved-volume","/","Deep/Movies").unwrap()),
            }).unwrap().put_into(&mut r.saved.settings).unwrap();
            assert!(!crate::app::ott_usb::is_usb_path(&path),"portable metadata also routes the intent");
        }
        assert!(app.ott.as_ref().unwrap().usb_folder_path(&path).unwrap());
        let editor=open(&mut app,"profile.add"); app.ott.as_mut().unwrap().saving=true;
        submit(&mut app,&editor,&path);
        let r=app.ott.as_ref().unwrap();
        assert_eq!(r.status,"Another settings operation is in progress","intent reached the ordinary writer");
        assert!(r.transient.local_candidate().is_none() && r.local_inventory.sources.is_empty());
        let mut profile=r.edit_profile.clone().unwrap(); profile.source=path;
        assert!(matches!(r.local_catalog_task(profile),Err("Local folder source is unavailable or ambiguous")),
            "a source string alone is never a USB grant");
    }
}

#[test]
fn unrelated_saved_during_validation_does_not_consume_or_acknowledge_the_editor() {
    let _serial=plx_base::testlock::serial(); let temp=local_integration::TempDir::new(); let path=temp.folder("source");
    let mut app=fixture(); let editor=open(&mut app,"profile.add"); submit(&mut app,&editor,&path);
    let saved=app.ott.as_ref().unwrap().saved.clone();
    {let r=app.ott.as_mut().unwrap();r.saving=true;r.save_navigation=SaveNavigation::Preserve;r.save_ui_owner=None;}
    complete(&mut app,Job::Saved(Ok(saved),None,false,ok(json!({}))));
    let r=app.ott.as_ref().unwrap();
    assert!(local_profile::pending(r,editor.owner,editor.id));
    assert!(r.edit_session.as_ref()==Some(&editor)); assert!(r.pending_editor_save.is_none());
    assert!(r.local_inventory.sources.is_empty() && r.saved.profiles.profiles.is_empty());
}

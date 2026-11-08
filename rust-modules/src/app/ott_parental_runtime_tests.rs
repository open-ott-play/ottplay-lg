//! Actual editor/runtime gate tests. Only one case performs real PIN derivation.
use super::*;
use std::time::Duration;
const RECORD:&str="pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65";
fn fixture() -> App {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    runtime.write_permit.revoke();
    runtime
        .saved
        .settings
        .insert("parentPin".into(), json!(RECORD));
    app.ott = Some(runtime);
    crate::app::bridge::show_page(&mut app.pages, plx_screens::registry::AppArg::Ott);
    for n in 1..=24 {
        crate::app::bridge::frame(
            &mut app.pages,
            &mut app.bridge,
            plx_machine::machine::Tick {
                ms: n * 16,
                dt_us: 16000,
            },
            Vec::new(),
        );
    }
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    super::super::tick(&mut app, sample);
    assert!(
        playback_allowed(&app),
        "fixture must pass real foreground synchronization"
    );
    let owner = app.pages.top_page().unwrap();
    app.ott.as_mut().unwrap().reply = Some(owner);
    app
}
fn act(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}
fn profile(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: id.into(),
        kind: ProviderKind::M3u,
        source: "https://example.invalid/list.m3u".into(),
        source_alias: id.into(),
        enabled: true,
    }
}
fn begin_without_running(app: &mut App) -> parental::Work {
    let mut runtime = app.ott.take().unwrap();
    open(&mut runtime, app, UNLOCK, "Parental access PIN").unwrap();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    let work = runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .verify(&context, Pin::parse("2468").unwrap(), now)
        .unwrap();
    let request = runtime.edit_session.clone().unwrap();
    let mut retained = None;
    start_with(&mut runtime, request, work, |work, _sender| {
        retained = Some(work);
        true
    })
    .unwrap();
    let work = retained.unwrap();
    app.ott = Some(runtime);
    work
}
#[test]
fn current_secret_editor_verifies_real_pin_and_grant_never_crosses_profiles() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    {
        let r = app.ott.as_mut().unwrap();
        r.saved.profiles.profiles = vec![profile("a"), profile("b")];
        r.saved.profiles.active_profile = Some("a".into());
    }
    assert!(app
        .ott
        .as_mut()
        .unwrap()
        .require_parental_control()
        .is_err());
    act(&mut app, OttAction::EditSetting(UNLOCK.into())).unwrap();
    let session = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    act(
        &mut app,
        OttAction::SubmitSetting {
            session,
            key: UNLOCK.into(),
            value: "2468".into(),
        },
    )
    .unwrap();
    assert!(
        !app.ott.as_ref().unwrap().saving,
        "verification is not a settings write"
    );
    let job = app
        .ott
        .as_ref()
        .unwrap()
        .rx
        .recv_timeout(Duration::from_secs(30))
        .expect("bounded PIN worker");
    let Job::Parental(completion) = job else {
        panic!("expected the addressed PIN result")
    };
    let mut runtime = app.ott.take().unwrap();
    completed(&mut runtime, &mut app, completion);
    assert!(runtime.require_parental_control().is_ok());
    assert!(runtime.require_parental_playback(&profile("a")).is_ok());
    assert!(
        runtime.require_parental_playback(&profile("b")).is_err(),
        "A grant must not move to B"
    );
    // Catalog, play and queue sequencing are not a new logical source grant.
    runtime.generation = runtime.generation.wrapping_add(1);
    runtime.play_generation = runtime.play_generation.wrapping_add(1);
    runtime.parental_source_installed();
    assert!(
        runtime.require_parental_control().is_ok(),
        "same logical source retains its grant"
    );
    runtime.saved.profiles.active_profile = Some("b".into());
    runtime.parental_source_installed();
    // No access/read/tick between these accepted assignments: only the install
    // hook can observe B before the source returns to the same A identity.
    runtime.saved.profiles.active_profile = Some("a".into());
    runtime.parental_source_installed();
    assert!(
        runtime.require_parental_control().is_err(),
        "A -> B -> A cannot resurrect the grant"
    );
    runtime.parental_uncertain();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    assert!(runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .verify(&context, Pin::parse("2468").unwrap(), now)
        .is_err());
    runtime.parental_authoritative_load();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    let work = runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .verify(&context, Pin::parse("2468").unwrap(), now)
        .expect("authoritative same-policy reload clears Unknown");
    runtime.retire_parental_completion(parental::Completion::failed(work.ticket()));
    drop(work);
    assert!(
        runtime.require_parental_control().is_err(),
        "verified reload clears uncertainty without granting"
    );
    runtime.saved.settings.remove("parentPin");
    runtime.parental_authoritative_load();
    assert!(
        runtime.require_parental_control().is_ok(),
        "same owner can use verified disabled policy"
    );
    app.ott = Some(runtime);
}
#[test]
fn actual_finished_work_drains_when_protected_state_is_unavailable() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let work = begin_without_running(&mut app);
    let ticket = work.ticket();
    drop(work);
    let mut runtime = app.ott.take().unwrap();
    runtime.loaded = false;
    completed(&mut runtime, &mut app, parental::Completion::failed(ticket));
    runtime.loaded = true;
    runtime.parental_authoritative_load();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    assert!(
        runtime
            .parental
            .gate
            .as_mut()
            .unwrap()
            .verify(&context, Pin::parse("2468").unwrap(), now)
            .is_ok(),
        "real ended slot must be reusable"
    );
    assert!(runtime.require_parental_control().is_err());
    app.ott = Some(runtime);
}
#[test]
fn canceled_old_result_never_grants_or_reopens_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let work = begin_without_running(&mut app);
    let ticket = work.ticket();
    drop(work);
    let session = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    act(&mut app, OttAction::CancelSetting { session }).unwrap();
    let mut runtime = app.ott.take().unwrap();
    completed(
        &mut runtime,
        &mut app,
        parental::Completion::verified_fixture(ticket),
    );
    assert!(runtime.edit_session.is_none());
    assert!(runtime.require_parental_control().is_err());
    assert!(!runtime.saving);
    app.ott = Some(runtime);
}
#[test]
fn missing_channels_default_toggles_off_via_real_saved_candidate_only() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott.as_mut().unwrap().saved.settings.remove("parentPin");
    act(&mut app, OttAction::EditSetting("parental.channels".into())).unwrap();
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(
        runtime.saved.settings.get("psChannels").is_none(),
        "candidate cannot publish early"
    );
    let job = runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(job, Job::Saved(Err(_), None, false, _)),
        "revoked real writer must fail before IO"
    );
}
#[test]
fn disabled_pin_and_unknown_classification_are_distinct() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let target = profile("a");
    runtime.saved.profiles.profiles.push(target.clone());
    runtime.saved.profiles.active_profile = Some("a".into());
    assert!(runtime.require_parental_playback(&target).is_err());
    assert!(!runtime.saving);
    runtime.saved.settings.remove("parentPin");
    assert!(runtime.require_parental_playback(&target).is_ok());
}

#[test]
fn salts_are_domain_separated_by_nonreused_runtime_editor_session() {
    assert_ne!(
        salt_for(Some([7; 16]), 1).unwrap(),
        salt_for(Some([7; 16]), 2).unwrap()
    );
    assert_ne!(
        salt_for(Some([7; 16]), 1).unwrap(),
        salt_for(Some([8; 16]), 1).unwrap()
    );
    assert!(salt_for(None, 1).is_err());
    assert!(salt_for(Some([7; 16]), 0).is_err());
}

#[test]
fn refused_worker_releases_its_busy_slot_without_needing_loaded_context() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    open(&mut runtime, &mut app, UNLOCK, "PIN").unwrap();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    let work = runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .verify(&context, Pin::parse("2468").unwrap(), now)
        .unwrap();
    let request = runtime.edit_session.clone().unwrap();
    runtime.loaded = false;
    assert!(start_with(&mut runtime, request, work, |_, _| false).is_err());
    runtime.loaded = true;
    runtime.parental_authoritative_load();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    assert!(runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .verify(&context, Pin::parse("2468").unwrap(), now)
        .is_ok());
    assert!(!runtime.saving);
    app.ott = Some(runtime);
}


// Synthetic worker completion isolates dispatcher policy without paying the
// PBKDF2 cost again. The existing test above verifies the real worker once.
fn grant_current_profile_for_dispatch(app: &mut App) {
    let work = begin_without_running(app);
    let ticket = work.ticket();
    drop(work);
    let mut runtime = app.ott.take().unwrap();
    completed(&mut runtime, app, parental::Completion::verified_fixture(ticket));
    assert!(runtime.require_parental_control().is_ok());
    app.ott = Some(runtime);
}
fn command_invocation(runtime: &mut Runtime, action: &str, params: Value) -> Invocation {
    let id: String = plx_base::sha256::sha256(&serde_json::to_vec(&json!([action, &params])).unwrap())
        .iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":&id[..32],"action":action,"params":params,"expires_at":1020
    }]}).to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, 100);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10000);
    invocation
}
fn profile_favorite(profile: &ProviderProfile, title: &str) -> Favorite {
    Favorite {
        title: title.into(),
        origin: plx_data::ott::favorites::Origin {
            provider: "m3u".into(), profile: profile.name.clone(),
            subprofile: None, source_alias: profile.source_alias.clone(),
        },
        locator: plx_data::ott::favorites::Locator::Channel { channel_id: format!("{}-channel",profile.id) },
    }
}
#[test]
fn expired_pin_grant_does_not_block_authenticated_strict_only_kiosk_change() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let a = profile("a");
    let favorite = profile_favorite(&a,"Favorite A");
    {
        let runtime=app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles=vec![a.clone()];
        runtime.saved.profiles.active_profile=Some(a.id.clone());
        runtime.provider=a.kind;
        let pin=crate::app::ott_kiosk_stored::Stored::item(favorite,false).unwrap();
        runtime.saved.settings.insert("nativeKiosk".into(),pin.encode().unwrap());
    }
    grant_current_profile_for_dispatch(&mut app);
    let mut runtime=app.ott.take().unwrap();
    let context=runtime.parental_context().unwrap();
    let expires=runtime.now_ms().saturating_add(3_600_000);
    assert!(matches!(runtime.parental.gate.as_mut().unwrap().access(&context,Target::Control,expires), Access::PinRequired));
    let saved=serde_json::to_value(&runtime.saved).unwrap();
    let playback=runtime.playback_epoch;
    let generation=runtime.play_generation;
    let invocation=command_invocation(&mut runtime,"kiosk",json!({"mode":"on","strict":true}));
    assert!(commands::command(&mut runtime,&mut app,&invocation).is_none());
    assert!(runtime.saving);
    assert!(commands::kiosk_pending(&runtime));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(),saved,"strictness is not published before Saved");
    assert_eq!(runtime.playback_epoch,playback);
    assert_eq!(runtime.play_generation,generation);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    // The real sole writer was admitted but its revoked permit prevents I/O.
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),Job::Saved(Err(_),Some(_),false,_)));
    app.ott=Some(runtime);
}
#[test]
fn protected_cross_profile_favorite_is_denied_by_composed_dispatch_before_play_or_pin_save() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let a=profile("a");let b=profile("b");
    let favorite=profile_favorite(&b,"Favorite B");
    let selection=format!("favorite:{}",favorite.identity());
    {
        let runtime=app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles=vec![a.clone(),b.clone()];
        runtime.saved.profiles.active_profile=Some(a.id.clone());
        runtime.provider=a.kind;
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.favorites_selected=true;
        // If admission accidentally regresses, queued resolution stays inert.
        runtime.resolver_busy=true;
    }
    grant_current_profile_for_dispatch(&mut app);
    let mut runtime=app.ott.take().unwrap();
    let (_,bound,_,_)=runtime.prepare_play_request(PlaybackRequest::ordinary(&selection,0)).unwrap();
    assert_eq!(bound.id,b.id,"the actual Favorite must bind to profile B");
    assert!(runtime.require_parental_playback(&a).is_ok());
    let saved=serde_json::to_value(&runtime.saved).unwrap();
    let generation=runtime.play_generation;
    let playback=runtime.playback_epoch;
    let ticket=plx_media::route::play_request_ticket();
    for (action,params) in [
        ("play",json!({"query":"Favorite B"})),
        ("kiosk",json!({"mode":"on","query":"Favorite B"})),
    ] {
        let invocation=command_invocation(&mut runtime,action,params);
        let response=commands::command(&mut runtime,&mut app,&invocation).unwrap();
        assert_eq!(response["status"],"rejected");
        assert!(response.to_string().contains("Select and unlock the target profile"),"{response}");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(),saved);
        assert!(!runtime.saving);
        assert!(!commands::kiosk_pending(&runtime));
        assert!(runtime.play_target.is_none());
        assert!(runtime.resolver_next.is_none());
        assert_eq!(runtime.play_generation,generation);
        assert_eq!(runtime.playback_epoch,playback);
        assert!(plx_media::route::play_request_is_current(ticket));
        assert!(runtime.rx.try_recv().is_err());
    }
    let error=handle_action(&mut runtime,&mut app,OttAction::Play(selection)).unwrap_err();
    assert!(error.contains("Select and unlock the target profile"));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(),saved);
    assert!(!runtime.saving);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    assert_eq!(runtime.play_generation,generation);
    assert_eq!(runtime.playback_epoch,playback);
    assert!(plx_media::route::play_request_is_current(ticket));
    app.ott=Some(runtime);
}

#[path = "ott_m3u_kiosk_admission_tests.rs"]
mod m3u_kiosk_admission;

// Existing App/dispatcher fixture, with no verifier, storage or network worker.
// Keep these helpers local: the regression needs the screen's real Pending state.
fn draft_tick(app: &mut App) -> plx_machine::machine::Tick {
    app.loop_t = app.loop_t.max(24 * 16).saturating_add(16);
    plx_machine::machine::Tick { ms: app.loop_t, dt_us: 16_000 }
}
fn draft_settle(app: &mut App) {
    for _ in 0..24 {
        let tick = draft_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
}
fn draft_key(app: &mut App, key: plx_machine::machine::Key) {
    let tick = draft_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
    draft_settle(app);
}
fn draft_actions(app: &mut App) -> Vec<OttAction> {
    app.bridge.take_ott_reqs().into_iter().filter_map(|out| match out {
        OttOut::Action { action: action @ OttAction::SubmitSetting { .. }, .. } => Some(action),
        _ => None,
    }).collect()
}
fn draft_submit(app: &mut App, session: &EditorSession, value: &str) -> OttAction {
    use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};
    let tick = draft_tick(app);
    let edits = [TextEdit::Clear, TextEdit::Commit(value.into())].into_iter()
        .map(|edit| InputEvent { at: tick, source: Source::Script, kind: InputKind::Text(edit) })
        .collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, edits);
    draft_settle(app);
    draft_key(app, Key::Down);
    draft_key(app, Key::Ok);
    let mut actions = draft_actions(app);
    assert_eq!(actions.len(), 1, "real Save must emit one addressed draft");
    let action = actions.remove(0);
    assert_eq!(action, OttAction::SubmitSetting {
        session: session.id, key: session.key.clone(), value: value.into(),
    });
    action
}
fn draft_retry(app: &mut App, session: &EditorSession, value: &str) {
    use plx_machine::machine::Key;
    for key in [Key::Up, Key::Up, Key::Down, Key::Ok] { draft_key(app, key); }
    assert_eq!(draft_actions(app), vec![OttAction::SubmitSetting {
        session: session.id, key: session.key.clone(), value: value.into(),
    }], "refused draft remains editable and retryable without text replacement");
}

#[test]
fn expired_settings_grant_preserves_the_submitted_draft_instead_of_opening_pin() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("psOptions".into(), json!(1));
    grant_current_profile_for_dispatch(&mut app);
    draft_settle(&mut app);
    act(&mut app, OttAction::EditSetting("remote.address".into())).unwrap();
    draft_settle(&mut app);
    let session = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    let value = "https://control.invalid/before-grant-expiry";
    let action = draft_submit(&mut app, &session, value);
    let mut runtime = app.ott.take().unwrap();
    let context = runtime.parental_context().unwrap();
    let expiry = runtime.now_ms().saturating_add(3_600_000);
    assert!(matches!(runtime.parental.gate.as_mut().unwrap().access(&context, Target::Settings, expiry), Access::PinRequired));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert_eq!(handle_action(&mut runtime, &mut app, action), Err(LOCKED));
    assert!(runtime.edit_session.as_ref() == Some(&session), "Save refusal must not replace the draft with parental.unlock");
    assert!(runtime.pending_editor_save.is_none());
    assert!(!runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    app.ott = Some(runtime);
    draft_settle(&mut app);
    draft_retry(&mut app, &session, value);
}

#[test]
fn refused_parental_submission_receives_its_own_pending_result() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    act(&mut app, OttAction::EditSetting(UNLOCK.into())).unwrap();
    draft_settle(&mut app);
    let session = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    let action = draft_submit(&mut app, &session, "2468");
    let mut runtime = app.ott.take().unwrap();
    // A strict policy can arrive after this editor opened, before its first work.
    let favorite = profile_favorite(&profile("pin-source"), "Fixture channel");
    let pin = crate::app::ott_kiosk_stored::Stored::item(favorite, true).unwrap();
    runtime.saved.settings.insert("nativeKiosk".into(), pin.encode().unwrap());
    assert!(crate::app::ott_runtime::input_policy::strict(&runtime));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(handle_action(&mut runtime, &mut app, action).is_err());
    assert!(runtime.parental.worker.is_none());
    assert!(runtime.edit_session.as_ref() == Some(&session));
    assert!(!runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    app.ott = Some(runtime);
    draft_settle(&mut app);
    draft_retry(&mut app, &session, "2468");
}

#[test]
fn duplicate_parental_submission_keeps_the_original_worker_pending() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let work = begin_without_running(&mut app);
    let ticket = work.ticket();
    drop(work);
    draft_settle(&mut app);
    let session = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    let action = draft_submit(&mut app, &session, "2468");
    let queued = app.pages.queued();
    assert!(act(&mut app, action).is_ok(), "duplicate work is consumed without a rejection ACK");
    assert_eq!(app.pages.queued(), queued);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.parental.worker.as_ref().is_some_and(|pending| pending.ticket == ticket && pending.request == session));
    assert!(runtime.edit_session.as_ref() == Some(&session));
    assert!(!runtime.saving);
    // Only the original worker result may release this pending editor.
    let mut runtime = app.ott.take().unwrap();
    completed(&mut runtime, &mut app, parental::Completion::verified_fixture(ticket));
    assert!(runtime.edit_session.is_none());
    assert!(runtime.parental.worker.is_none());
    assert!(runtime.require_parental_control().is_ok());
    app.ott = Some(runtime);
    draft_settle(&mut app);
}


struct ClassicRegistryGuard;
impl ClassicRegistryGuard {
    fn new() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for ClassicRegistryGuard {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}
fn classic_parental_app(durable: bool) -> (App, plx_plex::plex::ServerId) {
    let mut app = fixture();
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = 71;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = crate::app::bridge::Bridge::for_session_test(init);
    let origin = plx_plex::plex::Origin::parse("https://classic-parental.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        "classic-parental", &origin, "synthetic-token", None, "fixture", Default::default());
    assert!(plx_plex::plex::set_current(sid));
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    if durable {
        runtime.saved.profiles.profiles.push(ProviderProfile { id: "kept-classic".into(),
            name: "Fixture".into(), kind: ProviderKind::Plex, source: "classic-parental".into(),
            source_alias: String::new(), enabled: true });
        runtime.saved.profiles.active_profile = Some("kept-classic".into());
    }
    assert!(runtime.require_parental(Target::Providers).is_ok());
    assert!(classic_source::ensure_server_catalog(&mut runtime, &mut app, sid).unwrap());
    app.ott = Some(runtime);
    (app, sid)
}

#[test]
fn classic_unsaved_source_uses_the_existing_pin_editor_and_grant() {
    let _serial = plx_base::testlock::serial();
    let _registry = ClassicRegistryGuard::new();
    let (mut app, _) = classic_parental_app(false);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.saved.profiles.profiles.is_empty());
    assert_eq!(runtime.require_parental_playback(&profile), Err(LOCKED));
    let work = begin_without_running(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().key, UNLOCK);
    let ticket = work.ticket();
    drop(work);
    let mut runtime = app.ott.take().unwrap();
    completed(&mut runtime, &mut app, parental::Completion::verified_fixture(ticket));
    assert!(runtime.require_parental_playback(&profile).is_ok());
    assert!(runtime.edit_session.is_none() && !runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    app.ott = Some(runtime);
}

#[test]
fn classic_parental_grant_does_not_cross_network_or_session_rebinding() {
    let _serial = plx_base::testlock::serial();
    let _registry = ClassicRegistryGuard::new();
    for network in [false, true] {
        let (mut app, sid) = classic_parental_app(true);
        grant_current_profile_for_dispatch(&mut app);
        let mut runtime = app.ott.take().unwrap();
        let profile = runtime.effective_profile().unwrap().clone();
        let authority = transient::parental_source_authority(&runtime).unwrap().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        if network { plx_plex::plex::grant::network_changed(); }
        else {
            let mut init = app.bridge.snapshot_session_init();
            init.epoch += 1;
            app.bridge = crate::app::bridge::Bridge::for_session_test(init);
        }
        classic_source::reconcile(&mut runtime, &app);
        assert!(classic_source::ensure_server_catalog(&mut runtime, &mut app, sid).unwrap());
        assert_eq!(runtime.effective_profile(), Some(&profile), "durable profile identity is unchanged");
        let current = transient::parental_source_authority(&runtime).unwrap().unwrap();
        assert!(!authority.same_authority(&current));
        assert_eq!(runtime.require_parental_playback(&profile), Err(LOCKED));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving);
        app.ott = Some(runtime);
    }
}

#[test]
fn classic_stale_pin_completion_cannot_unlock_a_new_source_receipt() {
    let _serial = plx_base::testlock::serial();
    let _registry = ClassicRegistryGuard::new();
    let (mut app, sid) = classic_parental_app(true);
    let work = begin_without_running(&mut app);
    let ticket = work.ticket();
    drop(work);
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    plx_plex::plex::grant::network_changed();
    classic_source::reconcile(&mut runtime, &app);
    assert!(classic_source::ensure_server_catalog(&mut runtime, &mut app, sid).unwrap());
    completed(&mut runtime, &mut app, parental::Completion::verified_fixture(ticket));
    let profile = runtime.effective_profile().unwrap().clone();
    assert_eq!(runtime.require_parental_playback(&profile), Err(LOCKED));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving);
    app.ott = Some(runtime);
}

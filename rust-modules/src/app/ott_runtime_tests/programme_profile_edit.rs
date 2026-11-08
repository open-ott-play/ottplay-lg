//! Targeted profile editing through the shared OTT route; worker results are injected.
use super::*;
use plx_machine::machine::Key;

#[path = "programme_stalker_active.rs"]
mod stalker_active;
#[path = "programme_profile_edit_sources.rs"]
mod sources;

fn portal(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(), name: format!("Portal {id}"), kind: ProviderKind::Stalker,
        source: json!({"portal":format!("https://{id}.invalid/c/"),"mac":"02:00:00:00:00:01"}).to_string(),
        source_alias: String::new(), enabled: true,
    }
}
fn mounted(mode: u8) -> App {
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    // Model an already-saved paused position before making editors writable.
    // Otherwise the first tick starts a resume save and disables active Edit.
    assert!(runtime.saving);
    runtime.checkpoint(&app, false);
    assert!(!runtime.checkpoints.is_empty());
    runtime.saved.resume.extend(runtime.checkpoints.clone());
    runtime.saving = false;
    runtime.entropy = Some([3; 16]);
    // Keep the actual already-playing finite route while the active settings profile is A.
    runtime.saved.profiles.profiles.extend([portal("a"), portal("b")]);
    runtime.saved.profiles.active_profile = Some("a".into());
    runtime.saved.settings.insert("interfaceMode".into(), json!(mode));
    runtime.page = "profiles";
    runtime.provider = ProviderKind::Stalker;
    runtime.profile_focus = Some("a".into());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    app.ott = Some(runtime);
    app.bridge.apply_ui_mode(if mode == 0 { plx_machine::machine::UiMode::Classic } else { plx_machine::machine::UiMode::Modern });
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    action(&mut app, OttAction::ShowProfiles);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.navigation_epoch > 0);
    assert!(!runtime.saving, "mount must not start a background settings write");
    assert!(runtime.settings_write_refusal().is_none());
    app
}
fn edit(app: &App, target: &str) -> OttAction {
    app.ott.as_ref().unwrap().view().rows.into_iter().find_map(|row| match &row.action {
        OttAction::EditProfile { target: id, .. } if id == target => Some(row.action),
        _ => None,
    }).expect("each enabled Stalker profile has an explicit Edit row")
}
fn press(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
    runtime_tick(app);
    settle_pages(app);
}
fn action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}
fn checked(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}
fn saved(app: &mut App, image: Saved) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_ui_owner = None;
    runtime.tx.send(Job::Saved(Ok(image), None, false, json!({}))).ok().unwrap();
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn profile_edit_row_keyboard_and_cancel_preserve_active_source_and_playback_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [0, 1] {
        let mut app = mounted(mode);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let identity = route::playback_identity(&app.player.session);
        let generation = app.ott.as_ref().unwrap().generation;
        let view = app.ott.as_ref().unwrap().view();
        assert_eq!(app.ott.as_ref().unwrap().interface_mode().index(), mode);
        assert_eq!(app.bridge.ui_mode().index(), mode);
        let start = view.rows.iter().position(|row| row.id == "a").unwrap();
        let target = view.rows.iter().position(|row| matches!(&row.action,
            OttAction::EditProfile { target, .. } if target == "b")).unwrap();
        assert_eq!(view.rows[target].title, "Edit Portal b");
        assert_eq!(view.rows[target].content_title.as_deref(), Some("Portal b"));
        for _ in start..target { press(&mut app, Key::Down); }
        press(&mut app, Key::Ok);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.setup.as_ref().unwrap().original.as_ref(), Some(&portal("b")));
        assert_eq!(runtime.generation, generation, "opening must not retire the catalogue");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        press(&mut app, Key::Back);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.setup.is_none());
        assert_eq!(runtime.page, "profiles");
        assert_eq!(runtime.view().selected_row.as_deref(), Some("b"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
        assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
    }
}

#[test]
fn profile_edit_stale_target_is_rejected_before_cancelling_an_existing_editor() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mutation in ["unchanged", "source", "removed", "disabled", "navigation", "owner", "provider", "target"] {
        let mut app = mounted(1);
        // Keep a real scalar editor to prove stale admission does not cancel it or replace it with a PIN.
        action(&mut app, OttAction::EditSetting("profile.name".into()));
        let mut old = edit(&app, "b");
        let runtime = app.ott.as_mut().unwrap();
        let editor = runtime.edit_session.as_ref().unwrap().id;
        match mutation {
            "unchanged" => {},
            "provider" => runtime.provider = ProviderKind::M3u,
            "target" => { if let OttAction::EditProfile { target, .. } = &mut old { *target = "a".into(); } },
            "source" => runtime.saved.profiles.profiles.iter_mut().find(|p| p.id == "b").unwrap().source.push(' '),
            "removed" => runtime.saved.profiles.profiles.retain(|p| p.id != "b"),
            "disabled" => runtime.saved.profiles.profiles.iter_mut().find(|p| p.id == "b").unwrap().enabled = false,
            "navigation" => runtime.accepted_navigation(false),
            "owner" => runtime.reply = None,
            _ => unreachable!(),
        }
        if mutation == "unchanged" {
            assert!(checked(&mut app, old).is_ok(), "the unmodified row remains admissible with a scalar editor open");
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.edit_session.is_none());
            assert_eq!(runtime.setup.as_ref().unwrap().draft.profile_id(), "b");
            continue;
        }
        runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        runtime.saved.settings.insert("psOptions".into(), json!(1));
        assert!(checked(&mut app, old).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.edit_session.as_ref().unwrap().id, editor);
        assert_eq!(runtime.edit_session.as_ref().unwrap().key, "profile.name");
        assert!(runtime.setup.is_none());
        assert!(!runtime.saving);
    }
}

#[test]
fn profile_edit_saved_aba_retires_old_rows_but_display_changes_do_not() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let old = edit(&app, "b");
    let row_id = |app: &App| app.ott.as_ref().unwrap().view().rows.into_iter()
        .find(|row| matches!(&row.action, OttAction::EditProfile { target, .. } if target == "b"))
        .unwrap().id;
    let old_row = row_id(&app);
    app.ott.as_mut().unwrap().changed();
    assert_eq!(edit(&app, "b"), old, "ordinary display refresh must not expire a profile row");
    assert_eq!(row_id(&app), old_row);
    let original = app.ott.as_ref().unwrap().saved.clone();
    let mut replacement = original.clone();
    replacement.profiles.profiles.iter_mut().find(|p| p.id == "b").unwrap().name = "Changed".into();
    saved(&mut app, replacement);
    saved(&mut app, original);
    assert!(checked(&mut app, old).is_err(), "identical restored bytes do not revive old authority");
    assert_ne!(row_id(&app), old_row, "new source context must retire a held row activation");
    let fresh = edit(&app, "b");
    assert!(checked(&mut app, fresh).is_ok());
    assert_eq!(app.ott.as_ref().unwrap().setup.as_ref().unwrap().draft.profile_id(), "b");
}

#[test]
fn profile_edit_uncertainty_retires_rows_even_for_an_identical_reloaded_image() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let old = edit(&app, "b");
    let original = app.ott.as_ref().unwrap().saved.clone();
    app.ott.as_ref().unwrap().tx.send(Job::Uncertain(Some(original.clone()), None)).ok().unwrap();
    runtime_tick(&mut app);
    // An authoritative recovery may later restore eligibility; it must not restore an old row token.
    let runtime = app.ott.as_mut().unwrap();
    runtime.persistence_uncertain = false;
    runtime.tx.send(Job::Loaded(Ok((original, [3; 16])))).ok().unwrap();
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    assert!(checked(&mut app, old.clone()).is_err());
    assert_ne!(edit(&app, "b"), old);
}

#[test]
fn profile_edit_obeys_pinned_source_and_settings_parental_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for guard in ["soft", "strict", "parental"] {
        let mut app = mounted(1);
        let offer = edit(&app, "b");
        let runtime = app.ott.as_mut().unwrap();
        let before = runtime.saved.profiles.clone();
        if guard == "parental" {
            runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
            runtime.saved.settings.insert("psOptions".into(), json!(1));
        } else {
            let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
            runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":guard=="strict","origin":origin,
                "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
            assert_eq!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Locked { strict: guard == "strict" }));
        }
        assert!(checked(&mut app, offer).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.setup.is_none());
        assert_eq!(runtime.saved.profiles, before);
        assert!(!runtime.saving);
        if guard == "parental" {
            assert_eq!(runtime.edit_session.as_ref().unwrap().key, "parental.unlock");
        }
    }
}

#[test]
fn profile_edit_save_failure_retains_target_and_success_activates_only_after_saved() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let offer = edit(&app, "b");
    action(&mut app, offer);
    assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|row|
        row.enabled && matches!(&row.action, OttAction::EditSetting(key) if key == "profile.setup.save")),
        "targeted setup uses the existing Save profile row");
    let runtime = app.ott.as_mut().unwrap();
    // Reuse the established setup-candidate and Saved-result seam; no disk write is started.
    runtime.setup.as_mut().unwrap().draft.stage("profile.setup.name", "Renamed B").unwrap();
    let (id, candidate) = runtime.setup_candidate().unwrap();
    assert_eq!(runtime.active().unwrap().id, "a");
    assert_eq!(candidate.profiles.active_profile.as_deref(), Some("b"));
    runtime.pending_setup_save = Some(id);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Err("Fixture refused"), None, true, json!({}))).ok().unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.active().unwrap().id, "a");
    assert_eq!(runtime.setup.as_ref().unwrap().draft.value("profile.setup.name"), Ok("Renamed B"));
    runtime.pending_setup_save = Some(id);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(candidate), None, true, json!({}))).ok().unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.active().unwrap().id, "b");
    assert_eq!(runtime.active().unwrap().name, "Renamed B");
    assert_eq!(runtime.saved.profiles.profiles.iter().find(|p| p.id=="a"), Some(&portal("a")));
    assert!(runtime.setup.is_none());
}

#[test]
fn profile_edit_epoch_exhaustion_never_revives_offers() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let runtime = app.ott.as_mut().unwrap();
    runtime.profile_edit_epoch = u64::MAX;
    let old = edit(&app, "b");
    let mut next = app.ott.as_ref().unwrap().saved.clone();
    next.profiles.profiles.iter_mut().find(|p| p.id=="b").unwrap().name.push('x');
    saved(&mut app, next.clone());
    saved(&mut app, next);
    assert_eq!(app.ott.as_ref().unwrap().profile_edit_epoch, 0);
    assert!(app.ott.as_ref().unwrap().view().rows.iter().filter(|row| matches!(row.action, OttAction::EditProfile {..})).all(|row| !row.enabled));
    assert!(checked(&mut app, old).is_err());
}

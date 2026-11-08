//! Addressed source editors reuse the ordinary private form and save admission.
use super::*;
#[path = "programme_m3u_slots.rs"]
mod slots;
#[path = "programme_profile_edit_catalog.rs"]
mod catalog;

#[path = "programme_m3u_active.rs"]
mod m3u_active;

fn source_profile(kind: ProviderKind, id: &str) -> ProviderProfile {
    let profile = ProviderProfile {
        id: id.into(), name: format!("Saved {id}"), kind,
        source: match kind {
            ProviderKind::M3u => format!("https://{id}.invalid/list.m3u"),
            ProviderKind::Xtream => json!({"server":format!("https://{id}.invalid/"),
                "username":"fixture-user","password":"fixture-secret"}).to_string(),
            ProviderKind::Stalker => portal(id).source,
            ProviderKind::VPortal => format!("portal::[key:fixture-{id}]https://{id}.invalid/api/"),
            ProviderKind::Local => format!("/media/fixture-{id}"),
            ProviderKind::Plex => format!("fixture-machine-{id}"),
        },
        source_alias: format!("Source {id}"), enabled: true,
    };
    profile.validate().unwrap();
    profile
}

fn mounted_source(mode: u8, kind: ProviderKind) -> App {
    let mut app = mounted(mode);
    let mut runtime = app.ott.take().unwrap();
    for profile in &mut runtime.saved.profiles.profiles {
        if profile.id == "a" || profile.id == "b" {
            *profile = source_profile(kind, &profile.id);
        }
    }
    runtime.provider = kind;
    runtime.changed();
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    settle_pages(&mut app);
    app
}

fn private_editor_visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}

#[test]
fn addressed_source_editor_and_cancel_preserve_current_source_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [0, 1] {
        for kind in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
            ProviderKind::VPortal, ProviderKind::Local] {
            let mut app = mounted_source(mode, kind);
            let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let identity = route::playback_identity(&app.player.session);
            let generation = app.ott.as_ref().unwrap().generation;
            let view = app.ott.as_ref().unwrap().view();
            let start = view.rows.iter().position(|row| row.id == "a").unwrap();
            let target = view.rows.iter().position(|row| matches!(&row.action,
                OttAction::EditProfile { target, .. } if target == "b"))
                .expect("every editable saved kind exposes its addressed edit row");
            for _ in start..target { press(&mut app, Key::Down); }
            press(&mut app, Key::Ok);
            let runtime = app.ott.as_ref().unwrap();
            let expected = source_profile(kind, "b");
            if matches!(kind, ProviderKind::Xtream | ProviderKind::Stalker) {
                assert_eq!(runtime.setup.as_ref().unwrap().original.as_ref(), Some(&expected));
                assert_eq!(runtime.setup.as_ref().unwrap().draft.profile_id(), "b");
            } else {
                assert_eq!(runtime.edit_profile.as_ref(), Some(&expected));
                assert_eq!(runtime.edit_profile_original.as_ref(), Some(&expected));
                assert_eq!(runtime.edit_session.as_ref().unwrap().key, "profile.source");
                assert!(private_editor_visible(&app));
            }
            assert_eq!(runtime.generation, generation);
            assert_eq!(runtime.active().unwrap().id, "a");
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            assert!(!runtime.saving);
            press(&mut app, Key::Back);
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.setup.is_none());
            assert!(runtime.edit_session.is_none());
            assert!(!private_editor_visible(&app));
            assert_eq!(runtime.page, "profiles");
            assert_eq!(runtime.view().selected_row.as_deref(), Some("b"));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
            assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
        }
    }
}

#[test]
fn addressed_source_stale_offer_does_not_replace_a_new_private_draft() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for kind in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::VPortal, ProviderKind::Local] {
        for mutation in ["source", "duplicate", "provider", "navigation"] {
            let mut app = mounted_source(1, kind);
            action(&mut app, OttAction::EditSetting("profile.name".into()));
            let old = edit(&app, "b");
            let runtime = app.ott.as_mut().unwrap();
            let editor = runtime.edit_session.clone();
            let draft = runtime.edit_profile.clone();
            match mutation {
                "source" => runtime.saved.profiles.profiles.iter_mut()
                    .find(|p| p.id == "b").unwrap().source.push(' '),
                "duplicate" => runtime.saved.profiles.profiles.push(source_profile(kind, "b")),
                "provider" => runtime.provider = ProviderKind::Plex,
                "navigation" => runtime.accepted_navigation(false),
                _ => unreachable!(),
            }
            assert!(checked(&mut app, old).is_err());
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.edit_session == editor);
            assert_eq!(runtime.edit_profile, draft);
            assert!(runtime.setup.is_none());
            assert!(!runtime.saving);
        }
    }
}

#[test]
fn addressed_scalar_source_invalid_submit_retains_the_exact_saved_target() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for kind in [ProviderKind::M3u, ProviderKind::VPortal, ProviderKind::Local] {
        let mut app = mounted_source(1, kind);
        let offer = edit(&app, "b");
        action(&mut app, offer);
        let runtime = app.ott.as_ref().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        assert!(checked(&mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key.clone(), value: "invalid source".into(),
        }).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.edit_session.as_ref() == Some(&editor));
        assert_eq!(runtime.edit_profile.as_ref(), Some(&source_profile(kind, "b")));
        assert_eq!(runtime.edit_profile_original.as_ref(), Some(&source_profile(kind, "b")));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(runtime.pending_editor_save.is_none());
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
    }
}

#[test]
fn addressed_source_edit_keeps_plex_on_its_authorized_connection_flow() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted_source(1, ProviderKind::Plex);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.view().rows.iter().any(|row| matches!(row.action, OttAction::EditProfile { .. })));
    let context = runtime.profile_edit_context(&source_profile(ProviderKind::Plex, "b"));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(checked(&mut app, OttAction::EditProfile { context, target: "b".into() }).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.setup.is_none());
    assert!(runtime.edit_session.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
}

#[test]
fn addressed_m3u_save_keeps_selection_and_playback_until_explicit_load() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for target in ["a", "b"] {
        let mut app = mounted_source(1, ProviderKind::M3u);
        let identity = route::playback_identity(&app.player.session);
        let offer = edit(&app, target);
        action(&mut app, offer);
        let runtime = app.ott.as_mut().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        let original = runtime.edit_profile_original.clone().unwrap();
        let mut changed = original.clone();
        changed.source = "https://changed.invalid/list.m3u".into();
        let (candidate, reload) = runtime.profile_editor_candidate(&editor, changed.clone(), Some(&original)).unwrap();
        assert!(!reload);
        assert_eq!(candidate.profiles.active_profile.as_deref(), Some("a"));
        assert_eq!(runtime.active().unwrap().id, "a");
        // The production writer is revoked by the shared fixture. Its refusal
        // proves admission/navigation without touching the protected store.
        assert!(checked(&mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key.clone(), value: changed.source.clone(),
        }).is_ok());
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.saving);
        assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
        assert_eq!(runtime.active().unwrap().id, "a");
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Job::Saved(Err(_), None, false, _)));
        // Inject a verified result for this exact admitted candidate; preserve
        // the navigation receipt created by the real submission above.
        runtime.tx.send(Job::Saved(Ok(candidate), None, false, json!({}))).ok().unwrap();
        runtime_tick(&mut app);
        settle_pages(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.active().unwrap().id, "a");
        assert_eq!(runtime.saved.profiles.profiles.iter().find(|p| p.id == target), Some(&changed));
        assert_eq!(runtime.page, "profiles");
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
        assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
        assert!(runtime.view().rows.iter().any(|row|
            matches!(&row.action, OttAction::SelectProfile(id) if id == target)),
            "the separate Load action remains available after editing");
        assert!(checked(&mut app, OttAction::SelectProfile(target.into())).is_ok());
        let runtime = app.ott.as_ref().unwrap();
        assert!(matches!(runtime.save_navigation, SaveNavigation::Activate(_)));
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Job::Saved(Err(_), None, true, _)));
    }
}

#[test]
fn addressed_m3u_save_only_intent_cannot_leak_to_another_editor() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted_source(1, ProviderKind::M3u);
    let offer = edit(&app, "b");
    action(&mut app, offer);
    let old = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    action(&mut app, OttAction::CancelSetting { session: old.id });
    assert!(app.ott.as_ref().unwrap().profile_edit_save_only.is_none());
    let offer = edit(&app, "a");
    action(&mut app, offer);
    let current = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    assert_ne!(old.id, current.id);
    assert!(checked(&mut app, OttAction::SubmitSetting {
        session: old.id, key: old.key.clone(), value: "https://stale.invalid/list.m3u".into(),
    }).is_err());
    let runtime = app.ott.as_mut().unwrap();
    runtime.complete_profile_editor(&old, true);
    assert!(runtime.edit_session.as_ref() == Some(&current));
    assert!(runtime.profile_edit_save_only.as_ref() == Some(&current));
    action(&mut app, OttAction::EditSetting("profile.source".into()));
    let runtime = app.ott.as_mut().unwrap();
    let generic = runtime.edit_session.clone().unwrap();
    assert_ne!(current.id, generic.id);
    assert!(runtime.profile_edit_save_only.is_none());
    let original = runtime.edit_profile_original.clone().unwrap();
    let (_, reload) = runtime.profile_editor_candidate(&generic, original.clone(), Some(&original)).unwrap();
    assert!(reload, "the existing generic source editor retains its Save and load contract");
}

#[test]
fn addressed_local_source_still_passes_through_binding_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted_source(1, ProviderKind::Local);
    let offer = edit(&app, "b");
    action(&mut app, offer);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    // Direct submission must still honor the Local binding admission gate.
    assert!(playback_allowed(&app));
    app.ott_standby = true;
    assert!(!playback_allowed(&app));
    assert_eq!(checked(&mut app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key.clone(), value: "/media/new-fixture".into(),
    }), Err("Local folder connection is not available now"));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.edit_session.as_ref() == Some(&editor));
    assert_eq!(runtime.edit_profile_original.as_ref(), Some(&source_profile(ProviderKind::Local, "b")));
    assert!(runtime.transient.candidate.is_none());
    assert!(!runtime.saving);
}

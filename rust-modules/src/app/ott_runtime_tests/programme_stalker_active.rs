//! Active Stalker selection opens its existing editor instead of loading again.
use super::*;

#[test]
fn active_stalker_primary_row_edits_and_inactive_row_still_loads() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(0);
    for active in [Some("a"), Some("b"), None] {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.active_profile = active.map(str::to_owned);
        let view = runtime.view();
        for id in ["a", "b"] {
            let primary = view.rows.iter().find(|row| row.id == id).unwrap();
            assert_eq!(primary.title, format!("Portal {id}"));
            let explicit = view.rows.iter().find(|row| row.id.starts_with("action/profile.edit/")
                && matches!(&row.action, OttAction::EditProfile { target, .. } if target == id)).unwrap();
            if active == Some(id) {
                assert_eq!(primary.action, explicit.action);
                assert_eq!(primary.enabled, explicit.enabled);
            } else {
                assert_eq!(primary.action, OttAction::SelectProfile(id.into()));
            }
        }
    }
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.active_profile = Some("a".into());
    runtime.loaded = false;
    let view = runtime.view();
    assert!(!view.rows.iter().find(|row| row.id == "a").unwrap().enabled,
        "the primary Edit offer must inherit the existing editor's write admission");
}

#[test]
fn active_stalker_primary_ok_opens_setup_without_save_or_catalog_restart() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(0);
    let runtime = app.ott.as_ref().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let generation = runtime.generation;
    let identity = route::playback_identity(&app.player.session);
    assert_eq!(runtime.view().selected_row.as_deref(), Some("a"));
    assert!(matches!(&runtime.view().rows.iter().find(|row| row.id == "a").unwrap().action,
        OttAction::EditProfile { target, .. } if target == "a"));
    press(&mut app, Key::Ok);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "profile-setup");
    assert_eq!(runtime.setup.as_ref().unwrap().original.as_ref(), Some(&portal("a")));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.generation, generation);
    assert!(!runtime.saving);
    assert!(runtime.pending_setup_save.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn modern_stalker_primary_row_keeps_load_and_explicit_edit() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let app = mounted(1);
    let view = app.ott.as_ref().unwrap().view();
    for id in ["a", "b"] {
        assert_eq!(view.rows.iter().find(|row| row.id == id).unwrap().action,
            OttAction::SelectProfile(id.into()));
        assert!(view.rows.iter().any(|row| row.id.starts_with("action/profile.edit/")
            && matches!(&row.action, OttAction::EditProfile { target, .. } if target == id)));
    }
}

#[test]
fn active_xtream_primary_row_keeps_its_existing_load_action() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(0);
    let runtime = app.ott.as_mut().unwrap();
    for profile in runtime.saved.profiles.profiles.iter_mut().filter(|profile| profile.kind == ProviderKind::Stalker) {
        profile.kind = ProviderKind::Xtream;
        profile.source = json!({"server":"https://xtream.invalid", "username":"fixture", "password":"fixture"}).to_string();
        profile.validate().unwrap();
    }
    runtime.provider = ProviderKind::Xtream;
    let view = runtime.view();
    for id in ["a", "b"] {
        assert_eq!(view.rows.iter().find(|row| row.id == id).unwrap().action,
            OttAction::SelectProfile(id.into()));
    }
}

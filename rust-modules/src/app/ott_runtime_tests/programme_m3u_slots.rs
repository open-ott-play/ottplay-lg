//! Classic numbered slots use real profile actions and the existing Saved writer.
use super::*;

fn slots_app(mode: u8) -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([3; 16]);
    runtime.write_permit.revoke();
    runtime.saved.profiles.profiles = vec![source_profile(ProviderKind::M3u, "a"), source_profile(ProviderKind::M3u, "b")];
    runtime.saved.profiles.active_profile = Some("a".into());
    runtime.saved.settings.insert("interfaceMode".into(), json!(mode));
    runtime.provider = ProviderKind::M3u;
    runtime.page = "profiles";
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    app.bridge.apply_ui_mode(if mode == 0 { plx_machine::machine::UiMode::Classic } else { plx_machine::machine::UiMode::Modern });
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
}

fn numbered(app: &App, number: usize) -> plx_screens::ott::OttRow {
    let prefix = format!("{number}: ");
    app.ott.as_ref().unwrap().view().rows.into_iter().find(|row| row.title.starts_with(&prefix))
        .expect("all fifteen Classic slot labels survive UI localization")
}

#[test]
fn classic_m3u_shows_all_numbered_slots_and_preserves_explicit_actions() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles[0].name = "Empty".into();
    runtime.saved.profiles.profiles[1].name = "Load playlist".into();
    runtime.saved.profiles.profiles[1].source = "https://media.invalid/list.m3u?token=fixture-secret".into();
    let mut portal = source_profile(ProviderKind::M3u, "portal");
    portal.source = "portal::[key:fixture-secret]https://portal.invalid/api/".into();
    runtime.saved.profiles.profiles.push(portal);
    for number in 1..=15 { assert!(numbered(&app, number).enabled); }
    assert_eq!(numbered(&app, 1).title, "1: Empty");
    assert_eq!(numbered(&app, 2).title, "2: Load playlist");
    assert!(matches!(numbered(&app, 1).action, OttAction::EditProfile { target, .. } if target == "a"));
    assert!(matches!(numbered(&app, 2).action, OttAction::SelectProfile(id) if id == "b"));
    assert!(matches!(numbered(&app, 3).action, OttAction::SelectProfile(id) if id == "portal"));
    assert!(matches!(numbered(&app, 15).action, OttAction::EditProfile { .. }));
    let view = app.ott.as_ref().unwrap().view();
    assert!(view.rows.iter().any(|row| row.id.starts_with("action/profile.load/")
        && matches!(&row.action, OttAction::SelectProfile(id) if id == "a")));
    assert!(view.rows.iter().any(|row| matches!(&row.action, OttAction::EditProfile { target, .. } if target == "b")));
    for row in &view.rows {
        assert!(!format!("{} {} {}", row.title, row.detail, row.description).contains("fixture-secret"));
    }
}

#[test]
fn modern_profiles_do_not_gain_virtual_slots() {
    let _serial = plx_base::testlock::serial();
    let app = slots_app(1);
    let view = app.ott.as_ref().unwrap().view();
    assert!(view.rows.iter().any(|row| row.id == "a" && row.title == "Saved a"
        && matches!(&row.action, OttAction::SelectProfile(id) if id == "a")));
    assert!(view.rows.iter().any(|row| row.id == "action/profile.add"));
    assert!(!view.rows.iter().any(|row| row.id.starts_with("m3u-slot/")));
}

#[test]
fn empty_slot_cancel_never_persists_a_placeholder() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let offer = numbered(&app, 9).action;
    action(&mut app, offer);
    let runtime = app.ott.as_ref().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    assert_eq!(editor.key, "profile.source");
    assert!(runtime.edit_profile_original.is_none());
    assert!(runtime.edit_profile.as_ref().unwrap().source.is_empty());
    assert!(!runtime.saving);
    action(&mut app, OttAction::CancelSetting { session: editor.id });
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.is_none());
    assert_eq!(runtime.view().selected_row.as_deref(), Some("m3u-slot/9"));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn empty_slot_save_assigns_the_requested_number_without_selecting_or_loading() {
    let _serial = plx_base::testlock::serial();
    for (slot, active) in [(7, Some("a")), (15, None)] {
        let mut app = slots_app(0);
        app.ott.as_mut().unwrap().saved.profiles.active_profile = active.map(str::to_owned);
        let offer = numbered(&app, slot).action;
        action(&mut app, offer);
        let runtime = app.ott.as_mut().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        let mut profile = runtime.edit_profile.clone().unwrap();
        profile.source = "https://new.invalid/private.m3u".into();
        let id = profile.id.clone();
        let before = runtime.saved.profiles.profiles.clone();
        let generation = runtime.generation;
        let (candidate, reload) = runtime.profile_editor_candidate(&editor, profile.clone(), None).unwrap();
        assert!(!reload);
        assert_eq!(candidate.settings[&format!("slot:{id}")], json!(slot));
        assert_eq!(candidate.profiles.active_profile.as_deref(), active);
        assert!(checked(&mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key, value: profile.source,
        }).is_ok());
        let runtime = app.ott.as_ref().unwrap();
        assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(), Job::Saved(Err(_), None, false, _)));
        runtime.tx.send(Job::Saved(Ok(candidate), None, false, json!({}))).ok().unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.saved.settings[&format!("slot:{id}")], json!(slot));
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), active);
        assert_eq!(&runtime.saved.profiles.profiles[..before.len()], before.as_slice());
        assert_eq!(runtime.generation, generation);
        assert!(runtime.catalog_next.is_none());
        assert!(!runtime.catalog_loading);
        assert_eq!(numbered(&app, slot).id, id);
        assert_eq!(runtime.view().selected_row.as_deref(), Some(id.as_str()));
    }
}

#[test]
fn slot_only_remapping_rejects_old_offers_and_drafts_without_overwriting_profiles() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let stale = numbered(&app, 8).action;
    app.ott.as_mut().unwrap().saved.settings.insert("slot:b".into(), json!(8));
    assert!(checked(&mut app, stale).is_err());
    let offer = numbered(&app, 9).action;
    action(&mut app, offer);
    let runtime = app.ott.as_mut().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    runtime.saved.settings.insert("slot:b".into(), json!(9));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(checked(&mut app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key, value: "https://stale.invalid/list.m3u".into(),
    }).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn duplicate_disabled_and_out_of_range_claims_preserve_every_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = ["a", "b", "c", "d", "e", "f"].map(|id| source_profile(ProviderKind::M3u, id)).to_vec();
    runtime.saved.profiles.profiles[0].enabled = false;
    for (id, value) in [("a", json!(1)), ("b", json!(1)), ("c", json!(0)), ("d", json!(16)), ("e", json!("bad")), ("f", json!(-1))] {
        runtime.saved.settings.insert(format!("slot:{id}"), value);
    }
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(!numbered(&app, 1).enabled);
    assert!(matches!(numbered(&app, 2).action, OttAction::EditProfile { .. }));
    assert_eq!(numbered(&app, 5).id, "e");
    assert_eq!(numbered(&app, 6).id, "f");
    let view = app.ott.as_ref().unwrap().view();
    for id in ["a", "b", "c", "d", "e", "f"] { assert!(view.rows.iter().any(|row| row.id == id)); }
    assert!(!view.rows.iter().find(|row| row.id == "a").unwrap().enabled);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
}

#[test]
fn slot_metadata_aba_retires_offer_and_empty_draft() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let stale = numbered(&app, 12).action;
    action(&mut app, stale.clone());
    let runtime = app.ott.as_mut().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    let before = runtime.saved.clone();
    let mut occupied = before.clone();
    occupied.settings.insert("slot:b".into(), json!(12));
    runtime.observe_profile_edits(&occupied);
    runtime.saved = occupied;
    runtime.observe_profile_edits(&before);
    runtime.saved = before;
    assert!(checked(&mut app, stale).is_err());
    assert!(checked(&mut app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key, value: "https://stale.invalid/list.m3u".into(),
    }).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn empty_slot_preserves_existing_settings_write_admission_and_capacity() {
    let _serial = plx_base::testlock::serial();
    let mut app = slots_app(0);
    let offer = numbered(&app, 12).action;
    app.ott.as_mut().unwrap().loaded = false;
    assert!(!numbered(&app, 12).enabled);
    assert!(checked(&mut app, offer).is_err());
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saved.profiles.profiles = (1..=15).map(|n| source_profile(ProviderKind::M3u, &format!("p{n}"))).collect();
    for n in 1..=15 { runtime.saved.settings.insert(format!("slot:p{n}"), json!(n + 15)); }
    let offer = numbered(&app, 12);
    assert!(!offer.enabled, "unaddressable extras still count against the existing capacity");
    assert!(checked(&mut app, offer.action).is_err());
    let runtime = app.ott.as_ref().unwrap();
    for n in 1..=15 { assert!(runtime.view().rows.iter().any(|row| row.id == format!("p{n}"))); }
    assert_eq!(runtime.saved.profiles.profiles.len(), 15);
}

//! Profile-capacity admission uses the real view/action path, without a writer.
use super::*;
use plx_platform::i18n::{self, msg};

fn profile(kind: ProviderKind, slot: usize) -> ProviderProfile {
    ProviderProfile {
        id: format!("{}-{slot}", kind.canonical_id().replace('/', "_")),
        name: format!("Fixture {slot}"),
        kind,
        source: match kind {
            ProviderKind::M3u => "https://fixture.invalid/list.m3u",
            ProviderKind::Xtream => {
                r#"{"server":"https://fixture.invalid","username":"fixture","password":"fixture"}"#
            }
            ProviderKind::Stalker => {
                r#"{"portal":"https://fixture.invalid","mac":"00:1A:79:00:00:01"}"#
            }
            ProviderKind::VPortal => "portal::[key:fixture]https://fixture.invalid/portal",
            ProviderKind::Local => "/media/videos",
            ProviderKind::Edem | ProviderKind::Operator(_) => r#"{"key":"fixture"}"#.into(),
            ProviderKind::Plex => "fixture-server",
        }
        .into(),
        source_alias: String::new(),
        enabled: true,
    }
}

fn full_runtime(kind: ProviderKind) -> Box<Runtime> {
    // Use the normal decoded-state boundary so settings identities are ready,
    // then observe the same synthetic profile image without loading a source.
    let mut app = Box::new(ready_app_fixture());
    let mut runtime = app.ott.take().unwrap();
    // These cases exercise the structured profile editor, which is distinct
    // from Classic M3U's fixed slot list.
    runtime.saved.settings.insert("interfaceMode".into(), serde_json::json!(1));
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern);
    runtime.provider = kind;
    runtime.page = "profiles";
    runtime.saved.profiles.profiles = (1..=15).map(|slot| profile(kind, slot)).collect();
    runtime.saved.profiles.active_profile = Some(profile(kind, 1).id);
    runtime.sync_saved_context();
    runtime
}

fn open_settings_group(runtime: &mut Runtime, id: &str) {
    let key = format!("settings.open:{id}");
    let row = runtime.view().rows.into_iter().find(|row| row.id == key)
        .expect("group must be present in the actual Settings view");
    assert!(row.enabled);
    assert_eq!(row.action, OttAction::EditSetting(key));
    settings_groups::action(runtime, &row.action).expect("owned group action").unwrap();
}

#[test]
fn full_provider_disables_only_add_and_leaves_existing_profile_actions_available() {
    let _serial = plx_base::testlock::serial();
    for kind in [
        ProviderKind::M3u,
        ProviderKind::Xtream,
        ProviderKind::Stalker,
        ProviderKind::VPortal,
        ProviderKind::Local,
    ] {
        let mut runtime = full_runtime(kind);
        let view = runtime.view();
        let add = view
            .rows
            .iter()
            .find(|row| row.id == "action/profile.add")
            .unwrap();
        assert!(!add.enabled);
        assert_eq!(add.detail, msg::ott_profile_provider_limit_in(i18n::current()));
        assert!(view
            .rows
            .iter()
            .filter(|row| row.id != "action/profile.add")
            .all(|row| row.enabled));
        runtime.page = "settings";
        assert!(runtime.view().rows.iter().find(|row| row.id == "native.settings").unwrap().enabled);
        open_settings_group(&mut runtime, "profiles");
        let view = runtime.view();
        assert!(
            !view
                .rows
                .iter()
                .find(|row| row.id == "profile.add")
                .unwrap()
                .enabled
        );
        for id in ["profile.source", "profile.name"] {
            assert!(
                view.rows.iter().find(|row| row.id == id).unwrap().enabled,
                "{id}"
            );
        }
        settings_groups::action(&mut runtime, &OttAction::ShowSettings).unwrap().unwrap();
        open_settings_group(&mut runtime, "remote");
        assert!(runtime.view().rows.iter().find(|row| row.id == "remote.address").unwrap().enabled);
    }
}

#[test]
fn full_provider_refuses_scalar_and_structured_add_before_allocating_any_draft() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    for kind in [
        ProviderKind::M3u,
        ProviderKind::Xtream,
        ProviderKind::Stalker,
        ProviderKind::VPortal,
        ProviderKind::Local,
        ProviderKind::Plex,
    ] {
        let mut runtime = full_runtime(kind);
        let before = runtime.saved.clone();
        let serials = (runtime.edit_serial, runtime.setup_serial);
        assert_eq!(
            handle_action(
                &mut runtime,
                &mut app,
                OttAction::EditSetting("profile.add".into())
            ),
            Err("This provider already has 15 profiles")
        );
        assert_eq!((runtime.edit_serial, runtime.setup_serial), serials);
        assert!(
            runtime.edit_profile.is_none()
                && runtime.setup.is_none()
                && runtime.edit_session.is_none()
        );
        assert!(!runtime.saving);
        assert_eq!(
            serde_json::to_value(&runtime.saved).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        assert_eq!(runtime.page, "profiles");
    }
}

#[test]
fn full_provider_still_opens_existing_scalar_and_structured_profiles() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    for kind in [
        ProviderKind::M3u,
        ProviderKind::Xtream,
        ProviderKind::Stalker,
    ] {
        let mut runtime = full_runtime(kind);
        let before = runtime.saved.clone();
        let existing = profile(kind, 1);
        handle_action(
            &mut runtime,
            &mut app,
            OttAction::EditSetting("profile.source".into()),
        )
        .unwrap();
        if kind == ProviderKind::M3u {
            assert_eq!(runtime.edit_profile.as_ref(), Some(&existing));
        } else {
            assert_eq!(
                runtime.setup.as_ref().unwrap().original.as_ref(),
                Some(&existing)
            );
            assert!(
                runtime.setup_candidate().is_ok(),
                "editing needs no extra slot"
            );
        }
        assert!(!runtime.saving);
        assert_eq!(
            serde_json::to_value(&runtime.saved).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }
}

#[test]
fn releasing_one_slot_reenables_add_for_both_editor_types() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    for kind in [
        ProviderKind::M3u,
        ProviderKind::Xtream,
        ProviderKind::Stalker,
    ] {
        let mut runtime = full_runtime(kind);
        runtime.saved.profiles.profiles.pop();
        assert!(
            runtime
                .view()
                .rows
                .iter()
                .find(|row| row.id == "action/profile.add")
                .unwrap()
                .enabled
        );
        handle_action(
            &mut runtime,
            &mut app,
            OttAction::EditSetting("profile.add".into()),
        )
        .unwrap();
        if kind == ProviderKind::M3u {
            assert_eq!(
                runtime.edit_profile.as_ref().unwrap().id,
                profile(kind, 15).id
            );
        } else {
            let setup = runtime.setup.as_ref().unwrap();
            assert!(setup.original.is_none());
            assert_eq!(setup.draft.profile_id(), profile(kind, 15).id);
        }
        assert!(!runtime.saving);
        assert_eq!(runtime.saved.profiles.profiles.len(), 14);
    }
}

#[test]
fn total_capacity_is_reported_and_plex_connection_row_is_not_repurposed() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = full_runtime(ProviderKind::Plex);
    for kind in ProviderKind::all().into_iter().filter(|kind|*kind!=ProviderKind::Plex) {
        runtime
            .saved
            .profiles
            .profiles
            .extend((1..=15).map(|slot| profile(kind, slot)));
    }
    assert_eq!(
        runtime.profile_add_refusal(),
        Some("This installation has reached its profile limit")
    );
    assert!(
        runtime
            .view()
            .rows
            .iter()
            .find(|row| row.id == "action/plex.connect")
            .unwrap()
            .enabled
    );
    runtime.page = "settings";
    open_settings_group(&mut runtime, "profiles");
    let view = runtime.view();
    let add = view
        .rows
        .iter()
        .find(|row| row.id == "profile.add")
        .unwrap();
    assert!(!add.enabled);
    assert_eq!(add.detail, msg::ott_profile_total_limit_in(i18n::current()));
}

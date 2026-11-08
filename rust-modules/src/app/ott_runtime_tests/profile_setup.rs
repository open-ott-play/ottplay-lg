//! Profile onboarding crosses the real OTT screen, dispatcher and runner.
//! No protected-store, network, decoder or device operation is started.
use super::*;
use plx_machine::machine::Key;
use plx_screens::ott::{OttAction, OttOut};

fn mount_setup(app: &mut App) {
    app.ott.as_mut().unwrap().loaded = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    runtime_tick(app);
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
}

fn setup_key(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(
        &mut app.pages,
        &mut app.bridge,
        tick,
        bridge::script_key(key, tick),
    );
    runtime_tick(app);
    settle_pages(app);
}

fn setup_action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("OTT mounted");
    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn add_profile_draft_uses_first_unused_id_without_overwriting_saved_profiles() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    for (ids, expected) in [
        (vec!["m3u-2"], "m3u-1"),
        (vec!["m3u-1", "m3u-3"], "m3u-2"),
        (vec!["m3u-1", "m3u-2"], "m3u-3"),
    ] {
        let mut runtime = runtime_fixture();
        runtime.loaded = true;
        runtime.saved.profiles.profiles = ids
            .iter()
            .map(|id| ProviderProfile {
                id: (*id).into(),
                name: (*id).into(),
                kind: ProviderKind::M3u,
                source: "https://fixture.invalid/list.m3u".into(),
                source_alias: String::new(),
                enabled: true,
            })
            .collect();
        runtime.saved.profiles.active_profile = Some(ids[0].into());
        let before = serde_json::to_value(&runtime.saved).unwrap();
        handle_action(
            &mut runtime,
            &mut app,
            OttAction::EditSetting("profile.add".into()),
        )
        .unwrap();
        let draft = runtime.edit_profile.as_ref().expect("new source draft");
        assert_eq!(draft.id, expected);
        assert_eq!(draft.kind, ProviderKind::M3u);
        assert!(draft.source.is_empty());
        assert!(runtime
            .saved
            .profiles
            .profiles
            .iter()
            .all(|profile| profile.id != draft.id));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving, "opening the draft never submits a save");
    }
}

#[test]
fn providers_first_ok_opens_current_provider_profiles() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount_setup(&mut app);
    setup_key(&mut app, Key::Ok);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_eq!(
        runtime.page, "profiles",
        "initial focus must select M3U, not Providers navigation"
    );
}

#[test]
fn empty_m3u_profiles_first_ok_opens_source_editor_without_bouncing() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount_setup(&mut app);
    setup_action(&mut app, OttAction::SelectProvider("m3u".into()));
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    setup_key(&mut app, Key::Ok);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.page, "profiles",
        "first OK must not activate nav:providers"
    );
    let draft = runtime
        .edit_profile
        .as_ref()
        .expect("Add profile must open its source editor");
    assert_eq!(draft.kind, ProviderKind::M3u);
    assert!(draft.source.is_empty());
    assert!(
        runtime.saved.profiles.profiles.is_empty(),
        "opening a draft is not a save"
    );
}

#[test]
fn profile_page_and_editing_use_chosen_provider_instead_of_playing_provider() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let playing = ProviderProfile {
        id: "m3u-playing".into(),
        name: "TV".into(),
        kind: ProviderKind::M3u,
        source: "https://fixture.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let selected = ProviderProfile {
        id: "stalker-selected".into(),
        name: "Portal".into(),
        kind: ProviderKind::Stalker,
        source: json!({"portal":"https://fixture.invalid/c/", "mac":"02:00:00:00:00:01"})
            .to_string(),
        source_alias: String::new(),
        enabled: true,
    };
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![playing.clone(), selected.clone()];
        runtime.saved.profiles.active_profile = Some(playing.id.clone());
    }
    mount_setup(&mut app);
    setup_action(&mut app, OttAction::SelectProvider("stalker".into()));
    for key in ["profile.name", "profile.source"] {
        setup_action(&mut app, OttAction::EditSetting(key.into()));
        let runtime = app.ott.as_ref().unwrap();
        if key == "profile.source" {
            let setup = runtime.setup.as_ref().expect("structured source editor");
            assert_eq!(setup.draft.profile_id(), selected.id);
            assert_eq!(setup.draft.kind(), selected.kind);
            assert_eq!(setup.original.as_ref(), Some(&selected));
        } else {
            assert_eq!(runtime.edit_profile.as_ref(), Some(&selected),
                "editing the name must target the browsed provider");
        }
        assert_eq!(runtime.active(), Some(&playing),
            "opening an editor must not change the playing profile");
        setup_key(&mut app, Key::Back);
    }
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.view();
    assert_eq!(view.page_id, "profiles:stalker");
    assert_eq!(view.selected_row.as_deref(), Some(selected.id.as_str()));
    assert_eq!(
        runtime.saved.profiles.active_profile.as_deref(),
        Some(playing.id.as_str())
    );
    assert_eq!(runtime.saved.profiles.profiles, vec![playing, selected]);
}
#[test]
fn saved_source_catalog_play_back_and_restart_keep_the_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount_setup(&mut app);
    setup_action(&mut app, OttAction::SelectProvider("m3u".into()));
    setup_key(&mut app, Key::Ok);
    let mut profile = app
        .ott
        .as_ref()
        .unwrap()
        .edit_profile
        .clone()
        .expect("source draft opened");
    profile.name = "Fixture TV".into();
    profile.source = "https://fixture.invalid/list.m3u".into();
    profile.validate().unwrap();
    // Close the editor fixture before injecting the already-admitted durable result.
    // The save/ack gesture itself is covered by the editor protocol regressions.
    setup_key(&mut app, Key::Back);
    let saved = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saving = true;
        runtime.save_playback_epoch = runtime.playback_epoch;
        runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
        // Explicit result injection exercises the real durable completion path.
        // Keep all IO workers closed; no save/catalogue/playback worker is run.
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        let mut saved = runtime.saved.clone();
        saved.profiles.profiles.push(profile.clone());
        saved.profiles.active_profile = Some(profile.id.clone());
        saved
    };
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(saved.clone()),
            None,
            true,
            json!({"status":"ok"}),
        ))
        .unwrap_or_else(|_| panic!("saved-result receiver missing"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert_eq!(app.ott.as_ref().unwrap().active(), Some(&profile));
    let item = CatalogEntry {
        synopsis: None,
        id: "fixture-channel".into(),
        title: "Fixture channel".into(),
        category: "Fixture".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: "https://fixture.invalid/video.m3u8".into(),
        },
        duration_ms: None,
        provider_id: Some("fixture-1".into()),
        provider_id_field: None,
        live: None,
    };
    let runtime = app.ott.as_ref().unwrap();
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog { hierarchy: None,
                entries: vec![item.clone()],
            },
        ))
        .unwrap_or_else(|_| panic!("catalogue-result receiver missing"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    setup_action(&mut app, OttAction::Play(item.id.clone()));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|target| target.id.as_str()),
        Some(item.id.as_str())
    );
    setup_key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    setup_key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert_eq!(
        app.ott.as_ref().unwrap().saved.profiles.profiles,
        vec![profile.clone()]
    );

    let mut restarted = app_fixture();
    mount_setup(&mut restarted);
    {
        let runtime = restarted.ott.as_mut().unwrap();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime
            .tx
            .send(Job::Loaded(Ok((saved, [0; 16]))))
            .unwrap_or_else(|_| panic!("loaded-result receiver missing"));
    }
    runtime_tick(&mut restarted);
    settle_pages(&mut restarted);
    let runtime = restarted.ott.as_ref().unwrap();
    assert!(runtime.loaded);
    assert_eq!(runtime.active(), Some(&profile));
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_eq!(runtime.page, "channels");
    assert!(runtime.catalog_loading);
}

//! Setup crosses the real OTT dispatcher and scalar editor. Storage outcomes
//! are injected; no provider, protected-state or decoder worker is started.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};

fn mount(app: &mut App) {
    app.ott.as_mut().unwrap().loaded = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    runtime_tick(app);
    settle_pages(app);
}
fn action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}
fn press(app: &mut App, key: Key) {
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
fn text(app: &mut App, value: &str) {
    let tick = next_tick(app);
    let input = [TextEdit::Clear, TextEdit::Commit(value.into())]
        .into_iter()
        .map(|edit| InputEvent {
            at: tick,
            source: Source::Script,
            kind: InputKind::Text(edit),
        })
        .collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    runtime_tick(app);
    settle_pages(app);
}
fn visible(app: &App) -> bool {
    app.pages
        .nav
        .top_page()
        .unwrap()
        .inst
        .as_ref()
        .unwrap()
        .screen
        .as_any()
        .unwrap()
        .downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap()
        .handles_private_text()
}
fn open(app: &mut App, provider: &str) {
    action(app, OttAction::SelectProvider(provider.into()));
    action(app, OttAction::EditSetting("profile.add".into()));
    assert_eq!(app.ott.as_ref().unwrap().page, "profile-setup");
}
fn field(app: &mut App, key: &str, value: &str) {
    action(app, OttAction::EditSetting(key.into()));
    assert!(visible(app));
    text(app, value);
    press(app, Key::Down);
    press(app, Key::Ok);
    assert!(
        !visible(app),
        "successful field staging returns to the overview"
    );
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.is_none());
    assert!(!runtime.saving);
    assert_eq!(runtime.setup.as_ref().unwrap().draft.value(key), Ok(value));
}
fn fill(app: &mut App, provider: &str) {
    open(app, provider);
    field(app, "profile.setup.name", "Fixture account");
    if provider == "xtream" {
        field(app, "profile.setup.server", "https://fixture.invalid/");
        field(app, "profile.setup.username", "fixture-user");
        field(app, "profile.setup.password", " secret fixture password ");
    } else {
        field(app, "profile.setup.portal", "https://fixture.invalid/c/");
        field(app, "profile.setup.mac", "00:1a:79:00:00:01");
    }
}
fn admitted(app: &mut App) -> Saved {
    let runtime = app.ott.as_mut().unwrap();
    let (id, candidate) = runtime.setup_candidate().unwrap();
    runtime.pending_setup_save = Some(id);
    runtime.saving = true;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    candidate
}
fn job(app: &mut App, result: Job) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(result)
        .unwrap_or_else(|_| panic!("fixture receiver missing"));
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn structured_fields_use_real_scalar_save_and_keep_secrets_out_of_overview() {
    let _serial = plx_base::testlock::serial();
    for provider in ["xtream", "stalker"] {
        let mut app = app_fixture();
        mount(&mut app);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        fill(&mut app, provider);
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(runtime.view().page_id, format!("profile-setup:{provider}"));
        let overview = runtime
            .view()
            .rows
            .iter()
            .map(|row| format!("{} {} {}", row.title, row.detail, row.description))
            .collect::<Vec<_>>()
            .join(" ");
        for private in [
            "fixture.invalid",
            "fixture-user",
            "secret fixture password",
            "00:1a:79",
        ] {
            assert!(!overview.contains(private));
        }
        let (_, candidate) = runtime.setup_candidate().unwrap();
        assert_eq!(candidate.profiles.profiles.len(), 1);
        assert_eq!(
            candidate.profiles.active_profile.as_ref(),
            Some(&candidate.profiles.profiles[0].id)
        );
    }
}

#[test]
fn structured_first_field_arrow_navigation_and_back_keep_staged_value() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount(&mut app);
    open(&mut app, "xtream");
    press(&mut app, Key::Down);
    press(&mut app, Key::Up);
    press(&mut app, Key::Ok);
    assert_eq!(
        app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().key,
        "profile.setup.name"
    );
    text(&mut app, "Discarded name");
    press(&mut app, Key::Back);
    assert!(!visible(&app));
    assert_ne!(
        app.ott
            .as_ref()
            .unwrap()
            .setup
            .as_ref()
            .unwrap()
            .draft
            .value("profile.setup.name"),
        Ok("Discarded name")
    );
    assert_eq!(app.ott.as_ref().unwrap().page, "profile-setup");
    press(&mut app, Key::Back);
    assert!(app.ott.as_ref().unwrap().setup.is_none());
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    assert!(app.ott.as_ref().unwrap().saved.profiles.profiles.is_empty());
}

#[test]
fn structured_invalid_or_unready_save_retains_every_field_for_correction() {
    let _serial = plx_base::testlock::serial();
    for provider in ["xtream", "stalker"] {
        let mut app = app_fixture();
        mount(&mut app);
        open(&mut app, provider);
        let id = app.ott.as_ref().unwrap().setup.as_ref().unwrap().id;
        field(&mut app, "profile.setup.name", "Retained name");
        action(
            &mut app,
            OttAction::EditSetting("profile.setup.save".into()),
        );
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.setup.as_ref().unwrap().id, id);
        assert!(!runtime.status.is_empty());
        assert!(!runtime.saving);
        assert!(runtime.saved.profiles.profiles.is_empty());
        if provider == "xtream" {
            field(&mut app, "profile.setup.server", "https://fixture.invalid/");
            field(&mut app, "profile.setup.username", "user");
            field(&mut app, "profile.setup.password", "password");
        } else {
            field(
                &mut app,
                "profile.setup.portal",
                "https://fixture.invalid/c/",
            );
            field(&mut app, "profile.setup.mac", "00:1a:79:00:00:01");
        }
        assert!(app.ott.as_mut().unwrap().setup_candidate().is_ok());
        app.ott.as_mut().unwrap().loaded = false;
        action(
            &mut app,
            OttAction::EditSetting("profile.setup.save".into()),
        );
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.setup.as_ref().unwrap().id, id);
        assert_eq!(
            runtime
                .setup
                .as_ref()
                .unwrap()
                .draft
                .value("profile.setup.name"),
            Ok("Retained name")
        );
        assert!(runtime.pending_setup_save.is_none());
        assert!(!runtime.saving);
    }
}

#[test]
fn structured_cancel_rejects_old_field_submit_after_reopening_same_provider() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount(&mut app);
    open(&mut app, "xtream");
    action(
        &mut app,
        OttAction::EditSetting("profile.setup.password".into()),
    );
    let old = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    press(&mut app, Key::Back);
    action(
        &mut app,
        OttAction::EditSetting("profile.setup.cancel".into()),
    );
    open(&mut app, "xtream");
    action(
        &mut app,
        OttAction::EditSetting("profile.setup.password".into()),
    );
    let current = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    assert_ne!(old.id, current.id);
    action(
        &mut app,
        OttAction::SubmitSetting {
            session: old.id,
            key: old.key,
            value: "stale secret".into(),
        },
    );
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.as_ref() == Some(&current));
    assert_eq!(
        runtime
            .setup
            .as_ref()
            .unwrap()
            .draft
            .value("profile.setup.password"),
        Ok("")
    );
    assert!(runtime.saved.profiles.profiles.is_empty());
}

#[test]
fn structured_completion_retains_failed_draft_and_retires_only_verified_current_save() {
    let _serial = plx_base::testlock::serial();
    for outcome in ["success", "failed", "uncertain"] {
        let mut app = app_fixture();
        mount(&mut app);
        fill(&mut app, "xtream");
        let id = app.ott.as_ref().unwrap().setup.as_ref().unwrap().id;
        let candidate = admitted(&mut app);
        action(
            &mut app,
            OttAction::EditSetting("profile.setup.save".into()),
        );
        assert_eq!(
            app.ott.as_ref().unwrap().pending_setup_save,
            Some(id),
            "duplicate Save keeps the one admitted request"
        );
        let result = match outcome {
            "success" => Job::Saved(Ok(candidate), None, true, json!({"status":"ok"})),
            "failed" => Job::Saved(Err("Fixture write refused"), None, true, json!({})),
            _ => Job::Uncertain(None, None),
        };
        job(&mut app, result);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.pending_setup_save.is_none());
        assert_eq!(runtime.setup.is_none(), outcome == "success");
        assert_eq!(runtime.persistence_uncertain, outcome == "uncertain");
        if outcome == "success" {
            assert_eq!(runtime.saved.profiles.profiles.len(), 1);
        } else {
            assert_eq!(runtime.setup.as_ref().unwrap().id, id);
            assert_eq!(
                runtime
                    .setup
                    .as_ref()
                    .unwrap()
                    .draft
                    .value("profile.setup.password"),
                Ok(" secret fixture password ")
            );
            assert!(runtime.saved.profiles.profiles.is_empty());
        }
    }
}

#[test]
fn structured_old_completion_cannot_retire_replacement_draft() {
    let _serial = plx_base::testlock::serial();
    for (provider, kind) in [
        ("stalker", ProviderKind::Stalker),
        ("xtream", ProviderKind::Xtream),
    ] {
        let mut app = app_fixture();
        mount(&mut app);
        fill(&mut app, "xtream");
        let old = app.ott.as_ref().unwrap().setup.as_ref().unwrap().id;
        let candidate = admitted(&mut app);
        action(
            &mut app,
            OttAction::EditSetting("profile.setup.cancel".into()),
        );
        open(&mut app, provider);
        let current = app.ott.as_ref().unwrap().setup.as_ref().unwrap().id;
        assert_ne!(current, old);
        job(
            &mut app,
            Job::Saved(Ok(candidate), None, true, json!({"status":"ok"})),
        );
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(
            runtime.page, "profile-setup",
            "late saved result must respect the newer navigation intent"
        );
        assert_eq!(runtime.setup.as_ref().unwrap().id, current);
        assert_eq!(runtime.setup.as_ref().unwrap().draft.kind(), kind);
        assert!(
            runtime.setup.as_ref().unwrap().original.is_none(),
            "old result must not rebase a newer draft even with the same profile ID"
        );
        assert!(runtime.pending_setup_save.is_none());
    }
}

#[test]
fn structured_target_change_during_edit_cannot_overwrite_another_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount(&mut app);
    fill(&mut app, "xtream");
    let (_, candidate) = app.ott.as_mut().unwrap().setup_candidate().unwrap();
    app.ott.as_mut().unwrap().saved = candidate;
    action(&mut app, OttAction::EditSetting("profile.source".into()));
    field(&mut app, "profile.setup.username", "revised-user");
    assert!(app.ott.as_mut().unwrap().setup_candidate().is_ok());
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].name = "Concurrent edit".into();
    assert_eq!(
        app.ott.as_mut().unwrap().setup_candidate().err(),
        Some("This profile changed; reopen its setup before saving")
    );
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .setup
            .as_ref()
            .unwrap()
            .draft
            .value("profile.setup.username"),
        Ok("revised-user")
    );
}

#[test]
fn structured_refresh_preserves_draft_and_current_field_without_catalogue_reload() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount(&mut app);
    fill(&mut app, "xtream");
    let runtime = app.ott.as_ref().unwrap();
    let id = runtime.setup.as_ref().unwrap().id;
    let key = runtime.setup.as_ref().unwrap().selected_key.clone();
    let generation = runtime.generation;
    action(&mut app, OttAction::Refresh);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.setup.as_ref().unwrap().id, id);
    assert_eq!(runtime.setup.as_ref().unwrap().selected_key, key);
    assert_eq!(
        runtime
            .setup
            .as_ref()
            .unwrap()
            .draft
            .value("profile.setup.password"),
        Ok(" secret fixture password ")
    );
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.page, "profile-setup");
    assert!(!runtime.catalog_loading);
}

#[test]
fn structured_verified_write_with_apply_error_retains_correctable_own_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    mount(&mut app);
    fill(&mut app, "xtream");
    let candidate = admitted(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let id = runtime.setup.as_ref().unwrap().id;
    // The writer succeeded; applying settings to the running player failed.
    // Exercise the same completion boundary without inventing a storage failure.
    runtime.saved = candidate;
    runtime.saving = false;
    runtime.complete_setup(false, true);
    assert_eq!(runtime.setup.as_ref().unwrap().id, id);
    assert!(runtime.setup_candidate().is_ok());
    runtime
        .setup
        .as_mut()
        .unwrap()
        .draft
        .stage("profile.setup.username", "corrected-user")
        .unwrap();
    let (_, corrected) = runtime.setup_candidate().unwrap();
    assert_eq!(corrected.profiles.profiles.len(), 1);
    let source: serde_json::Value =
        serde_json::from_str(&corrected.profiles.profiles[0].source).unwrap();
    assert_eq!(source["username"], "corrected-user");
}

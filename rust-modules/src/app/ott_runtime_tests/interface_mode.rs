//! Real UI action admission and public preference settlement; no renderer or decoder is started.
use super::*;
use plx_machine::machine::{Host, UiMode};

struct TypographyReset(plx_machine::machine::FontScale);
impl Drop for TypographyReset {
    fn drop(&mut self) {
        plx_gfx::text::set_ui_typography(self.0);
    }
}
fn mounted() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.tx.send(Job::PublicLoaded(Ok(None)))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.page = "settings";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    app
}
fn act(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}
fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(
        MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn classic_choice(app: &App) -> OttAction {
    app.ott
        .as_ref()
        .unwrap()
        .view()
        .rows
        .into_iter()
        .find(|row| row.id == "interface:0")
        .unwrap()
        .action
}
fn open(app: &mut App) {
    act(app, OttAction::EditSetting("interfaceMode".into())).unwrap();
}

#[test]
fn mode_picker_cancel_preserves_selection_source_and_all_saved_data() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    let (_root_dir, mut app, root) = super::local_integration::setup();
    super::local_integration::install(&mut app, Some(&root), "first");
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = false;
    runtime.entropy = Some([1; 16]);
    runtime.public_preferences.accept(None);
    runtime.page = "settings";
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let selected = runtime.selected.clone();
    let epochs = (
        runtime.generation,
        runtime.playback_epoch,
        runtime.navigation_epoch,
    );
    let identity = plx_media::route::playback_identity(&app.player.session);
    let source = plx_media::route::external_source(&app.player.session);
    open(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().view().rows.len(), 3);
    act(&mut app, OttAction::Back).unwrap();
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "settings");
    assert_eq!(runtime.selected, selected);
    assert_eq!(
        (
            runtime.generation,
            runtime.playback_epoch,
            runtime.navigation_epoch
        ),
        epochs
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(matches!(
        runtime.transient.accepted,
        Some(transient::TransientMedia::Local(_))
    ));
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        source
    );
}

#[test]
fn rejected_mode_save_never_publishes_a_draft() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    for revoked_at_admission in [true, false] {
        let mut app = mounted();
        open(&mut app);
        if revoked_at_admission {
            let choice = classic_choice(&app);
            app.ott.as_ref().unwrap().write_permit.revoke();
            assert!(act(&mut app, choice).is_err(), "revoked authority is rejected before a writer starts");
            assert!(!app.ott.as_ref().unwrap().saving);
            assert!(matches!(app.ott.as_ref().unwrap().rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)));
        } else {
            // Model a separately admitted writer's definite failure through the
            // real completion handler, without starting storage IO in this fixture.
            app.ott.as_mut().unwrap().saving = true;
            app.ott.as_mut().unwrap().save_navigation = SaveNavigation::Preserve;
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.saving);
            assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
            assert_eq!(runtime.interface_mode(), UiMode::Modern);
            assert_eq!(app.bridge.ui_mode(), UiMode::Modern);
            let job = Job::PublicSaved(
                Err(public_preferences::WriteFailure::Rejected(private::Error::Revoked)),
                None, json!({"status":"ok"}));
            assert!(matches!(&job, Job::PublicSaved(Err(_), None, _)));
            runtime.tx.send(job)
                .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
            runtime_tick(&mut app);
        }
        assert!(!app.ott.as_ref().unwrap().saving);
        assert_eq!(app.ott.as_ref().unwrap().interface_mode(), UiMode::Modern);
        assert_eq!(app.bridge.ui_mode(), UiMode::Modern);
    }
}

#[test]
fn committed_mode_reaches_outer_and_nested_views_and_restores_on_restart() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = mounted();
        let runtime = app.ott.as_mut().unwrap();
        runtime.selected = Some("selected-media".into());
        runtime.saved.category = "Favorites".into();
        let provider = runtime.provider;
        let epochs = (
            runtime.generation,
            runtime.playback_epoch,
            runtime.navigation_epoch,
        );
        let snapshot = public_preferences::Snapshot {
            value: plx_platform::storage::public_preferences::Preferences {
                interface_mode: Some(mode.index() as u8), ..Default::default()
            }, revision: 1, epoch: 1,
        };
        runtime.saving = true;
        runtime.save_navigation = SaveNavigation::Preserve;
        runtime.save_playback_epoch = runtime.playback_epoch;
        runtime
            .tx
            .send(Job::PublicSaved(
                Ok(snapshot.clone()),
                None,
                json!({"status":"ok"}),
            ))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.interface_mode(), mode);
        assert_eq!(runtime.provider, provider);
        assert_eq!(runtime.selected.as_deref(), Some("selected-media"));
        assert_eq!(runtime.saved.category, "Favorites");
        assert_eq!(
            (
                runtime.generation,
                runtime.playback_epoch,
                runtime.navigation_epoch
            ),
            epochs
        );
        let views = app.bridge.views();
        assert_eq!(bridge::AppHost::ui_mode(&views), mode);
        drop(app);
        let mut reopened = mounted();
        let runtime = reopened.ott.as_mut().unwrap();
        runtime.loaded = false;
        runtime
            .tx
            .send(Job::PublicLoaded(Ok(Some(snapshot))))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(reopened.bridge.ui_mode(), mode);
        // A later protected-state load cannot replace the acknowledged public mode.
        reopened.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((Saved::default(), [1; 16]))))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(reopened.bridge.ui_mode(), mode);
    }
}

#[test]
fn stale_picker_choice_cannot_commit_after_navigation_or_preference_change() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    let mut app = mounted();
    open(&mut app);
    let stale = classic_choice(&app);
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    assert!(act(&mut app, stale).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
    let stale = classic_choice(&app);
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("settingRevision:interfaceMode".into(), json!("new"));
    assert!(act(&mut app, stale).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn mode_picker_actions_do_not_advance_navigation_through_the_real_dispatcher() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    let mut app = mounted();
    let before = app.ott.as_ref().unwrap().navigation_epoch;
    for action in [
        OttAction::EditSetting("interfaceMode".into()),
        OttAction::Back,
    ] {
        ui(&mut app, action);
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, before);
    }
    assert_eq!(app.ott.as_ref().unwrap().page, "settings");
    assert_eq!(
        app.ott.as_ref().unwrap().view().selected_row.as_deref(),
        Some("interfaceMode")
    );
    // A real OK after Back must re-enter the same row, not Add profile.
    let tick = next_tick(&mut app);
    bridge::frame(
        &mut app.pages,
        &mut app.bridge,
        tick,
        bridge::script_key(plx_machine::machine::Key::Ok, tick),
    );
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "interface-mode");
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, before);
}

#[test]
fn source_replacement_invalidates_a_picker_without_exposing_private_source() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    let mut app = mounted();
    let profile = synthetic_profile();
    let runtime = app.ott.as_mut().unwrap();
    runtime.provider = profile.kind;
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles.push(profile);
    open(&mut app);
    let stale = classic_choice(&app);
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].source =
        "https://replacement.invalid/private".into();
    assert!(act(&mut app, stale).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert!(!runtime.interface_mode_token().contains("replacement"));
}

#[test]
fn retired_picker_choice_cannot_commit_to_a_reopened_picker() {
    let _serial = plx_base::testlock::serial();
    let _typography = TypographyReset(plx_ui::typography::current());
    let mut app = mounted();
    ui(&mut app, OttAction::EditSetting("interfaceMode".into()));
    let retired_choice = classic_choice(&app);
    ui(&mut app, OttAction::Back);
    ui(&mut app, OttAction::EditSetting("interfaceMode".into()));
    assert_ne!(classic_choice(&app), retired_choice);
    ui(&mut app, retired_choice);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert_eq!(
        runtime.status,
        "Interface selection changed; select the mode again"
    );
    assert_eq!(runtime.interface_mode(), UiMode::Modern);
    assert_eq!(app.bridge.ui_mode(), UiMode::Modern);
}

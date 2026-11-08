//! Actual Classic action/modal path; no global locale or storage write.
use super::*;

fn mounted() -> App {
    let mut app = ready_app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.write_permit.revoke();
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

fn queue_ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT owner");
    app.pages.emit(MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
}

fn ui(app: &mut App, action: OttAction) {
    queue_ui(app, action);
    settle_pages(app);
}

fn command(app: &App, suffix: &str) -> OttAction {
    app.ott.as_ref().unwrap().view().rows.iter().find(|row|
        row.id.starts_with("settings.draft:") && row.id.ends_with(suffix))
        .expect("visible Classic command").action.clone()
}

fn open_interface(app: &mut App) {
    ui(app, OttAction::EditSetting("settings.open:interface".into()));
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
}

fn open_language(app: &mut App) -> plx_machine::machine::EntryId {
    let action = command(app, ":language");
    ui(app, action);
    runtime_tick(app);
    app.pages.nav.modals.surfaces.iter().find(|surface|
        matches!(surface.entry.arg, AppArg::Settings(plx_screens::family::SettingsPage::Language)))
        .expect("ordinary native Language surface").entry.id
}

fn dismiss(app: &mut App, id: plx_machine::machine::EntryId) {
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(id));
    settle_pages(app);
    runtime_tick(app);
}

#[test]
fn classic_language_modal_returns_to_same_group_and_preserves_its_unsaved_draft() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    open_interface(app);
    let action = command(app, ":pick:uiFontScale");
    ui(app, action);
    let action = command(app, ":choose:uiFontScale:150");
    ui(app, action);
    let owner = app.pages.top_page();
    let (generation, navigation) = {
        let runtime = app.ott.as_ref().unwrap();
        (runtime.generation, runtime.navigation_epoch)
    };
    let surface = open_language(app);
    assert!(app.pages.surface_up());
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
    // Exercise actual projection refresh, without changing a process-global locale.
    super::super::locale_changed(app);
    dismiss(app, surface);
    assert_eq!(app.pages.top_page(), owner);
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.view();
    assert_eq!(view.title, "Interface");
    assert!(view.rows.iter().any(|row| row.detail == "150%"));
    assert!(view.selected_row.as_deref().is_some_and(|id| id.ends_with(":language")));
    assert_eq!((runtime.generation, runtime.navigation_epoch), (generation, navigation));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn language_modal_does_not_preserve_a_group_after_its_source_changes() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    open_interface(app);
    let surface = open_language(app);
    app.ott.as_mut().unwrap().generation += 1;
    runtime_tick(app);
    dismiss(app, surface);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.view().rows.iter().any(|row| row.id == "settings.open:interface"));
    assert!(!runtime.view().rows.iter().any(|row| row.id.ends_with(":language")));
    assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn unrelated_modal_cannot_borrow_the_language_draft_handoff() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    open_interface(app);
    let _surface = open_language(app);
    bridge::open_account_menu(&mut app.pages);
    settle_pages(app);
    runtime_tick(app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.view().rows.iter().any(|row| row.id == "settings.open:interface"));
    assert!(!runtime.saving);
}

#[test]
fn queued_language_present_and_dismiss_preserve_the_owned_group() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    open_interface(app);
    let action = command(app, ":language");
    queue_ui(app, action);
    assert!(app.pages.has_pending_surface_navigation());
    assert!(!app.pages.has_pending_navigation(), "Present is not page navigation");
    runtime_tick(app);
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
    settle_pages(app);
    runtime_tick(app);
    let surface = app.pages.nav.modals.surfaces.iter().find(|surface|
        matches!(surface.entry.arg, AppArg::Settings(plx_screens::family::SettingsPage::Language)))
        .expect("Language mounted").entry.id;
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(surface));
    assert!(app.pages.has_pending_surface_navigation());
    assert!(!app.pages.has_pending_navigation(), "Dismiss is not page navigation");
    runtime_tick(app);
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
    settle_pages(app);
    runtime_tick(app);
    let view = app.ott.as_ref().unwrap().view();
    assert_eq!(view.title, "Interface");
    assert!(view.selected_row.as_deref().is_some_and(|id| id.ends_with(":language")));
}

#[test]
fn retained_language_draft_does_not_admit_a_covered_ott_command() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    open_interface(app);
    let owner = app.ott.as_ref().unwrap().reply.expect("actual mounted OTT owner");
    let surface = open_language(app);
    assert_eq!(current_ui_owner(app), None);
    assert_eq!(app.ott.as_ref().unwrap().reply, Some(owner), "only the draft is retained");
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let navigation = app.ott.as_ref().unwrap().navigation_epoch;
    // Queue an actual AppFx from the still-mounted but covered OTT body.
    // ShowProviders would navigate away and cancel the draft if reply retention
    // accidentally replaced the real-input-owner admission check.
    queue_ui(app, OttAction::ShowProviders);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.reply, Some(owner));
    assert_eq!(runtime.view().title, "Interface");
    assert_eq!(runtime.navigation_epoch, navigation);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(app.pages.nav.entry(surface).is_some());
    assert!(!runtime.saving && runtime.play_target.is_none());
    dismiss(app, surface);
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
}

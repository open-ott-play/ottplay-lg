use super::*;
use plx_machine::machine::UiMode;

struct VisibilityReset(plx_appkit::menu_visibility::Visibility);
impl VisibilityReset {
    fn all_visible() -> Self {
        let old = plx_appkit::menu_visibility::projected();
        plx_appkit::menu_visibility::project(Default::default());
        Self(old)
    }
}
impl Drop for VisibilityReset {
    fn drop(&mut self) { plx_appkit::menu_visibility::project(self.0); }
}

fn action(screen: &OttScreen, id: &str) -> Option<FormAction> {
    let index = screen.form.index_of(&id.to_owned())?;
    match screen.form.activate(index)? {
        Activation::Action(action) => Some(action),
        _ => None,
    }
}
fn actions(screen: &OttScreen) -> Vec<FormAction> {
    screen.keys.keys().filter_map(|id| action(screen, id)).collect()
}
fn static_page(page: &str, owner_back: Option<(&str, OttAction)>, revision: u64) -> Arc<OttView> {
    let mut choice = snapshot(1, &["choice"]).rows[0].clone();
    choice.title = "Unrelated translated label".into();
    choice.action = OttAction::EditSetting("interfaceMode.select:0:captured".into());
    let mut rows = vec![choice];
    if let Some((id, back_action)) = owner_back {
        let mut back = rows[0].clone();
        back.id = id.into(); back.title = "Return".into(); back.action = back_action;
        rows.push(back);
    }
    Arc::new(OttView { page_id: page.into(), revision, rows, selected_row: Some("choice".into()),
        ..OttView::default() })
}
fn screen(mode: UiMode) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
    screen
}

#[test]
fn static_choice_keeps_one_owner_back_and_no_generic_refresh_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (page, back_id) in [("interface-mode", "interface:back"), ("menu-items", "menu:back")] {
            let mut screen = screen(mode);
            screen.install(static_page(page, Some((back_id, OttAction::ShowSettings)), 1));
            let current = actions(&screen);
            assert_eq!(current.iter().filter(|a| matches!(a,
                FormAction::Forward(OttAction::Back | OttAction::ShowSettings))).count(), 1);
            assert!(!current.iter().any(|a| matches!(a, FormAction::Forward(OttAction::Refresh))));
            assert!(action(&screen, "nav:back").is_none());
            assert!(matches!(action(&screen, "item:choice"), Some(FormAction::Forward(OttAction::EditSetting(_)))));
            let key = screen.keys[&format!("item:{back_id}")];
            let sent = effects(|fx| screen.activate(key, false, fx));
            assert_eq!(sent.iter().filter(|item| matches!(&item.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ShowSettings, .. })))).count(), 1);
        }
    }
}

#[test]
fn neighboring_static_settings_keep_a_back_path_without_refresh() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for page in ["settings", "profile-setup:m3u"] {
            let mut screen = screen(mode);
            screen.install(static_page(page, None, 1));
            assert!(matches!(action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))));
            assert!(action(&screen, "nav:refresh").is_none());
        }
        let mut screen = screen(mode);
        screen.install(static_page("torrent-settings", Some(("torrent.back", OttAction::EditSetting("torrent.back".into()))), 1));
        assert!(action(&screen, "nav:back").is_none());
        assert!(action(&screen, "nav:refresh").is_none());
        assert!(matches!(action(&screen, "item:torrent.back"),
            Some(FormAction::Forward(OttAction::EditSetting(key))) if key == "torrent.back"));
    }
}

#[test]
fn disabled_or_mismatched_return_cannot_remove_fallback_and_browse_keeps_close() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen(mode);
        let mut disabled = (*static_page("interface-mode", Some(("interface:back", OttAction::ShowSettings)), 1)).clone();
        disabled.rows[1].enabled = false;
        screen.install(Arc::new(disabled));
        assert!(matches!(action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))));
        screen.install(static_page("interface-mode", Some(("interface:back", OttAction::ShowCatalog)), 2));
        assert!(matches!(action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))));
        screen.install(static_page("interface-mode", Some(("interface:back", OttAction::ShowSettings)), 3));
        effects(|fx| screen.open_browse(fx));
        assert!(matches!(action(&screen, "nav:back"), Some(FormAction::CloseBrowse)));
        assert!(action(&screen, "item:interface:back").is_none());
        let back = screen.keys["nav:back"];
        let sent = effects(|fx| screen.activate(back, false, fx));
        assert!(!screen.browse);
        assert!(!sent.iter().any(|item| matches!(item.fx, Fx::App(_))));
        assert!(action(&screen, "nav:back").is_none());
        assert!(matches!(action(&screen, "item:interface:back"), Some(FormAction::Forward(OttAction::ShowSettings))));
    }
}

#[test]
fn refreshing_catalogs_still_works_after_static_settings_and_live_updates() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen(mode);
        screen.install(static_page("interface-mode", Some(("interface:back", OttAction::ShowSettings)), 1));
        for page in ["channels", "settings-catalog"] {
            let mut catalog = (*snapshot(4, &["film"])).clone();
            catalog.page_id = page.into();
            screen.install(Arc::new(catalog));
            effects(|fx| screen.open_browse(fx));
            assert!(matches!(action(&screen, "nav:refresh"), Some(FormAction::Forward(OttAction::Refresh))));
            let key = screen.keys["nav:refresh"];
            let sent = effects(|fx| screen.activate(key, false, fx));
            assert_eq!(sent.iter().filter(|item| matches!(item.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Refresh, .. })))).count(), 1);
        }
        screen.install(static_page("menu-items", Some(("menu:back", OttAction::ShowSettings)), 5));
        screen.install(static_page("menu-items", Some(("menu:back", OttAction::ShowSettings)), 6));
        assert!(action(&screen, "nav:refresh").is_none(), "a retained key is not a visible action");
        assert!(action(&screen, "nav:back").is_none());
    }
}


#[test]
fn torrent_configure_keeps_one_owner_back_and_no_inert_refresh() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen(mode);
        let view = static_page("torrent-configure", Some(("torrent.back",
            OttAction::EditSetting("torrent.back".into()))), 1);
        screen.install(view.clone());
        assert!(action(&screen, "nav:refresh").is_none());
        assert!(action(&screen, "nav:back").is_none());
        assert!(matches!(action(&screen, "item:torrent.back"),
            Some(FormAction::Forward(OttAction::EditSetting(key))) if key == "torrent.back"));
        let key = screen.keys["item:torrent.back"];
        let sent = effects(|fx| screen.activate(key, false, fx));
        assert_eq!(sent.iter().filter(|item| matches!(&item.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::EditSetting(key), .. }))
                if key == "torrent.back")).count(), 1);

        let mut unavailable_back = (*view).clone();
        unavailable_back.revision = 2;
        unavailable_back.rows[1].enabled = false;
        screen.install(Arc::new(unavailable_back));
        assert!(action(&screen, "nav:refresh").is_none());
        assert!(matches!(action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))),
            "an unavailable owner action cannot strand the form");
    }
}

#[test]
fn torrent_source_list_still_offers_refresh_after_configuration() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen(mode);
        screen.install(static_page("torrent-configure", None, 1));
        assert!(action(&screen, "nav:refresh").is_none());
        screen.install(static_page("torrent-sources", None, 2));
        assert!(matches!(action(&screen, "nav:refresh"), Some(FormAction::Forward(OttAction::Refresh))));
    }
}


#[test]
fn sleep_selector_keeps_one_settings_return_and_no_catalog_refresh() {
    let _serial = plx_base::testlock::serial();
    let _visibility = VisibilityReset::all_visible();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen(mode);
        let mut view = (*static_page("sleep-timer", Some(("sleep:back", OttAction::ShowSettings)), 1)).clone();
        view.rows[0].action = OttAction::EditSetting("sleepTimeout.select:2:opaque-context".into());
        screen.install(Arc::new(view.clone()));
        assert!(action(&screen, "nav:refresh").is_none());
        assert!(action(&screen, "nav:back").is_none());
        assert!(matches!(action(&screen, "item:sleep:back"),
            Some(FormAction::Forward(OttAction::ShowSettings))));
        let current = actions(&screen);
        assert_eq!(current.iter().filter(|a| matches!(a,
            FormAction::Forward(OttAction::Back | OttAction::ShowSettings))).count(), 1);
        let choice = screen.keys["item:choice"];
        let sent = effects(|fx| screen.activate(choice, false, fx));
        assert_eq!(sent.iter().filter(|item| matches!(&item.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::EditSetting(key), .. }))
                if key == "sleepTimeout.select:2:opaque-context")).count(), 1);
        let back = screen.keys["item:sleep:back"];
        let sent = effects(|fx| screen.activate(back, false, fx));
        assert_eq!(sent.iter().filter(|item| matches!(&item.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ShowSettings, .. })))).count(), 1);

        view.revision = 2;
        view.rows[1].enabled = false;
        screen.install(Arc::new(view));
        assert!(matches!(action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))));
        assert!(action(&screen, "nav:refresh").is_none());
    }
}

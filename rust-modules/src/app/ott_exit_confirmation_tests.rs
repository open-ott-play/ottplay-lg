//! Mount actual page and modal owners. Home is observed at the loop-request boundary.
use super::*;
use plx_machine::machine::{Key, Tick};
use plx_screens::registry::LoopReq;

fn settle(app: &mut App) {
    for _ in 0..80 {
        app.loop_t = app.loop_t.wrapping_add(16);
        let tick = Tick { ms: app.loop_t, dt_us: 16_000 };
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
}

fn catalogue(mode: UiMode, page: &'static str, panel: bool) -> (Box<App>, Box<Runtime>) {
    let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    let mut runtime = Box::new(Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test()));
    runtime.remote = Remote::disabled_for_test("dev_fixture", "exit-question");
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
    runtime.loaded = true;
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
    runtime.saved.favorites_selected = page == "favorites";
    runtime.page = page;
    app.bridge.apply_ui_mode(mode);
    bridge::show_page(&mut app.pages, if panel { AppArg::Player } else { AppArg::Ott });
    settle(&mut app);
    if panel {
        player_browse::present(&mut runtime, &mut app, None, page);
        settle(&mut app);
    }
    runtime.reply = Some(current_ui_owner(&app).expect("settled OTT input owner"));
    runtime.publish(&mut app);
    settle(&mut app);
    (app, runtime)
}

fn press_back(app: &mut App, runtime: &mut Runtime, expected: OttAction) {
    app.loop_t = app.loop_t.wrapping_add(16);
    let tick = Tick { ms: app.loop_t, dt_us: 16_000 };
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Back, tick));
    let mut actions = app.bridge.take_ott_reqs().into_iter().filter_map(|request| match request {
        OttOut::Action { reply, action: action @ (OttAction::Back | OttAction::ExitAnswer { .. }) } => {
            assert_eq!(Some(reply), runtime.reply);
            Some(action)
        }
        _ => None,
    });
    let action = actions.next().expect("mounted OTT screen handles Back");
    assert_eq!(action, expected);
    assert!(actions.next().is_none());
    apply_ui_action(runtime, app, action, true, false, false);
}

fn home_requests(app: &mut App) -> usize {
    settle(app);
    app.bridge.take_reqs().iter().filter(|request| matches!(request, LoopReq::BackAtRoot)).count()
}

#[test]
fn classic_page_and_fullscreen_catalogue_confirm_once_and_cancel_preserves_work() {
    let _serial = plx_base::testlock::serial();
    for panel in [false, true] {
        for page in ["channels", "favorites"] {
            let (mut app, mut runtime) = catalogue(UiMode::Classic, page, panel);
            let owner = runtime.reply;
            let parent = app.pages.top_page();
            let before = (runtime.navigation_epoch, runtime.generation, runtime.playback_epoch);
            press_back(&mut app, &mut runtime, OttAction::Back);
            let pending = runtime.exit_confirmation.pending.as_ref().expect("primary list opens the question");
            assert_eq!(Some(pending.owner), owner);
            assert_eq!(pending.page_id, runtime.raw_view().page_id);
            assert_eq!(matches!(pending.host, Host::Panel { .. }), panel);
            assert_eq!(home_requests(&mut app), 0);
            press_back(&mut app, &mut runtime, OttAction::ExitAnswer { token: 1, accept: false });
            assert_eq!(home_requests(&mut app), 0);
            assert_eq!((runtime.navigation_epoch, runtime.generation, runtime.playback_epoch), before);
            assert_eq!(runtime.page, page);
            assert_eq!(current_ui_owner(&app), owner);
            assert_eq!(app.pages.top_page(), parent);
            press_back(&mut app, &mut runtime, OttAction::Back);
            // A local decline of an older grant cannot consume the new question.
            let late_decline = OttAction::ExitAnswer { token: 1, accept: false };
            assert_eq!(action(&mut runtime, &mut app, &late_decline), Some(Ok(())));
            let answer = OttAction::ExitAnswer { token: 2, accept: true };
            assert_eq!(action(&mut runtime, &mut app, &answer), Some(Ok(())));
            assert_eq!(home_requests(&mut app), 1);
            assert_eq!(current_ui_owner(&app), owner);
            assert_eq!(app.pages.top_page(), parent);
            assert!(action(&mut runtime, &mut app, &answer).unwrap().is_err());
            assert_eq!(home_requests(&mut app), 0);
        }
    }
}

#[test]
fn classic_panel_question_rejects_a_queued_cover_standby_changed_list_or_policy() {
    let _serial = plx_base::testlock::serial();
    for change in ["consent", "settings", "standby", "scope", "search", "policy"] {
        let (mut app, mut runtime) = catalogue(UiMode::Classic, "favorites", true);
        assert_eq!(action(&mut runtime, &mut app, &OttAction::Back), Some(Ok(())));
        assert!(runtime.exit_confirmation.pending.is_some());
        match change {
            "consent" => bridge::open_first_run_consent(&mut app.pages),
            "settings" => bridge::open_settings(&mut app.pages),
            "standby" => app.ott_standby = true,
            "scope" => runtime.saved.favorites_selected = false,
            "search" => {
                search::action(&mut runtime, &mut app, &OttAction::BeginSearch).unwrap().unwrap();
                let session = runtime.edit_session.as_ref().unwrap().id;
                let submit = OttAction::SubmitSearch { session, value: "evening".into() };
                search::action(&mut runtime, &mut app, &submit).unwrap().unwrap();
                assert!(runtime.edit_session.is_none() && runtime.search_active());
            }
            "policy" => runtime.persistence_uncertain = true,
            _ => unreachable!(),
        }
        let answer = OttAction::ExitAnswer { token: 1, accept: true };
        assert!(action(&mut runtime, &mut app, &answer).unwrap().is_err(), "{change}");
        assert_eq!(home_requests(&mut app), 0, "{change}");
    }
}

#[test]
fn classic_inner_pickers_and_modern_catalogue_keep_their_back_handlers() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = catalogue(UiMode::Classic, "channels", true);
    for page in ["categories", "settings", "providers", "profiles", "multiview-local", "vportal-quality", "guide"] {
        runtime.page = page;
        assert!(action(&mut runtime, &mut app, &OttAction::Back).is_none(), "{page}");
    }
    runtime.page = "channels";
    boot_resume::arm(&mut runtime);
    runtime.navigation_epoch = 0;
    assert!(!handles(&mut runtime, &app, &OttAction::Back));
    assert!(action(&mut runtime, &mut app, &OttAction::Back).is_none());
    let (mut app, mut runtime) = catalogue(UiMode::Modern, "channels", false);
    assert!(action(&mut runtime, &mut app, &OttAction::Back).is_none());
    runtime.page = "providers";
    assert_eq!(action(&mut runtime, &mut app, &OttAction::Back), Some(Ok(())));
    assert!(runtime.exit_confirmation.pending.is_some());
}

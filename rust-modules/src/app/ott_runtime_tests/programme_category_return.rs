//! Category return uses the actual Player offer, OTT surface and key dispatcher.
//! The inherited worker slots and native engine remain inert; no device is exercised.
use super::*;
use plx_machine::machine::UiMode;
use plx_screens::registry::{OttPlayerAction, OttPlayerRequest};

// Capture existing ownership and playback state, not a model of navigation.
fn retained_player(app: &App) -> impl std::fmt::Debug + PartialEq {
    let runtime = app.ott.as_ref().unwrap();
    (
        route::playback_identity(&app.player.session),
        route::external_source(&app.player.session),
        player::SHARED.playpos_ns.load(Ordering::Acquire),
        player::state(&app.player.session),
        app.pages.top_page(),
        app.pages.nav.tabs.stack.depth(),
        bridge::player(&app.pages).unwrap().origin,
        (runtime.playback_epoch, runtime.play_generation, runtime.generation),
        serde_json::to_value(&runtime.saved).unwrap(),
        runtime.playing.as_ref().map(|playing| (
            playing.profile.clone(), playing.entry.clone(),
            playing.identity.clone(), playing.selection_id.clone(),
        )),
        runtime.saving,
    )
}

fn assert_no_player_replacement(app: &App) {
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(app.route(), AppArg::Player));
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
    assert!(!app.refresh_hubs);
    assert!(!runtime.cancel.load(Ordering::Acquire));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.catalog_next.is_none());
}

fn open_direct_categories(app: &mut App) {
    // Use the semantic request emitted by Player; physical Blue mapping is outside this test.
    crate::app::ott_runtime::publish_remote_player_offer(app);
    let request = OttPlayerRequest {
        owner: app.pages.top_page().expect("retained Player instance"),
        source: app.bridge.ott_player_offer().expect("installed source offer").source.clone(),
        action: OttPlayerAction::OpenCategories,
    };
    assert!(crate::app::ott_runtime::remote_player_action(app, request).unwrap().is_none());
    idle(app);
    assert!(ott_surface(app).is_some());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
}

#[test]
fn classic_direct_categories_one_back_returns_to_the_same_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(UiMode::Classic);
    let before = retained_player(&app);
    let owner = app.pages.top_page().unwrap();
    open_direct_categories(&mut app);

    key(&mut app, Key::Back);

    assert!(ott_surface(&app).is_none(), "Classic Categories closes in one Back");
    assert!(!app.pages.surface_up());
    assert_eq!(input_owner(&app), owner);
    assert_eq!(retained_player(&app), before);
    assert_no_player_replacement(&app);
}

#[test]
fn classic_catalog_categories_one_back_returns_to_the_same_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(UiMode::Classic);
    let before = retained_player(&app);
    let owner = app.pages.top_page().unwrap();
    open_more(&mut app);
    choose_catalog(&mut app);
    surface_action(&mut app, OttAction::ShowCategories);
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");

    key(&mut app, Key::Back);

    assert!(ott_surface(&app).is_none(), "Classic Categories also closes when entered through the catalog");
    assert!(!app.pages.surface_up());
    assert_eq!(input_owner(&app), owner);
    assert_eq!(retained_player(&app), before);
    assert_no_player_replacement(&app);
}

#[test]
fn modern_categories_back_retains_nested_catalog_navigation() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for direct in [false, true] {
        let mut app = continuity_catalog_app(UiMode::Modern);
        let before = retained_player(&app);
        if direct {
            open_direct_categories(&mut app);
        } else {
            open_more(&mut app);
            choose_catalog(&mut app);
            surface_action(&mut app, OttAction::ShowCategories);
        }
        let surface = ott_surface(&app).unwrap();
        assert_eq!(app.ott.as_ref().unwrap().page, "categories");

        key(&mut app, Key::Back);

        assert_eq!(app.ott.as_ref().unwrap().page, "channels", "direct={direct}");
        assert_eq!(ott_surface(&app), Some(surface), "Modern keeps the catalog panel");
        assert_eq!(input_owner(&app), surface);
        assert_eq!(retained_player(&app), before);
        assert_no_player_replacement(&app);
    }
}

#[test]
fn retired_categories_owner_cannot_close_the_current_player_panel() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(UiMode::Classic);
    let before = retained_player(&app);
    open_direct_categories(&mut app);
    let retired = ott_surface(&app).unwrap();
    // Retire through the real shared modal lifecycle. Do not depend on the
    // one-Back fix to reach this separate stale-envelope assertion on baseline.
    let entry = app.pages.nav.entry_of_instance(retired).unwrap();
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
    idle(&mut app);
    retire_closing_surfaces(&mut app);
    assert!(ott_surface(&app).is_none());
    open_direct_categories(&mut app);
    let current = ott_surface(&app).unwrap();
    assert_ne!(current, retired);
    let navigation = app.ott.as_ref().unwrap().navigation_epoch;
    assert_eq!(app.ott.as_ref().unwrap().reply, Some(current));

    app.pages.emit(MachineId::Instance(retired), Fx::App(AppFx::Ott(OttOut::Action {
        reply: retired, action: OttAction::Back,
    })));
    frame(&mut app, Vec::new());
    idle(&mut app);
    retire_closing_surfaces(&mut app);

    assert_eq!(ott_surface(&app), Some(current));
    assert_eq!(input_owner(&app), current);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.reply, Some(current));
    assert_eq!(runtime.navigation_epoch, navigation);
    assert_eq!(runtime.page, "categories");
    assert_eq!(retained_player(&app), before);
    assert_no_player_replacement(&app);
}

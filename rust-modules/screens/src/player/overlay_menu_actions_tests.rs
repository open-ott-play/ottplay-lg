//! Actual overlay reducer and typed addressed feedback; no application, IO or decoder.
use super::*;
use crate::registry::{AppMsg, PageMemory, PlayerMenuOffer, PlayerMenuRequest};
use plx_appkit::more_menu::{PlaybackMenuAction as Action, PlaybackMenuCapabilities as Caps};
use plx_machine::machine::{Host, InputOwner, Tick};
use plx_ui::fixture::{FixtureArg, FixtureMeasure};

struct TestHost;
impl Host for TestHost {
    type Arg = FixtureArg; type Fx = AppFx; type Msg = AppMsg; type Elem = u32;
    type Views<'a> = (&'a plx_media::route::PlaybackSession, &'a Option<PlayerMenuOffer>, plx_data::metadata::MetadataView<'a>);
    type Init = FixtureArg; type Memory = PageMemory;
}
impl crate::registry::PlayerLike for TestHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.0 }
    fn playback_menu_offer(cx: &Cx<'_, Self>) -> Option<PlayerMenuOffer> { cx.views.1.clone() }
}
impl crate::registry::MetadataLike for TestHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.2 }
}
const ENTRY: EntryId = EntryId(73);
const OWNER: plx_machine::machine::InstanceId = plx_machine::machine::InstanceId(91);
struct Fixture {
    ps: plx_media::route::PlaybackSession,
    meta: plx_data::stores::metadata::MetadataStore,
    offer: Option<PlayerMenuOffer>,
    page: PlayerOverlayScreen,
}
impl Fixture {
    fn new() -> Self {
        let mut ps = plx_media::route::PlaybackSession::IDLE;
        plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
        let offer = Some(PlayerMenuOffer { source: plx_media::route::playback_identity(&ps).unwrap(),
            actions: Caps { categories: true, guide: true, ..Default::default() } });
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let page = PlayerOverlayScreen::new(&ps, meta.view(), ENTRY, OverlayKind::More { page: None });
        Self { ps, meta, offer, page }
    }
    fn step(&mut self, ev: ScreenEvent<TestHost>, owner: EntryId) -> Vec<plx_machine::machine::Stamped<TestHost>> {
        let cx = Cx::<TestHost> { views: (&self.ps, &self.offer, self.meta.view()), tick: Tick::default(),
            measure: &FixtureMeasure, focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(owner) };
        let mut out = Vec::new(); let mut present = plx_machine::present::Present::new();
        self.page.step(&ev, &cx, &mut Effects::new(&mut out, MachineId::Instance(OWNER), &mut present));
        out
    }
    fn request(&mut self) -> PlayerMenuRequest {
        let out = self.step(ScreenEvent::Activate(40), ENTRY);
        assert!(!out.iter().any(|e| matches!(e.fx, Fx::Nav(NavOp::Dismiss(_)))));
        out.into_iter().find_map(|e| match e.fx { Fx::App(AppFx::Player(PlayerReq::Menu(r))) => Some(r), _ => None }).unwrap()
    }
    fn note(&self) -> Option<&str> { let Panel::More(menu) = &self.page.panel else { unreachable!() }; menu.playback_failure_note() }
}
#[test]
fn menu_request_waits_for_exact_refusal_and_keeps_the_same_panel() {
    let _serial = plx_base::testlock::serial(); let mut f = Fixture::new();
    let request = f.request(); assert_eq!(request.owner, OWNER); assert_eq!(request.action, Action::Categories);
    assert!(f.step(ScreenEvent::Activate(40), ENTRY).is_empty());
    let out = f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult { request: request.clone(), outcome: Err("Source unavailable") }), ENTRY);
    assert_eq!(f.note(), Some("Source unavailable"));
    assert!(!out.iter().any(|e| matches!(e.fx, Fx::Nav(NavOp::Dismiss(_)))));
    let retry = f.request(); assert_ne!(retry.id, request.id); assert!(f.note().is_none());
}
#[test]
fn stale_menu_reply_cannot_annotate_another_request_owner_or_source() {
    let _serial = plx_base::testlock::serial();
    for kind in 0..4 {
        let mut f = Fixture::new(); let request = f.request(); let mut wrong = request.clone();
        match kind {
            0 => wrong.id = RequestId(wrong.id.0 + 1),
            1 => wrong.owner = plx_machine::machine::InstanceId(99),
            2 => { let mut other = plx_media::route::PlaybackSession::IDLE;
                plx_media::route::install_plex_playback_for_test(&mut other, plx_plex::plex::ServerId::from_raw(0), "other");
                wrong.offer.source = plx_media::route::playback_identity(&other).unwrap(); },
            _ => wrong.action = Action::Guide,
        }
        f.step(ScreenEvent::Async(wrong.id, AppMsg::PlayerMenuResult { request: wrong, outcome: Err("stale") }), ENTRY);
        assert!(f.note().is_none());
        f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult { request, outcome: Err("current") }), ENTRY);
        assert_eq!(f.note(), Some("current"));
    }
}
#[test]
fn removed_menu_row_and_covered_owner_cannot_start_an_action() {
    let _serial = plx_base::testlock::serial(); let mut f = Fixture::new();
    f.step(ScreenEvent::Tick(Tick::default()), ENTRY);
    f.offer.as_mut().unwrap().actions.categories = false;
    assert!(f.step(ScreenEvent::Activate(40), ENTRY).iter().all(|e| !matches!(e.fx, Fx::App(AppFx::Player(PlayerReq::Menu(_))))));
    assert!(f.step(ScreenEvent::Activate(41), EntryId(99)).iter().all(|e| !matches!(e.fx, Fx::App(AppFx::Player(PlayerReq::Menu(_))))));
}
#[test]
fn closing_and_source_replacement_retire_menu_feedback() {
    let _serial = plx_base::testlock::serial();
    for closing in [false, true] {
        let mut f = Fixture::new(); let request = f.request();
        if closing { f.step(ScreenEvent::Closing, ENTRY); }
        else { plx_media::route::install_plex_playback_for_test(&mut f.ps, plx_plex::plex::ServerId::from_raw(0), "other"); }
        f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult { request, outcome: Err("late") }), ENTRY);
        assert!(f.note().is_none()); assert!(f.page.menu_pending.is_none());
    }
}

#[test]
fn menu_cover_then_stale_uncover_waits_for_fresh_same_source_capabilities() {
    let _serial = plx_base::testlock::serial();
    for revoked in [false, true] {
        let mut f = Fixture::new();
        let old_request = f.request();
        let offer = f.offer.take();
        f.step(ScreenEvent::Cover, EntryId(99));
        assert!(f.page.menu_pending.is_none());
        f.step(ScreenEvent::Uncover, ENTRY); // still the old empty app publication
        let out = f.step(ScreenEvent::Activate(40), ENTRY);
        assert!(!out.iter().any(|e| matches!(e.fx, Fx::App(AppFx::Player(PlayerReq::Menu(_))))));
        f.step(ScreenEvent::Async(old_request.id, AppMsg::PlayerMenuResult { request: old_request.clone(), outcome: Err("late") }), ENTRY);
        assert!(f.note().is_none());
        if !revoked { f.offer = offer; }
        f.step(ScreenEvent::Tick(Tick::default()), ENTRY);
        let out = f.step(ScreenEvent::Activate(40), ENTRY);
        let request = out.into_iter().find_map(|e| match e.fx {
            Fx::App(AppFx::Player(PlayerReq::Menu(r))) => Some(r), _ => None,
        });
        assert_eq!(request.is_some(), !revoked);
        if let Some(request) = request { assert_ne!(request.id, old_request.id); }
    }
}

#[test]
fn menu_feedback_is_utf8_bounded_and_request_counter_never_wraps() {
    let _serial = plx_base::testlock::serial(); let mut f = Fixture::new();
    f.page.menu_next = Some(u32::MAX);
    let request = f.request(); assert_eq!(request.id.0, u32::MAX); assert!(f.page.menu_next.is_none());
    const LONG: &str = "éééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééé";
    f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult { request, outcome: Err(LONG) }), ENTRY);
    let note = f.note().unwrap(); assert_eq!(note.chars().count(), 256); assert!(note.chars().all(|c| c == 'é'));
    let out = f.step(ScreenEvent::Activate(40), ENTRY);
    assert!(!out.iter().any(|e| matches!(e.fx, Fx::App(AppFx::Player(PlayerReq::Menu(_))))));
    assert!(f.page.menu_pending.is_none());
}

#[test]
fn changed_opening_source_cannot_reacquire_old_capabilities() {
    let _serial = plx_base::testlock::serial(); let mut f = Fixture::new();
    f.step(ScreenEvent::Tick(Tick::default()), ENTRY);
    let old = std::mem::replace(&mut f.ps, plx_media::route::PlaybackSession::IDLE);
    plx_media::route::install_plex_playback_for_test(&mut f.ps, plx_plex::plex::ServerId::from_raw(0), "other");
    f.step(ScreenEvent::Tick(Tick::default()), ENTRY);
    assert!(f.page.menu_source.is_none());
    f.ps = old; // even an artificially restored old identity cannot rebind this retired panel
    let out = f.step(ScreenEvent::Activate(40), ENTRY);
    assert!(!out.iter().any(|e| matches!(e.fx, Fx::App(AppFx::Player(PlayerReq::Menu(_))))));
    assert!(f.page.menu_offer.is_none());
}

#[path = "overlay_menu_accessibility_tests.rs"]
mod accessibility;

#[path = "overlay_locale_tests.rs"]
mod locale_tests;

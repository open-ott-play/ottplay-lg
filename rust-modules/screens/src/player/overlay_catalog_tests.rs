//! UI source-receipt tests, not a native playback or dispatcher qualification.
use super::*;
use crate::registry::{AppMsg, MetadataLike, PageMemory, PlayerLike};
use plx_machine::machine::{Host, InputEvent, InputOwner, InstanceId, Key, MachineId, Source, Tick};
use plx_ui::fixture::FixtureMeasure;
use plx_ui::form::FormId;

struct CatalogHost;
impl Host for CatalogHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = (&'a plx_media::route::PlaybackSession, plx_data::metadata::MetadataView<'a>, Option<&'a plx_media::route::PlaybackIdentity>);
    type Init = plx_ui::fixture::FixtureArg;
    type Memory = PageMemory;
}
impl PlayerLike for CatalogHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.0 }
    fn ott_catalog_offer(cx: &Cx<'_, Self>) -> Option<plx_media::route::PlaybackIdentity> { cx.views.2.cloned() }
}
impl MetadataLike for CatalogHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.1 }
}
const ENTRY: EntryId = EntryId(44);
const OWNER: InstanceId = InstanceId(7);
fn key() -> u32 { plx_appkit::more_menu::Action::OpenCatalog.key().0 }
fn identity() -> plx_media::route::PlaybackIdentity {
    let mut ps = plx_media::route::PlaybackSession::default();
    plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "fixture");
    plx_media::route::playback_identity(&ps).unwrap()
}
fn panel() -> PlayerOverlayScreen {
    PlayerOverlayScreen::new(&plx_media::route::PlaybackSession::IDLE,
        plx_data::stores::metadata::MetadataStore::default().view(), ENTRY, OverlayKind::More { page: None })
}
fn deliver(page: &mut PlayerOverlayScreen, offer: Option<&plx_media::route::PlaybackIdentity>, event: ScreenEvent<CatalogHost>)
    -> (Handled, Vec<PlayerReq>, bool)
{
    let store = plx_data::stores::metadata::MetadataStore::default();
    let cx = Cx::<CatalogHost> {
        views: (&plx_media::route::PlaybackSession::IDLE, store.view(), offer),
        tick: Tick::default(), measure: &FixtureMeasure, focus: Default::default(),
        press: Default::default(), owner: InputOwner::Entry(ENTRY),
    };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let handled = page.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(OWNER), &mut present));
    let mut requests = Vec::new();
    let mut dismissed = false;
    for item in output {
        match item.fx {
            Fx::App(AppFx::Player(request)) => requests.push(request),
            Fx::Nav(NavOp::Dismiss(_)) => dismissed = true,
            _ => {}
        }
    }
    (handled, requests, dismissed)
}
fn keys(page: &PlayerOverlayScreen) -> Vec<u32> {
    let Panel::More(menu) = &page.panel else { panic!("More panel") };
    menu.keys()
}
fn press(edge: Edge) -> ScreenEvent<CatalogHost> {
    ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Ok,
        sym: consts::SDLK_RETURN, wcode: 0, edge, at_edge: false },
        at: Tick { ms: 1000, dt_us: 0 }, source: Source::Sdl })
}

#[test]
fn more_catalog_keyboard_and_pointer_activation_keep_the_captured_source_and_menu_open() {
    let _lock = plx_base::testlock::serial();
    for keyboard in [false, true] {
        let source = identity();
        let mut page = panel();
        deliver(&mut page, Some(&source), ScreenEvent::Tick(Tick::default()));
        assert!(keys(&page).contains(&key()));
        if keyboard {
            let (handled, requests, closed) = deliver(&mut page, Some(&source), press(Edge::Down));
            assert_eq!(handled, Handled::No, "the engine activates the focused Bare row");
            assert!(requests.is_empty()); assert!(!closed);
        }
        let (_, requests, closed) = deliver(&mut page, Some(&source), ScreenEvent::Activate(key()));
        assert!(!closed, "only application admission may replace this overlay");
        assert_eq!(requests.len(), 1);
        let PlayerReq::OpenCatalog(request) = &requests[0] else { panic!("addressed catalog request only") };
        assert_eq!(request.owner, OWNER); assert_eq!(request.source, source);
        let (_, released, closed) = deliver(&mut page, Some(&source), press(Edge::Up));
        assert!(released.is_empty()); assert!(!closed);
    }
}

#[test]
fn more_catalog_no_initial_offer_cannot_acquire_a_later_source() {
    let _lock = plx_base::testlock::serial();
    let source = identity(); let mut page = panel();
    deliver(&mut page, None, ScreenEvent::Tick(Tick::default()));
    let (_, requests, closed) = deliver(&mut page, Some(&source), ScreenEvent::Activate(key()));
    assert!(!keys(&page).contains(&key())); assert!(requests.is_empty()); assert!(!closed);
}

#[test]
fn more_catalog_revocation_rejects_the_old_key_and_never_rebinds_after_aba() {
    let _lock = plx_base::testlock::serial();
    let a = identity(); let b = identity(); assert_ne!(a, b);
    for replacement in [None, Some(&b)] {
        let mut page = panel();
        deliver(&mut page, Some(&a), ScreenEvent::Tick(Tick::default()));
        let Panel::More(menu) = &mut page.panel else { panic!("More panel") };
        assert!(menu.focus_key(key()));
        let (_, requests, closed) = deliver(&mut page, replacement, ScreenEvent::Activate(key()));
        assert!(requests.is_empty(), "a removed key must not activate its neighbour"); assert!(!closed);
        assert!(!keys(&page).contains(&key()));
        let (_, requests, closed) = deliver(&mut page, Some(&a), ScreenEvent::Activate(key()));
        assert!(!keys(&page).contains(&key())); assert!(requests.is_empty()); assert!(!closed);
    }
}

#[test]
fn more_catalog_closing_retires_the_offer_before_a_late_activation() {
    let _lock = plx_base::testlock::serial();
    let source = identity(); let mut page = panel();
    deliver(&mut page, Some(&source), ScreenEvent::Tick(Tick::default()));
    deliver(&mut page, Some(&source), ScreenEvent::Closing);
    let (_, requests, _) = deliver(&mut page, Some(&source), ScreenEvent::Activate(key()));
    assert!(requests.is_empty()); assert!(!keys(&page).contains(&key()));
}

//! Real fullscreen screen admission: the passive offer cannot enable playback.
use super::*;
use crate::registry::{AppMsg, MetadataLike, PageMemory};
use plx_appkit::{multiview_chrome::{ChromeFrame, Event, SlotState, SlotView, ViewState},
    multiview_layout::{Layout, Mode}};
use plx_machine::machine::{Host, InputEvent, InputOwner, Key, MachineId, Source, Tick};
use plx_ui::fixture::FixtureMeasure;

#[derive(Clone, Copy)]
struct Views<'a> {
    session: &'a plx_media::route::PlaybackSession,
    metadata: plx_data::metadata::MetadataView<'a>,
    offer: Option<&'a ChromeFrame>,
}
struct TestHost;
impl Host for TestHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = Views<'a>;
    type Init = plx_ui::fixture::FixtureArg;
    type Memory = PageMemory;
}
impl PlayerLike for TestHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.session }
    fn multiview_offer(cx: &Cx<'_, Self>) -> Option<ChromeFrame> { cx.views.offer.cloned() }
}
impl MetadataLike for TestHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.metadata }
}
fn frame(owned: bool) -> ChromeFrame {
    ChromeFrame::new(Layout::new(Rect::FULL, Mode::Pip, 1.0).unwrap(),
        &[SlotView { id: 0, title: "Admitted fixture main".into(), state: SlotState::Playing }],
        ViewState { revision: 415, geometry_revision: 19, active: Some(0), audio: Some(0), focused: Some(0), input_owned: owned }).unwrap()
}
fn key(sym: u32, edge: Edge) -> InputKind<u32> {
    InputKind::Key { key: Key::Other, sym, wcode: 0, edge, at_edge: false }
}
fn deliver(page: &mut PlayerScreen, offer: Option<&ChromeFrame>, kind: InputKind<u32>) -> (Handled, Vec<PlayerReq>) {
    deliver_screen(page, offer, ScreenEvent::Input(InputEvent { kind,
        at: Tick { ms: 1_000, dt_us: 0 }, source: Source::Sdl }))
}
fn deliver_screen(page: &mut PlayerScreen, offer: Option<&ChromeFrame>, event: ScreenEvent<TestHost>)
    -> (Handled, Vec<PlayerReq>) {
    let session = plx_media::route::PlaybackSession::IDLE;
    let store = plx_data::stores::metadata::MetadataStore::default();
    let cx = Cx::<TestHost> { views: Views { session: &session, metadata: store.view(), offer },
        tick: Tick { ms: 1_000, dt_us: 0 }, measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(EntryId(53)) };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let handled = page.step(&event, &cx,
        &mut Effects::new(&mut output, MachineId::Instance(InstanceId(8)), &mut present));
    (handled, output.into_iter().filter_map(|item| match item.fx {
        Fx::App(AppFx::Player(request)) => Some(request), _ => None,
    }).collect())
}
#[test]
fn admitted_multiview_primary_click_and_keys_emit_revision_bound_events() {
    let _lock = plx_base::testlock::serial();
    let frame = frame(true);
    let mut page = PlayerScreen::new(EntryId(53));
    let inset = frame.layout().slot(1).unwrap().bounds;
    let (handled, requests) = deliver(&mut page, Some(&frame),
        InputKind::Click { x: inset.cx(), y: inset.cy(), hit: None });
    assert_eq!(handled, Handled::Yes);
    assert_eq!(requests, vec![PlayerReq::Multiview(Event::ActivateSlot { revision: 415, slot: 1 })]);
    let (_, requests) = deliver(&mut page, Some(&frame), key(consts::SDLK_RETURN, Edge::Down));
    assert_eq!(requests, vec![PlayerReq::Multiview(Event::ActivateSlot { revision: 415, slot: 0 })]);
    let (_, requests) = deliver(&mut page, Some(&frame), key(consts::SDLK_RIGHT, Edge::Down));
    assert_eq!(requests, vec![PlayerReq::Multiview(Event::FocusSlot { revision: 415, slot: 1 })]);
    assert_eq!(frame.state().active, Some(0), "renderer events never commit active media");
}
#[test]
fn absent_or_nonowning_offer_retains_identical_ordinary_fullscreen_input() {
    let _lock = plx_base::testlock::serial();
    let inactive = frame(false);
    for sym in [consts::SDLK_UP, consts::SDLK_RIGHT, consts::SDLK_RETURN, consts::WCODE_PAUSE] {
        let baseline = deliver(&mut PlayerScreen::new(EntryId(53)), None, key(sym, Edge::Down));
        let passive = deliver(&mut PlayerScreen::new(EntryId(53)), Some(&inactive), key(sym, Edge::Down));
        assert_eq!(baseline, passive);
        assert!(!passive.1.iter().any(|request| matches!(request, PlayerReq::Multiview(_))));
    }
}
#[test]
fn owned_selection_does_not_activate_on_hover_scroll_drag_keyup_or_outside_click() {
    let _lock = plx_base::testlock::serial();
    let frame = frame(true);
    for kind in [InputKind::Pointer { x: 100.0, y: 100.0, hit: None },
        InputKind::Wheel { dy: 1.0 }, InputKind::Drag { x: 100.0, y: 100.0, hit: None },
        key(consts::SDLK_RETURN, Edge::Up), key(consts::SDLK_RETURN, Edge::Repeat),
        InputKind::Click { x: -1.0, y: -1.0, hit: None }] {
        let (_, requests) = deliver(&mut PlayerScreen::new(EntryId(53)), Some(&frame), kind);
        assert!(requests.is_empty(), "no playback/selection request from non-confirmation: {requests:?}");
    }
}
#[test]
fn local_repair_modal_wins_over_admitted_scene_selection() {
    let _lock = plx_base::testlock::serial();
    let frame = frame(true);
    let mut page = PlayerScreen::new(EntryId(53));
    page.repair_alert.open(c"Existing repair");
    for sym in [consts::SDLK_UP, consts::SDLK_RETURN] {
        let (_, requests) = deliver(&mut page, Some(&frame), key(sym, Edge::Down));
        assert!(!requests.iter().any(|request| matches!(request, PlayerReq::Multiview(_))));
    }
}
#[test]
fn real_player_draw_uses_admitted_frame_and_absence_draws_no_slot_chrome() {
    let _lock = plx_base::testlock::serial();
    let frame = frame(false);
    let session = plx_media::route::PlaybackSession::IDLE;
    let store = plx_data::stores::metadata::MetadataStore::default();
    for offer in [None, Some(&frame)] {
        let cx = Cx::<TestHost> { views: Views { session: &session, metadata: store.view(), offer },
            tick: Tick::default(), measure: &FixtureMeasure, focus: Default::default(),
            press: Default::default(), owner: InputOwner::Entry(EntryId(53)) };
        let mut page = PlayerScreen::new(EntryId(53));
        let runs = plx_ui::draw_census::capture_runs(|| {
            let mut draw = DrawFrame::new(&cx, plx_ui::Painter::recording());
            Screen::<TestHost>::draw(&mut page, &mut draw);
        });
        assert_eq!(runs.iter().any(|run| run.text == "Admitted fixture main"), offer.is_some());
    }
}

#[test]
fn scene_selection_rejects_delayed_hud_activation_from_previous_input_owner() {
    let _lock = plx_base::testlock::serial();
    let frame = frame(true);
    let mut page = PlayerScreen::new(EntryId(53));
    let (handled, requests) = deliver_screen(&mut page, Some(&frame),
        ScreenEvent::Activate(player_hud::control_elem(0)));
    assert_eq!(handled, Handled::Yes);
    assert!(requests.is_empty());
}

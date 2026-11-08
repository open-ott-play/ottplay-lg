//! The real HUD geometry controls focus eligibility and late event admission.
use super::*;
use plx_machine::machine::{Host, InputEvent, InputOwner, MachineId, PressId, Source, Tick, UiMode};
use plx_ui::focus::FocusEngine;
use plx_ui::screen::FocusTarget;

struct ModeHost;
impl Host for ModeHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx; type Msg = crate::registry::AppMsg; type Elem = u32;
    type Views<'a> = (UiMode, &'a plx_media::route::PlaybackSession, plx_data::metadata::MetadataView<'a>);
    type Init = plx_ui::fixture::FixtureArg; type Memory = crate::registry::PageMemory;
    fn ui_mode(views: &Self::Views<'_>) -> UiMode { views.0 }
}
impl PlayerLike for ModeHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.1 }
}
impl crate::registry::MetadataLike for ModeHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.2 }
}
const ENTRY: EntryId = EntryId(81);

fn context<'a>(mode: UiMode, ps: &'a plx_media::route::PlaybackSession,
    meta: plx_data::metadata::MetadataView<'a>) -> Cx<'a, ModeHost> {
    Cx { views: (mode, ps, meta), tick: Tick { ms: 1_000, dt_us: 0 },
        measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(),
        press: Default::default(), owner: InputOwner::Entry(ENTRY) }
}

#[test]
fn real_focus_engine_cannot_follow_links_to_hidden_classic_discs_or_tabs() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    let meta = plx_data::stores::metadata::MetadataStore::default();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let cx = context(mode, &ps, meta.view());
        let page = PlayerScreen::new(ENTRY);
        let mut groups = Vec::new(); page.groups(&cx, &mut groups);
        for group in [GROUP_ROW, GROUP_TABS] {
            assert_eq!(groups.iter().any(|g| g.id == group), mode == UiMode::Modern);
        }
        let mut links = Vec::new(); Screen::<ModeHost>::links(&page, &mut links);
        for direction in [Dir::Up, Dir::Down] {
            let mut engine = FocusEngine::new();
            engine.enter(cx.owner, &page, FocusTarget::ContainerGroup(GROUP_SCRUB), None, &cx);
            engine.move_dir(cx.owner, &page, &links, direction, &cx);
            let current = engine.current(cx.owner).unwrap();
            let expected = if mode == UiMode::Classic { GROUP_SCRUB }
                else if direction == Dir::Up { GROUP_ROW } else { GROUP_TABS };
            assert_eq!(page.group_of(&current.elem, &cx), Some(expected));
            assert!(page.place(&current.elem, &cx, At::Drawn).is_some());
        }
        for elem in [player_hud::control_elem(player_hud::BTN_MORE), player_hud::ELEM_TAB_BASE] {
            let mut engine = FocusEngine::new();
            engine.enter(cx.owner, &page, FocusTarget::ContainerGroup(GROUP_SCRUB),
                Some(FocusKey { entry: ENTRY, elem }), &cx);
            assert_eq!(engine.current(cx.owner).unwrap().elem,
                if mode == UiMode::Classic { player_hud::ELEM_SCRUB } else { elem });
        }
    }
}

fn deliver(page: &mut PlayerScreen, cx: &Cx<'_, ModeHost>, ev: ScreenEvent<ModeHost>) -> Vec<PlayerReq> {
    let mut out = Vec::new(); let mut present = plx_machine::present::Present::new();
    page.step(&ev, cx, &mut Effects::new(&mut out,
        MachineId::Instance(plx_machine::machine::InstanceId(82)), &mut present));
    out.into_iter().filter_map(|event| match event.fx { Fx::App(AppFx::Player(req)) => Some(req), _ => None }).collect()
}

#[test]
fn late_classic_focus_click_activate_and_commit_do_not_target_hidden_controls() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    let meta = plx_data::stores::metadata::MetadataStore::default();
    for elem in [player_hud::control_elem(player_hud::BTN_MORE), player_hud::ELEM_TAB_BASE] {
        for event in 0..5 {
            let mut page = PlayerScreen::new(ENTRY); page.hud.visible_at_press = true;
            page.hud.nav.focus = PlayerScreen::hud_group_of(elem).unwrap();
            page.hud.nav.btn = player_hud::BTN_MORE;
            let mut cx = context(UiMode::Classic, &ps, meta.view());
            cx.focus.current = Some(FocusKey { entry: ENTRY, elem });
            let ev = match event {
                0 => ScreenEvent::FocusMoved { from: None, to: cx.focus.current.unwrap(), by: By::Dir },
                1 => ScreenEvent::Activate(elem),
                2 => ScreenEvent::PressCommit(PressId(1)),
                3 => ScreenEvent::Input(InputEvent { kind: InputKind::Click { x: 0.0, y: 0.0, hit: Some(elem) },
                    at: cx.tick, source: Source::Sdl }),
                _ => ScreenEvent::Input(InputEvent { kind: InputKind::Key {
                    key: plx_machine::machine::Key::Ok, sym: 13, wcode: 0,
                    edge: Edge::Down, at_edge: false }, at: cx.tick, source: Source::Sdl }),
            };
            assert!(deliver(&mut page, &cx, ev).is_empty(), "late event {event} for {elem}");
            assert_eq!(page.hud.nav.focus, GROUP_SCRUB);
        }
    }
    let mut page = PlayerScreen::new(ENTRY);
    let mut cx = context(UiMode::Modern, &ps, meta.view());
    cx.focus.current = Some(FocusKey { entry: ENTRY, elem: player_hud::control_elem(player_hud::BTN_MORE) });
    assert!(!deliver(&mut page, &cx, ScreenEvent::PressCommit(PressId(1))).is_empty());
}

#[test]
fn classic_contextual_skip_and_up_next_keep_geometry_focus_and_commit() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    let meta = plx_data::stores::metadata::MetadataStore::default();
    use plx_data::metadata::{Marker, MarkerKind};
    for kind in [MarkerKind::Intro, MarkerKind::Credits] {
        let mut page = PlayerScreen::new(ENTRY);
        page.slot = player_hud::slot_for(Some(Marker { kind, start_ms: 1_000, end_ms: 2_000,
            final_seg: kind == MarkerKind::Credits }), kind == MarkerKind::Credits,
            plx_media::route::NextEpisodeMode::Countdown);
        let mut cx = context(UiMode::Classic, &ps, meta.view());
        let mut groups = Vec::new(); page.groups(&cx, &mut groups);
        assert!(groups.iter().any(|g| g.id == GROUP_ROW));
        assert!(!groups.iter().any(|g| g.id == GROUP_TABS));
        let mut links = Vec::new(); Screen::<ModeHost>::links(&page, &mut links);
        let mut engine = FocusEngine::new();
        engine.enter(cx.owner, &page, FocusTarget::ContainerGroup(GROUP_SCRUB), None, &cx);
        engine.move_dir(cx.owner, &page, &links, Dir::Up, &cx);
        assert_eq!(page.group_of(&engine.current(cx.owner).unwrap().elem, &cx), Some(GROUP_ROW));
        for i in 0..page.slot.items() {
            let elem = player_hud::control_elem(i);
            assert!(page.place(&elem, &cx, At::Drawn).is_some());
            cx.focus.current = Some(FocusKey { entry: ENTRY, elem });
            assert!(!deliver(&mut page, &cx, ScreenEvent::PressCommit(PressId(1))).is_empty());
        }
    }
}

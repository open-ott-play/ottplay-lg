//! Background pointer intent only: real screen reducer, no renderer or engine.
use super::*;
use plx_machine::machine::{Host, InputEvent, InputOwner, MachineId, Source, Tick, UiMode};
use crate::registry::{AppMsg, MetadataLike, PageMemory, OttPlayerAction};

struct PointerHost;
impl Host for PointerHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx; type Msg = AppMsg; type Elem = u32;
    type Views<'a> = (UiMode, &'a plx_media::route::PlaybackSession,
        plx_data::metadata::MetadataView<'a>, Option<&'a plx_media::route::PlaybackIdentity>);
    type Init = Self::Arg; type Memory = PageMemory;
    fn ui_mode(views: &Self::Views<'_>) -> UiMode { views.0 }
}
impl PlayerLike for PointerHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.1 }
    fn ott_catalog_offer(cx: &Cx<'_, Self>) -> Option<plx_media::route::PlaybackIdentity> { cx.views.3.cloned() }
}
impl MetadataLike for PointerHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.2 }
}
const ENTRY: EntryId = EntryId(81);
const OWNER: InstanceId = InstanceId(82);
fn click(page: &mut PlayerScreen, cx: &Cx<'_, PointerHost>, x: f32, y: f32, hit: Option<u32>) -> Vec<PlayerReq> {
    let mut out = Vec::new(); let mut present = plx_machine::present::Present::new();
    page.step(&ScreenEvent::Input(InputEvent { kind: InputKind::Click { x, y, hit },
        at: cx.tick, source: Source::Sdl }), cx,
        &mut Effects::new(&mut out, MachineId::Instance(OWNER), &mut present));
    out.into_iter().filter_map(|e| match e.fx { Fx::App(AppFx::Player(r)) => Some(r), _ => None }).collect()
}

#[test]
fn classic_top_band_is_bounded_and_rechecks_source_owner_and_gestures() {
    let _lock = plx_base::testlock::serial();
    use std::sync::atomic::Ordering::Relaxed;
    struct RestorePublished { hud_until: u32, scrub_ns: i64 }
    impl Drop for RestorePublished {
        fn drop(&mut self) {
            plx_media::player::TX.hud_until.store(self.hud_until, Relaxed);
            plx_media::player::TX.scrub_ns.store(self.scrub_ns, Relaxed);
        }
    }
    let _restore = RestorePublished {
        hud_until: plx_media::player::TX.hud_until.load(Relaxed),
        scrub_ns: plx_media::player::TX.scrub_ns.load(Relaxed),
    };
    let mut ps = plx_media::route::PlaybackSession::IDLE;
    plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "42");
    let source = plx_media::route::playback_identity(&ps).unwrap();
    let meta = plx_data::stores::metadata::MetadataStore::default();
    let mut cx = Cx::<PointerHost> { views: (UiMode::Classic, &ps, meta.view(), Some(&source)),
        tick: Tick { ms: 1000, dt_us: 0 }, measure: &plx_ui::fixture::FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
    for (x, y) in [(0.0, 0.0), (1919.0, 215.0)] {
        let mut page = PlayerScreen::new(ENTRY);
        let req = click(&mut page, &cx, x, y, None);
        assert!(matches!(req.as_slice(), [PlayerReq::OttAction(r)]
            if r.owner == OWNER && r.source.playback() == Some(&source) && r.action == OttPlayerAction::OpenMenu));
    }
    for (x, y) in [(-1.0, 0.0), (1920.0, 0.0), (0.0, -1.0), (0.0, 1080.0),
        (f32::NAN, 10.0), (10.0, f32::INFINITY)] {
        assert!(click(&mut PlayerScreen::new(ENTRY), &cx, x, y, None).is_empty());
    }
    for refusal in 0..7 {
        let mut page = PlayerScreen::new(ENTRY);
        cx.owner = InputOwner::Entry(ENTRY); cx.press = Default::default(); cx.views.3 = Some(&source);
        match refusal {
            0 => page.set_hud_policy(HudPolicy::Lifted),
            1 => {
                // Seat the real pointer gesture: drag alone with the idle ns=-1
                // is not a live scrub and cannot be produced by a scrubber click.
                page.model.duration_ns = 60_000_000_000;
                let bar = page.hud_geometry(&cx).playbar;
                assert!(click(&mut page, &cx, bar.cx(), bar.cy(), Some(player_hud::ELEM_SCRUB)).is_empty());
                assert!(page.scrub.drag && page.scrub.is_live(), "scrubber click must own a live target");
            },
            2 => page.transport_tap = true,
            3 => cx.press.owner = Some(FocusKey { entry: ENTRY, elem: player_hud::ELEM_SCRUB }),
            4 => cx.owner = InputOwner::Entry(EntryId(99)),
            5 => cx.views.3 = None,
            _ => page.busy = player_hud::Busy::Readout(plx_ui::widgets::StatusKind::Failed, c"failed"),
        }
        assert!(click(&mut page, &cx, 100.0, 100.0, None).is_empty(), "refusal {refusal}");
        if refusal == 1 {
            // Only the scrub is blocking: the same source/owner remains eligible
            // after its target is cancelled, without resetting any other guard.
            page.scrub.cancel();
            assert!(matches!(click(&mut page, &cx, 100.0, 100.0, None).as_slice(),
                [PlayerReq::OttAction(r)] if r.owner == OWNER && r.source.playback() == Some(&source)
                    && r.action == OttPlayerAction::OpenMenu));
        }
    }
    cx.owner = InputOwner::Entry(ENTRY); cx.press = Default::default(); cx.views.3 = Some(&source);
    let mut replacement = plx_media::route::PlaybackSession::IDLE;
    plx_media::route::install_plex_playback_for_test(&mut replacement, plx_plex::plex::ServerId::from_raw(0), "other");
    cx.views.1 = &replacement;
    assert!(click(&mut PlayerScreen::new(ENTRY), &cx, 100.0, 100.0, None).is_empty());
}

#[test]
fn hidden_control_targets_are_not_reinterpreted_and_modern_keeps_picture_toggle() {
    let _lock = plx_base::testlock::serial();
    let mut ps = plx_media::route::PlaybackSession::IDLE;
    plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "42");
    let source = plx_media::route::playback_identity(&ps).unwrap();
    let meta = plx_data::stores::metadata::MetadataStore::default();
    let mut cx = Cx::<PointerHost> { views: (UiMode::Classic, &ps, meta.view(), Some(&source)),
        tick: Tick { ms: 1000, dt_us: 0 }, measure: &plx_ui::fixture::FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
    for elem in [player_hud::control_elem(player_hud::BTN_MORE), player_hud::ELEM_TAB_BASE] {
        assert!(click(&mut PlayerScreen::new(ENTRY), &cx, 100.0, 100.0, Some(elem)).is_empty());
    }
    // The boundary itself and lower picture keep the pre-existing click path.
    assert!(matches!(click(&mut PlayerScreen::new(ENTRY), &cx, 100.0, 216.0, None).as_slice(),
        [PlayerReq::Transport(None)]));
    cx.views.0 = UiMode::Modern;
    assert!(matches!(click(&mut PlayerScreen::new(ENTRY), &cx, 100.0, 100.0, None).as_slice(),
        [PlayerReq::Transport(None)]));
}

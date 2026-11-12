//! Ordinary Player input tests. These prove source-bound screen effects, not
//! application admission, physical remote codes or native decoder continuity.
use super::*;
use crate::registry::{AppMsg, MetadataLike, OttPlayerAction as Action, OttPlayerOffer, PageMemory};
use plx_machine::machine::{Host, InputEvent, InputOwner, Key, MachineId, Source, Tick, UiMode};
use plx_ui::fixture::FixtureMeasure;

#[derive(Clone, Copy)]
struct Views<'a> {
    session: &'a plx_media::route::PlaybackSession,
    metadata: plx_data::metadata::MetadataView<'a>,
    offer: Option<&'a OttPlayerOffer>,
    mode: UiMode,
    prelude: Option<bool>,
}
struct RemoteHost;
impl Host for RemoteHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = Views<'a>;
    type Init = plx_ui::fixture::FixtureArg;
    type Memory = PageMemory;
    fn ui_mode(views: &Self::Views<'_>) -> UiMode { views.mode }
}
impl PlayerLike for RemoteHost {
    fn hud_key_prelude(cx: &Cx<'_, Self>, source: Source) -> Option<bool> {
        if matches!(source, Source::Sdl | Source::RemoteFifo) { cx.views.prelude } else { None }
    }
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.session }
    fn ott_player_offer(cx: &Cx<'_, Self>, source: Source) -> Option<OttPlayerOffer> {
        // This fixture represents a native TV host; the application adapter
        // separately proves that physical desktop Sdl events have no offer.
        matches!(source, Source::Sdl | Source::RemoteFifo).then(|| cx.views.offer.cloned()).flatten()
    }
}
impl MetadataLike for RemoteHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.metadata }
}
const ENTRY: EntryId = EntryId(54);
const OWNER: InstanceId = InstanceId(17);

fn playback() -> (plx_media::route::PlaybackSession, OttPlayerOffer) {
    let mut session = plx_media::route::PlaybackSession::default();
    plx_media::route::reset_player_control_for_test(&session);
    plx_media::route::install_plex_playback_for_test(&mut session,
        plx_plex::plex::ServerId::from_raw(0), "remote-fixture");
    let source = plx_media::route::playback_identity(&session).unwrap();
    (session, OttPlayerOffer { source: source.into(), recovery_only: false, seek_arrows: false, accept: Action::OpenCatalog, guide: true })
}
fn bare(session: &plx_media::route::PlaybackSession) -> PlayerScreen {
    let mut page = PlayerScreen::new(ENTRY);
    page.hud.dismissed = true;
    // Same pre-clear capture that note_global_press uses for a bound key.
    page.hud.note_fresh_press(session, 1_000, false);
    assert!(!page.hud.visible_at_press);
    assert!(!page.hud.dismissed, "global input has already cleared dismissal");
    page
}
#[allow(clippy::too_many_arguments)]
fn send(page: &mut PlayerScreen, session: &plx_media::route::PlaybackSession,
    offer: Option<&OttPlayerOffer>, mode: UiMode, sym: u32, wcode: u32,
    edge: Edge, at_edge: bool, focused: Option<u32>) -> (Handled, Vec<PlayerReq>)
{
    send_source(page, session, offer, mode, sym, wcode, edge, at_edge, focused, Source::Sdl)
}
#[allow(clippy::too_many_arguments)]
fn send_source(page: &mut PlayerScreen, session: &plx_media::route::PlaybackSession,
    offer: Option<&OttPlayerOffer>, mode: UiMode, sym: u32, wcode: u32,
    edge: Edge, at_edge: bool, focused: Option<u32>, source: Source) -> (Handled, Vec<PlayerReq>)
{
    deliver(page, session, offer, mode, sym, wcode, edge, at_edge, focused, source, None)
}
#[allow(clippy::too_many_arguments)]
fn deliver(page: &mut PlayerScreen, session: &plx_media::route::PlaybackSession,
    offer: Option<&OttPlayerOffer>, mode: UiMode, sym: u32, wcode: u32,
    edge: Edge, at_edge: bool, focused: Option<u32>, source: Source,
    prelude: Option<bool>) -> (Handled, Vec<PlayerReq>)
{
    let store = plx_data::stores::metadata::MetadataStore::default();
    let mut cx = Cx::<RemoteHost> {
        views: Views { session, metadata: store.view(), offer, mode, prelude },
        tick: Tick { ms: 1_000, dt_us: 0 }, measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY),
    };
    cx.focus.current = focused.map(|elem| FocusKey { entry: ENTRY, elem });
    let event = ScreenEvent::<RemoteHost>::Input(InputEvent {
        kind: InputKind::Key { key: Key::Other, sym, wcode, edge, at_edge },
        at: cx.tick, source,
    });
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let handled = page.step(&event, &cx,
        &mut Effects::new(&mut output, MachineId::Instance(OWNER), &mut present));
    let requests = output.into_iter().filter_map(|item| match item.fx {
        Fx::App(AppFx::Player(request)) => Some(request), _ => None,
    }).collect();
    (handled, requests)
}
fn ott(requests: &[PlayerReq]) -> Vec<&crate::registry::OttPlayerRequest> {
    requests.iter().filter_map(|r| match r { PlayerReq::OttAction(r) => Some(r), _ => None }).collect()
}

#[test]
fn hidden_fullscreen_buttons_emit_one_owned_action_in_both_themes() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (sym, raw, expected) in [
            (consts::SDLK_UP, 82, Action::Next), (consts::SDLK_DOWN, 81, Action::Previous),
            (consts::SDLK_LEFT, 80, Action::OpenMenu), (consts::SDLK_RIGHT, 79, Action::ShowInfo),
            (consts::SDLK_RETURN, 40, Action::OpenCatalog),
            (0, 300, Action::Next), (0, 301, Action::Previous),
            (consts::SDLK_PAGEUP, 0, Action::Next),
            (consts::SDLK_PAGEDOWN, 0, Action::Previous),
            (0, 486, Action::OpenGuide), (0, 488, Action::OpenMenu), (0, 489, Action::OpenCategories),
        ] {
            let mut page = bare(&session);
            let (handled, requests) = send(&mut page, &session, Some(&offer), mode,
                sym, raw, Edge::Down, false, Some(player_hud::ELEM_SCRUB));
            assert_eq!(handled, Handled::Yes);
            assert_eq!(requests.len(), 1, "{mode:?}: {sym}/{raw}");
            let request = ott(&requests)[0];
            assert_eq!(request.owner, OWNER); assert_eq!(request.source, offer.source);
            assert_eq!(request.action, expected);
        }
    }
}

#[test]
fn failed_source_offer_dispatches_channel_steps_and_explicit_catalogue_only() {
    let _lock = plx_base::testlock::serial();
    let (_, mut offer) = playback();
    offer.recovery_only = true;
    let session = plx_media::route::PlaybackSession::default();
    plx_media::route::reset_player_control_for_test(&session);
    assert!(!plx_media::route::playback_identity_matches(&session, offer.source.playback().unwrap()));
    struct Restore(u8);
    impl Drop for Restore {
        fn drop(&mut self) { plx_media::player::restore_state_for_test(self.0); }
    }
    let _state = Restore(plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Error));
    assert!(player_hud::transport_hidden(&session));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (code, action) in [(consts::WCODE_CH_UP_KEY, Action::Next),
            (consts::WCODE_CH_DOWN_KEY, Action::Previous)] {
            for (edge, at_edge, count) in [(Edge::Down, false, 1), (Edge::Repeat, false, 1),
                (Edge::Up, false, 0), (Edge::Down, true, 0)] {
                let (_, requests) = send(&mut PlayerScreen::new(ENTRY), &session, Some(&offer), mode,
                    0, code, edge, at_edge, None);
                assert_eq!(ott(&requests).len(), count);
                if count == 1 {
                    assert_eq!(ott(&requests)[0].action, action);
                    assert_eq!(ott(&requests)[0].source, offer.source);
                }
            }
            for policy in [HudPolicy::Lifted, HudPolicy::Hidden] {
                let mut page = PlayerScreen::new(ENTRY); page.hud_policy = policy;
                let (_, requests) = send(&mut page, &session, Some(&offer), mode,
                    0, code, Edge::Down, false, None);
                assert!(ott(&requests).is_empty(), "a covering surface still owns input");
            }
        }
        for visible in [false, true] {
            for (edge, expected) in [(Edge::Down, 1), (Edge::Repeat, 0), (Edge::Up, 0)] {
                let mut page = PlayerScreen::new(ENTRY); page.hud.visible_at_press = visible;
                let (_, requests) = send(&mut page, &session, Some(&offer), mode,
                    consts::SDLK_RETURN, 40, edge, false, None);
                assert_eq!(ott(&requests).len(), expected);
                if expected == 1 { assert_eq!(ott(&requests)[0].action, Action::OpenCatalog); }
            }
        }
        for (sym, code) in [(consts::SDLK_UP, 82), (consts::SDLK_DOWN, 81),
            (consts::SDLK_LEFT, 80), (consts::SDLK_RIGHT, 79),
            (0, consts::WCODE_RED), (0, consts::WCODE_BLUE), (0, consts::WCODE_YELLOW)] {
            let (_, requests) = send(&mut PlayerScreen::new(ENTRY), &session, Some(&offer), mode,
                sym, code, Edge::Down, false, None);
            assert!(ott(&requests).is_empty(), "recovery grants no transport or menu action");
        }
    }
}

#[test]
fn channel_numbers_allow_visible_hud_but_numeric_seeks_keep_the_bare_video_gate() {
    let _lock = plx_base::testlock::serial();
    let (session, _) = playback();
    struct Restore(u8);
    impl Drop for Restore {
        fn drop(&mut self) { plx_media::player::restore_state_for_test(self.0); }
    }
    let _state = Restore(plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing));
    let mut page = PlayerScreen::new(ENTRY);
    page.hud.dismissed = true;
    assert!(page.accepts_channel_number());
    assert!(page.accepts_numeric_seek(&session, 1_000, false));
    page.hud.dismissed = false;
    page.hud.extend_default(1_000);
    assert!(page.hud.visible(&session, 1_000, false));
    assert!(page.accepts_channel_number());
    assert!(!page.accepts_numeric_seek(&session, 1_000, false));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Error);
    page.hud.dismissed = true;
    assert!(page.accepts_channel_number(), "the runtime must validate retained failed-source authority");
    assert!(!page.accepts_numeric_seek(&session, 1_000, false));
    for policy in [HudPolicy::Lifted, HudPolicy::Hidden] {
        page.hud_policy = policy;
        assert!(!page.accepts_channel_number());
    }
    page.hud_policy = HudPolicy::Normal;
    page.scrub.ns = 0;
    assert!(!page.accepts_channel_number(), "a pending seek retains its input owner");
}

#[test]
fn media_arrow_override_and_accept_are_explicit_source_policy() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback();
    for seek in [false, true] {
        offer.seek_arrows = seek;
        for (sym, expected) in [(consts::SDLK_LEFT, if seek { Action::SeekBackward } else { Action::OpenMenu }),
            (consts::SDLK_RIGHT, if seek { Action::SeekForward } else { Action::ShowInfo })] {
            let (_, requests) = send(&mut bare(&session), &session, Some(&offer), UiMode::Classic,
                sym, 0, Edge::Down, false, None);
            assert_eq!(ott(&requests)[0].action, expected);
        }
    }
    for accept in [Action::OpenCatalog, Action::OpenGuide, Action::TogglePause] {
        offer.accept = accept;
        let (_, requests) = send(&mut bare(&session), &session, Some(&offer), UiMode::Modern,
            consts::SDLK_RETURN, 40, Edge::Down, false, None);
        assert_eq!(ott(&requests)[0].action, accept);
    }
}

#[test]
fn all_visible_hud_groups_keep_shared_arrows_and_ok_including_the_scrubber() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for focus in [player_hud::ELEM_SCRUB, player_hud::ELEM_CONTROL_BASE, player_hud::ELEM_TAB_BASE] {
        for sym in [consts::SDLK_UP, consts::SDLK_DOWN, consts::SDLK_LEFT,
            consts::SDLK_RIGHT, consts::SDLK_RETURN] {
            let mut page = PlayerScreen::new(ENTRY);
            page.hud.visible_at_press = true;
            let (handled, requests) = send(&mut page, &session, Some(&offer), UiMode::Modern,
                sym, 0, Edge::Down, false, Some(focus));
            assert!(ott(&requests).is_empty(), "visible {focus} must not use fullscreen action {sym}");
            if focus == player_hud::ELEM_SCRUB && [consts::SDLK_UP, consts::SDLK_DOWN].contains(&sym) {
                assert_eq!(handled, Handled::No, "shared engine owns scrubber-to-row/tab travel");
            }
        }
    }
}

#[test]
fn absent_or_stale_offer_never_turns_generic_player_input_into_ott_actions() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    let (_, replacement) = playback();
    assert_ne!(offer.source, replacement.source);
    for publication in [None, Some(&replacement)] {
        for (sym, raw) in [(consts::SDLK_UP, 82), (consts::SDLK_RETURN, 40), (0, 300), (0, 489)] {
            let (_, requests) = send(&mut bare(&session), &session, publication, UiMode::Classic,
                sym, raw, Edge::Down, false, None);
            assert!(ott(&requests).is_empty());
            if publication.is_some() {
                assert!(requests.is_empty(), "stale OTT input cannot fall back to transport on a replacement source");
            }
        }
    }
}

#[test]
fn repeat_uses_existing_edges_and_release_or_redelivery_cannot_duplicate_action() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for (sym, raw) in [(consts::SDLK_UP, 82), (0, 300)] {
        let mut page = bare(&session);
        for (edge, at_edge, count) in [(Edge::Down, false, 1), (Edge::Repeat, false, 1),
            (Edge::Repeat, true, 0), (Edge::Up, false, 0)] {
            let (handled, requests) = send(&mut page, &session, Some(&offer), UiMode::Classic,
                sym, raw, edge, at_edge, None);
            assert_eq!(handled, Handled::Yes); assert_eq!(requests.len(), count);
        }
    }
    let mut page = bare(&session);
    for edge in [Edge::Repeat, Edge::Up] {
        let (_, requests) = send(&mut page, &session, Some(&offer), UiMode::Modern,
            consts::SDLK_RETURN, 40, edge, false, None);
        assert!(requests.is_empty(), "OK only acts on its initial down edge");
    }
}

#[test]
fn covered_player_and_active_seek_preserve_their_existing_owner() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for policy in [HudPolicy::Lifted, HudPolicy::Hidden] {
        let mut page = bare(&session); page.hud_policy = policy;
        let (_, requests) = send(&mut page, &session, Some(&offer), UiMode::Modern,
            consts::SDLK_UP, 82, Edge::Down, false, None);
        assert!(ott(&requests).is_empty());
    }
    let mut page = bare(&session); page.scrub.ns = 10_000_000_000;
    assert!(page.scrub.is_live());
    let (_, requests) = send(&mut page, &session, Some(&offer), UiMode::Classic,
        consts::SDLK_RETURN, 40, Edge::Down, false, None);
    assert!(ott(&requests).is_empty());
}

#[test]
fn finite_guide_gate_and_unproven_buttons_do_not_manufacture_actions() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback(); offer.guide = false;
    for raw in [486, 487, 457, 403, 404, 405, 406, 427, 428, 33, 34] {
        let (_, requests) = send(&mut bare(&session), &session, Some(&offer), UiMode::Classic,
            0, raw, Edge::Down, false, None);
        assert!(ott(&requests).is_empty(), "{raw} has no admitted action");
    }
}

#[test]
fn relative_media_seek_keeps_the_existing_requested_target_burst() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback(); offer.seek_arrows = true;
    let mut page = bare(&session);
    let second = 1_000_000_000;
    page.seek_burst.jump(900, 40 * second, 10 * second, 100 * second);
    let before = page.seek_burst.snapshot();
    let (_, requests) = send(&mut page, &session, Some(&offer), UiMode::Modern,
        consts::SDLK_RIGHT, 79, Edge::Down, false, None);
    assert_eq!(ott(&requests)[0].action, Action::SeekForward);
    assert_eq!(page.seek_burst.snapshot(), before, "executor jump_by must retain its accumulated target");
    let (_, requests) = send(&mut page, &session, Some(&offer), UiMode::Modern,
        0, 300, Edge::Down, false, None);
    assert_eq!(ott(&requests)[0].action, Action::Next);
    assert!(page.seek_burst.snapshot().is_none(), "changing item ends the previous seek gesture");
}

#[test]
fn offer_uses_each_input_provenance_not_the_previous_event() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    let mut page = bare(&session);
    for (source, expected) in [(Source::RemoteFifo, true), (Source::Script, false),
        (Source::Sdl, true), (Source::Replay, false), (Source::RemoteFifo, true)] {
        let (_, requests) = send_source(&mut page, &session, Some(&offer), UiMode::Modern,
            0, 300, Edge::Down, false, None, source);
        assert_eq!(!ott(&requests).is_empty(), expected, "{source:?}");
    }
}

#[test]
fn ordered_prelude_samples_each_press_before_it_changes_visibility() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.dismissed = true;
        page.model.paused = false; // The previous draw is not this input's pause fact.
        let (_, first) = deliver(&mut page, &session, Some(&offer), mode,
            consts::SDLK_UP, 82, Edge::Down, false, None, Source::RemoteFifo, Some(true));
        assert_eq!(ott(&first).len(), 1);
        assert_eq!(ott(&first)[0].action, Action::Next);
        assert!(!page.hud.visible_at_press);
        assert!(!page.hud.dismissed);
        let (_, second) = deliver(&mut page, &session, Some(&offer), mode,
            consts::SDLK_RIGHT, 79, Edge::Down, false, Some(player_hud::ELEM_SCRUB),
            Source::RemoteFifo, Some(true));
        assert!(page.hud.visible_at_press, "second press sees the first press's revealed paused HUD");
        assert!(ott(&second).is_empty(), "a visible HUD keeps shared focus/transport handling");
    }
}

#[test]
fn ordered_prelude_applies_to_physical_generic_keys_without_an_ott_offer() {
    let _lock = plx_base::testlock::serial();
    let (session, _) = playback();
    let mut page = PlayerScreen::new(ENTRY);
    page.hud.dismissed = true;
    page.model.paused = false;
    let (_, requests) = deliver(&mut page, &session, None, UiMode::Modern,
        consts::SDLK_UP, 82, Edge::Down, false, None, Source::Sdl, Some(true));
    assert!(!page.hud.visible_at_press, "ingress did not pre-clear this generic key's dismissal");
    assert!(!page.hud.dismissed);
    assert!(ott(&requests).is_empty());
}

#[test]
fn releases_repeats_redelivery_and_unbound_colors_do_not_resample() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for (sym, raw, edge, at_edge) in [
        (consts::SDLK_UP, 82, Edge::Repeat, false),
        (consts::SDLK_UP, 82, Edge::Up, false),
        (consts::SDLK_UP, 82, Edge::Down, true),
        (0, 487, Edge::Down, false),
    ] {
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.dismissed = true;
        page.hud.exit_armed = true;
        page.hud.visible_at_press = false;
        deliver(&mut page, &session, Some(&offer), UiMode::Modern,
            sym, raw, edge, at_edge, None, Source::RemoteFifo, Some(true));
        assert!(page.hud.dismissed, "{sym}/{raw} {edge:?} must not clear dismissal");
        assert!(page.hud.exit_armed, "only a new bound press spends BACK arming");
        assert!(!page.hud.visible_at_press);
    }
}

#[test]
fn script_and_replay_keep_their_existing_ingress_sample() {
    let _lock = plx_base::testlock::serial();
    let (session, _) = playback();
    for source in [Source::Script, Source::Replay] {
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.dismissed = true;
        page.hud.exit_armed = true;
        deliver(&mut page, &session, None, UiMode::Modern,
            0, 300, Edge::Down, false, None, source, Some(true));
        assert!(page.hud.dismissed);
        assert!(page.hud.exit_armed);
    }
}

#[test]
fn native_blue_opens_categories_over_an_unmoved_reveal() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut page = bare(&session);
        page.scrub.begin(1_000, true);
        page.scrub.ns = 10_000_000_000;
        page.scrub.reveal = true;
        page.hud.visible_at_press = true;
        let before = (page.scrub.ns, page.scrub.reveal, page.scrub.drag, page.scrub.key);
        let (handled, requests) = send(&mut page, &session, Some(&offer), mode,
            0, 489, Edge::Down, false, Some(player_hud::ELEM_SCRUB));
        assert_eq!(ott(&requests).len(), 1, "native Blue must outlive a reveal-only preview");
        assert_eq!(handled, Handled::Yes);
        assert_eq!(requests.len(), 1, "opening categories must not commit or resume a seek");
        let request = ott(&requests)[0];
        assert_eq!(request.action, Action::OpenCategories);
        assert_eq!(request.owner, OWNER);
        assert_eq!(request.source, offer.source);
        assert_eq!((page.scrub.ns, page.scrub.reveal, page.scrub.drag, page.scrub.key), before,
            "request emission must not discard the preview before source admission");
        // The accepted panel uses the existing ownership transition to retire it.
        page.set_hud_policy(HudPolicy::Lifted);
        assert!(!page.scrub.is_live());
        assert_eq!(page.scrub.key.dir, 0);
        assert!(!page.scrub.reveal && !page.scrub.drag);
    }
}

#[test]
fn native_blue_preserves_moved_held_and_pointer_scrub_owners() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        // A moved target, an engaged hold, and a pointer drag keep the scrub owner.
        for (reveal, engaged, drag) in [(false, false, false), (true, true, false), (true, false, true)] {
            let mut page = bare(&session);
            page.scrub.begin(1_000, true);
            page.scrub.ns = 10_000_000_000;
            page.scrub.reveal = reveal;
            page.scrub.key.engaged = engaged;
            page.scrub.drag = drag;
            let before = (page.scrub.ns, page.scrub.reveal, page.scrub.drag, page.scrub.key);
            let (_, requests) = send(&mut page, &session, Some(&offer), mode,
                0, 489, Edge::Down, false, Some(player_hud::ELEM_SCRUB));
            assert!(requests.is_empty(), "active scrubbing must keep native Blue out of the catalog");
            assert_eq!((page.scrub.ns, page.scrub.reveal, page.scrub.drag, page.scrub.key), before);
        }
    }
}

#[test]
fn native_blue_does_not_promote_a_transport_tap_seed_to_catalog_navigation() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    struct Duration(i64);
    impl Drop for Duration {
        fn drop(&mut self) {
            plx_media::player::SHARED.duration_ns.store(self.0, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _duration = Duration(plx_media::player::SHARED.duration_ns.swap(
        100_000_000_000, std::sync::atomic::Ordering::Relaxed));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for key in [consts::Key::Left { alt: true }, consts::Key::Right { alt: true }] {
            let mut page = bare(&session);
            page.scrub.ns = 30_000_000_000;
            assert!(!page.scrub.reveal);
            let mut output = Vec::new();
            let mut present = plx_machine::present::Present::new();
            assert!(page.jump_key::<RemoteHost>(&session, key, Edge::Down, 1_000,
                &mut Effects::new(&mut output, MachineId::Instance(OWNER), &mut present)));
            assert!(output.iter().all(|item| !matches!(&item.fx, Fx::App(AppFx::Player(_)))));
            assert_eq!(page.scrub.ns, 30_000_000_000, "the tap retains the already moved target");
            assert!(page.transport_tap && page.scrub.reveal);
            assert!(!page.scrub.key.engaged && !page.scrub.drag);
            let (_, requests) = send(&mut page, &session, Some(&offer), mode,
                0, 489, Edge::Down, false, Some(player_hud::ELEM_SCRUB));
            assert!(requests.is_empty(), "a transport tap is not a reveal-only catalog affordance");
            // The ordinary non-transport key path cancels the interrupted tap without seeking.
            assert!(!page.transport_tap && !page.scrub.is_live());
        }
    }
}

#[path = "ott_info_cycle_tests.rs"]
mod info_cycle;

#[test]
fn failed_current_source_keeps_catalogue_and_channel_keys_but_never_seeks() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback();
    struct Restore(u8);
    impl Drop for Restore { fn drop(&mut self) { plx_media::player::restore_state_for_test(self.0); } }
    let _state = Restore(plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Error));
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.visible_at_press = true;
        let (_, requests) = send(&mut page, &session, Some(&offer), mode, consts::SDLK_RETURN, 40, Edge::Down, false, None);
        assert_eq!(ott(&requests).len(), 1);
        assert_eq!(ott(&requests)[0].action, Action::OpenCatalog);
        let (_, requests) = send(&mut page, &session, Some(&offer), mode, 0, consts::WCODE_CH_DOWN_KEY, Edge::Down, false, None);
        assert_eq!(ott(&requests)[0].action, Action::Previous);
        offer.seek_arrows = true;
        for sym in [consts::SDLK_LEFT, consts::SDLK_RIGHT] {
            let (_, requests) = send(&mut page, &session, Some(&offer), mode, sym, 0, Edge::Down, false, None);
            assert!(requests.is_empty(), "dead source cannot receive seek or play/pause");
        }
        offer.seek_arrows = false;
    }
}

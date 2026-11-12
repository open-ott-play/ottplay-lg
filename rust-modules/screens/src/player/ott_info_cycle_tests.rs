//! Original event phase and typed live-source qualification.
use super::*;

fn send_info_key(key: Key, page: &mut PlayerScreen, session: &plx_media::route::PlaybackSession,
    offer: &OttPlayerOffer, mode: UiMode, live: bool, archive: bool,
    edge: Edge, at_edge: bool) -> Vec<PlayerReq> {
    use plx_data::metadata::{NowPlaying, OttNowPlaying, OttLivePlaying, OttArchivePlaying};
    let mut store = plx_data::stores::metadata::MetadataStore::default();
    store.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(NowPlaying {
        ott: Some(OttNowPlaying { live: live.then(|| OttLivePlaying {
            channel_title: "Channel".into(), ordinal: None, guide: None }),
            archive: archive.then_some(OttArchivePlaying { start_s: 1, end_s: 2 }),
            collection_label: String::new(), confirmed_start_unix_ms: None, next: None, art: None }),
        is_episode: false, is_real_episode: false, title: "Programme".into(), ep_title: String::new(),
        season: 0, index: 0, summary: "Description".into(), year: 0, dur_ms: 0,
        rating: String::new(), thumb: String::new(), detail_rk: String::new(),
    })));
    let cx = Cx::<RemoteHost> { views: Views { session, metadata: store.view(), offer: Some(offer),
        mode, prelude: Some(false) }, tick: Tick { ms: 1_000, dt_us: 0 }, measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
    let event = ScreenEvent::<RemoteHost>::Input(InputEvent {
        kind: InputKind::Key { key, sym: if key == Key::Up { consts::SDLK_UP } else { consts::SDLK_RIGHT },
            wcode: if key == Key::Up { 82 } else { 79 }, edge, at_edge },
        at: cx.tick, source: Source::RemoteFifo });
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    page.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(OWNER), &mut present));
    output.into_iter().filter_map(|item| match item.fx {
        Fx::App(AppFx::Player(request)) => Some(request), _ => None }).collect()
}

fn send_info(page: &mut PlayerScreen, session: &plx_media::route::PlaybackSession,
    offer: &OttPlayerOffer, mode: UiMode, live: bool, archive: bool,
    edge: Edge, at_edge: bool) -> Vec<PlayerReq> {
    send_info_key(Key::Right, page, session, offer, mode, live, archive, edge, at_edge)
}

#[test]
fn live_up_reveals_then_expands_in_both_themes_without_channel_step() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut page = bare(&session);
        let first = send_info_key(Key::Up, &mut page, &session, &offer, mode,
            true, false, Edge::Down, false);
        assert_eq!(ott(&first)[0].action, Action::ShowInfo);
        page.hud.extend_default(1_000);
        let second = send_info_key(Key::Up, &mut page, &session, &offer, mode,
            true, false, Edge::Down, false);
        assert_eq!(ott(&second)[0].action, Action::ShowDescription);
        for (edge, at_edge) in [(Edge::Repeat, false), (Edge::Up, false), (Edge::Down, true)] {
            let requests = send_info_key(Key::Up, &mut page, &session, &offer, mode,
                true, false, edge, at_edge);
            assert!(ott(&requests).is_empty(), "holding Up must not advance the info cycle");
        }
    }
}

#[test]
fn finite_and_archive_up_keep_existing_channel_or_sequence_step() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (seek, live, archive) in [(true, false, false), (true, true, false),
            (false, false, false), (false, true, true)] {
            offer.seek_arrows = seek;
            let mut page = bare(&session);
            let requests = send_info_key(Key::Up, &mut page, &session, &offer, mode,
                live, archive, Edge::Down, false);
            assert_eq!(ott(&requests)[0].action, Action::Next);
        }
    }
}

#[test]
fn classic_live_info_phase_is_captured_before_later_hud_changes() {
    let _lock = plx_base::testlock::serial();
    let (session, offer) = playback();
    let mut page = bare(&session);
    let first = send_info(&mut page, &session, &offer, UiMode::Classic, true, false, Edge::Down, false);
    page.hud.extend_default(1_000);
    let second = send_info(&mut page, &session, &offer, UiMode::Classic, true, false, Edge::Down, false);
    assert_eq!(ott(&first)[0].action, Action::ShowInfo);
    assert_eq!(ott(&second)[0].action, Action::ShowDescription);
    page.hud.hide(false);
    assert_eq!(ott(&first)[0].action, Action::ShowInfo,
        "later HUD state cannot rewrite an already queued phase");
    for (edge, at_edge) in [(Edge::Repeat, false), (Edge::Up, false), (Edge::Down, true)] {
        let requests = send_info(&mut page, &session, &offer, UiMode::Classic, true, false, edge, at_edge);
        assert!(ott(&requests).is_empty(), "repeat, release and propagated edge are not a new phase");
    }
}

#[test]
fn modern_finite_and_archive_visible_right_keep_existing_focus_ownership() {
    let _lock = plx_base::testlock::serial();
    let (session, mut offer) = playback();
    for (mode, seek, live, archive) in [(UiMode::Modern, false, true, false),
        (UiMode::Classic, true, true, false), (UiMode::Classic, false, false, false),
        (UiMode::Classic, false, false, true)] {
        offer.seek_arrows = seek;
        let mut page = bare(&session);
        page.hud.extend_default(1_000);
        let requests = send_info(&mut page, &session, &offer, mode, live, archive, Edge::Down, false);
        assert!(ott(&requests).is_empty());
    }
}

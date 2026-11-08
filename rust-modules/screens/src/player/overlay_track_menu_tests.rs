//! Exercises the real More and Tracks screen reducers; no player, network or storage fixture.
use super::*;

// This More fixture supplies the retained request, installed Plex identity and matching inventory.
// Production publishes the request before resolve and retains it across apply_plan.
// Subsequent Track requests still use the parent's isolated idle-screen fixture.
struct InventoryHost;
impl Host for InventoryHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = (&'a plx_media::route::PlaybackSession, plx_data::metadata::MetadataView<'a>);
    type Init = plx_ui::fixture::FixtureArg;
    type Memory = PageMemory;
}
impl crate::registry::PlayerLike for InventoryHost {
    fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.0 }
}
impl crate::registry::MetadataLike for InventoryHost {
    fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.1 }
}
fn enter_tracks(tab: i32) -> PlayerOverlayScreen {
    use plx_data::{metadata::{PlayingItem, Stream}, stores::metadata::{MetadataCmd, MetadataStore}};
    let mut ps = plx_media::route::PlaybackSession::IDLE;
    plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
    plx_media::route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
    let mut meta = MetadataStore::default();
    let mut item = PlayingItem::with_subs(vec![Stream { id: 3, index: 0, codec: "srt".into(), ..Default::default() }]);
    item.audio = vec![Stream { id: 1, index: 0, codec: "aac".into(), ..Default::default() },
        Stream { id: 2, index: 1, codec: "aac".into(), ..Default::default() }];
    meta.run(MetadataCmd::InstallPlaying(Some(item)));
    let mut more = PlayerOverlayScreen::new(&ps, meta.view(), ENTRY, OverlayKind::More { page: None });
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let cx = Cx::<InventoryHost> { views: (&ps, meta.view()), tick: Tick::default(), measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
    more.step(&ScreenEvent::Activate(if tab == 0 { 8 } else { 9 }), &cx,
        &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present));
    assert!(out.iter().any(|e| matches!(e.fx, Fx::Nav(NavOp::Dismiss(id)) if id == ENTRY)),
        "More must close when transferring to a track menu");
    let reqs: Vec<_> = out.into_iter().filter_map(|e| match e.fx {
        Fx::App(AppFx::Player(req)) => Some(req), _ => None,
    }).collect();
    let destinations: Vec<_> = reqs.iter().filter_map(|req| match req {
        PlayerReq::OpenOverlay(kind) => Some(*kind), _ => None,
    }).collect();
    assert_eq!(destinations, [OverlayKind::Tracks { tab }]);
    assert!(!reqs.iter().any(|req| matches!(req, PlayerReq::More(_) | PlayerReq::Exit
        | PlayerReq::Transport(_) | PlayerReq::CommitTrackAddressed(_))),
        "opening a panel neither changes transport nor commits a track");
    PlayerOverlayScreen::new(&plx_media::route::PlaybackSession::IDLE, MetadataStore::default().view(), ENTRY, destinations[0])
}

#[test]
fn more_transfers_to_exact_audio_and_subtitle_tabs() {
    let _lock = plx_base::testlock::serial();
    for tab in [0, 1] {
        let page = enter_tracks(tab);
        assert_eq!(page.tracks_probe().map(|value| value.0), Some(tab));
    }
}

#[test]
fn subtitle_off_reached_from_more_still_waits_for_addressed_admission() {
    let _lock = plx_base::testlock::serial();
    let mut page = enter_tracks(1);
    let (_, reqs, dismissed) = open_row(&mut page, TrackRow::SubOff);
    assert!(!dismissed, "requesting Off is not yet an accepted track change");
    assert_eq!(track_requests(&reqs).len(), 1);
    let (_, duplicate, dismissed) = open_row(&mut page, TrackRow::SubOff);
    assert!(track_requests(&duplicate).is_empty()); assert!(!dismissed);
    assert!(acknowledge(&mut page, &reqs, plx_appkit::track_menu::TrackCommitOutcome::Admitted));
}

#[test]
fn subtitle_style_reached_from_more_keeps_its_existing_acknowledged_flow() {
    let _lock = plx_base::testlock::serial();
    struct RestoreTone(plx_plex::plex::session::SubtitleTone);
    impl Drop for RestoreTone {
        fn drop(&mut self) { plx_media::player::restore_subtitle_tone(self.0); }
    }
    let _restore = RestoreTone(plx_media::player::subtitle_tone());
    plx_media::player::restore_subtitle_tone(plx_plex::plex::session::SubtitleTone::White);
    let mut page = enter_tracks(1);
    let (_, reqs, dismissed) = open_row(&mut page, TrackRow::Style);
    assert!(track_requests(&reqs).is_empty()); assert!(!dismissed);
    assert_eq!(tracks_path(&page), [TrackPage::Style]);
    open_row(&mut page, TrackRow::OpenField(StyleField::Color));
    let (_, reqs, dismissed) = open_row(&mut page, TrackRow::Choice(StyleField::Color, 1));
    assert!(!dismissed);
    assert_eq!(track_requests(&reqs), [plx_appkit::track_menu::TrackCommit::SubtitleTone(
        plx_plex::plex::session::SubtitleTone::Silver)]);
    assert!(!acknowledge(&mut page, &reqs, plx_appkit::track_menu::TrackCommitOutcome::Saved));
    assert_eq!(tracks_path(&page), [TrackPage::Style, TrackPage::Picker(StyleField::Color)]);
}

#[test]
fn more_actual_ingress_uses_only_the_current_inventory() {
    use plx_data::{metadata::{PlayingItem, Stream}, stores::metadata::{MetadataCmd, MetadataStore}};
    let _lock = plx_base::testlock::serial();
    for (audio_count, subtitle_count, key, expected) in [
        (0, 0, 8, false), (0, 0, 9, false), (1, 0, 8, false),
        (2, 0, 8, true), (1, 1, 9, true),
    ] {
        let mut ps = plx_media::route::PlaybackSession::IDLE;
        plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
        plx_media::route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
        let mut meta = MetadataStore::default();
        let mut item = PlayingItem::with_subs((0..subtitle_count).map(|i| Stream {
            id: i + 10, index: i, codec: "srt".into(), ..Default::default()
        }).collect());
        item.audio = (0..audio_count).map(|i| Stream {
            id: i + 1, index: i, codec: "aac".into(), ..Default::default()
        }).collect();
        meta.run(MetadataCmd::InstallPlaying(Some(item)));
        let mut more = PlayerOverlayScreen::new(&ps, meta.view(), ENTRY, OverlayKind::More { page: None });
        let cx = Cx::<InventoryHost> { views: (&ps, meta.view()), tick: Tick::default(), measure: &FixtureMeasure,
            focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        more.step(&ScreenEvent::Activate(key), &cx,
            &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present));
        let opened = out.iter().filter_map(|e| match &e.fx {
            Fx::App(AppFx::Player(PlayerReq::OpenOverlay(kind))) => Some(*kind), _ => None,
        }).collect::<Vec<_>>();
        let expected_kinds = if expected { vec![OverlayKind::Tracks { tab: if key == 8 { 0 } else { 1 } }] } else { vec![] };
        assert_eq!(opened, expected_kinds, "audio={audio_count}, subtitles={subtitle_count}, key={key}");
        assert_eq!(out.iter().any(|e| matches!(e.fx, Fx::Nav(NavOp::Dismiss(id)) if id == ENTRY)), expected);
    }
}

#[test]
fn an_open_more_rejects_retired_track_keys_before_the_next_tick() {
    use plx_data::{metadata::{PlayingItem, Stream}, stores::metadata::{MetadataCmd, MetadataStore}};
    let _lock = plx_base::testlock::serial();
    #[derive(Clone, Copy, Debug)]
    enum Change { Inventory, Source, Request }
    for change in [Change::Inventory, Change::Source, Change::Request] {
        for key in [8, 9] {
            let mut ps = plx_media::route::PlaybackSession::IDLE;
            plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
            plx_media::route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "rk");
            let mut meta = MetadataStore::default();
            let mut item = PlayingItem::with_subs(vec![Stream { id: 3, index: 0, codec: "srt".into(), ..Default::default() }]);
            item.audio = vec![Stream { id: 1, index: 0, codec: "aac".into(), ..Default::default() },
                Stream { id: 2, index: 1, codec: "aac".into(), ..Default::default() }];
            meta.run(MetadataCmd::InstallPlaying(Some(item)));
            let mut more = PlayerOverlayScreen::new(&ps, meta.view(), ENTRY, OverlayKind::More { page: None });
            let Panel::More(menu) = more.panel() else { unreachable!() };
            assert!(menu.keys().contains(&key), "the old key really existed");
            match change {
                Change::Inventory => { meta.run(MetadataCmd::InstallPlaying(None)); }
                Change::Source => {
                    plx_media::route::install_plex_playback_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "replacement");
                    plx_media::route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "replacement");
                }
                Change::Request => {
                    let installed = plx_media::route::playback_identity(&ps);
                    plx_media::route::install_plex_request_for_test(&mut ps, plx_plex::plex::ServerId::from_raw(0), "replacement");
                    assert_eq!(plx_media::route::playback_identity(&ps), installed);
                    assert_eq!(plx_media::route::cur_rk(&ps), "rk", "the new request has not landed");
                }
            }
            let cx = Cx::<InventoryHost> { views: (&ps, meta.view()), tick: Tick::default(), measure: &FixtureMeasure,
                focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(ENTRY) };
            let mut out = Vec::new();
            let mut present = plx_machine::present::Present::new();
            more.step(&ScreenEvent::Activate(key), &cx,
                &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present));
            assert!(!out.iter().any(|e| matches!(e.fx, Fx::App(AppFx::Player(_)) | Fx::Nav(NavOp::Dismiss(_)))),
                "{change:?}: a stale key cannot open a track panel or activate/dismiss its neighbour");
        }
    }
}

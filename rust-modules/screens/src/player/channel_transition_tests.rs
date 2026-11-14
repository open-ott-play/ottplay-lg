//! Source changes reveal information without rewriting playback or metadata authority.
use super::*;
use plx_ui::widgets::StatusKind;

fn working() -> player_hud::Busy { player_hud::Busy::Readout(StatusKind::Working, c"Resolving") }

#[test]
fn accepted_channel_change_reveals_footer_and_clears_stale_numeric_notice() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.hud.hide(false);
    page.set_number_notice(&Some(("17".into(), "Old channel".into())), 900);
    page.begin_channel_change(1_000);
    assert!(!page.hud.dismissed);
    assert!(page.hud.until >= 6_000);
    assert!(!page.number_notice);
    assert!(page.channel_change_hud);
}

#[test]
fn slow_channel_acquisition_keeps_information_then_normal_linger() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.begin_channel_change(1_000);
    page.set_playback_feedback(working(), 7_000);
    assert!(page.hud.until >= 12_000);
    page.transport_reset(false);
    assert_eq!(page.hud.until, 0);
    page.set_playback_feedback(player_hud::Busy::Transport, 14_000);
    assert!(page.hud.until >= 19_000);
    page.set_playback_feedback(player_hud::Busy::None, 20_000);
    assert!(!page.channel_change_hud);
    assert!(page.hud.until >= 25_000);
    let until = page.hud.until;
    page.set_playback_feedback(player_hud::Busy::None, 30_000);
    assert_eq!(page.hud.until, until, "ordinary playback must auto-hide again");
}

#[test]
fn deliberately_dismissed_transition_footer_is_not_reopened_by_feedback() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.begin_channel_change(1_000);
    page.hud.hide(true);
    let until = page.hud.until;
    for (busy, now) in [(working(), 8_000), (player_hud::Busy::Transport, 12_000), (player_hud::Busy::None, 14_000)] {
        page.set_playback_feedback(busy, now);
        assert!(page.hud.dismissed);
        assert_eq!(page.hud.until, until);
    }
}

#[test]
fn terminal_failure_finishes_transition_hold_without_hiding_error() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.begin_channel_change(1_000);
    let failed = player_hud::Busy::Readout(StatusKind::Failed, c"Playback failed");
    page.set_playback_feedback(failed, 9_000);
    assert_eq!(page.busy, failed);
    assert!(!page.channel_change_hud);
    let until = page.hud.until;
    page.set_playback_feedback(failed, 20_000);
    assert_eq!(page.hud.until, until);
}

#[test]
fn unrelated_rebuffer_does_not_extend_a_hidden_or_expired_footer() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.hud.until = 100;
    page.set_playback_feedback(player_hud::Busy::Transport, 10_000);
    assert_eq!(page.hud.until, 100);
}

#[test]
fn channel_footer_replaces_outgoing_facts_when_target_is_published() {
    use plx_data::metadata::{NowPlaying, OttLivePlaying, OttNowPlaying};
    use plx_data::stores::metadata::{MetadataCmd, MetadataStore};
    let _lock = plx_base::testlock::serial();
    let now = |title: &str, ordinal| NowPlaying {
        title: title.into(), ep_title: String::new(), is_episode: false, is_real_episode: false,
        season: 0, index: 0, summary: String::new(), year: 0, dur_ms: 0,
        rating: String::new(), thumb: String::new(), detail_rk: String::new(),
        ott: Some(OttNowPlaying { live: Some(OttLivePlaying { channel_title: title.into(), ordinal: Some(ordinal), guide: None }),
            archive: None, collection_label: title.into(), confirmed_start_unix_ms: None, next: None, art: None }),
    };
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut page = PlayerScreen::new(EntryId(1)); page.transport = true;
        let mut store = MetadataStore::default();
        store.run(MetadataCmd::SetNowPlaying(Some(now("Outgoing channel", 7))));
        page.begin_channel_change(1_000);
        page.set_playback_feedback(working(), 1_000);
        let before = plx_ui::draw_census::capture_runs(|| plx_ui::record_walk(||
            page.draw_hud_in_mode(&plx_media::route::PlaybackSession::IDLE, 1_000,
                &plx_ui::fixture::FixtureMeasure, store.view(), mode)));
        assert!(before.iter().any(|r| r.text == "Outgoing channel"), "{mode:?}: {before:?}");
        store.run(MetadataCmd::SetNowPlaying(Some(now("Admitted target", 8))));
        page.set_playback_feedback(player_hud::Busy::None, 8_000);
        let after = plx_ui::draw_census::capture_runs(|| plx_ui::record_walk(||
            page.draw_hud_in_mode(&plx_media::route::PlaybackSession::IDLE, 8_000,
                &plx_ui::fixture::FixtureMeasure, store.view(), mode)));
        assert!(after.iter().any(|r| r.text == "Admitted target"), "{mode:?}: {after:?}");
        assert!(!after.iter().any(|r| r.text == "Outgoing channel"));
    }
}


#[test]
fn restored_first_load_reveals_footer_without_a_channel_key_and_then_auto_hides() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    assert!(!page.channel_change_hud);
    page.set_playback_feedback(working(), 10_000);
    assert!(page.channel_change_hud);
    assert!(page.hud.until >= 15_000);
    assert!(page.hud_drawn(&plx_media::route::PlaybackSession::IDLE, 10_000));
    page.set_playback_feedback(working(), 20_000);
    assert!(page.hud.until >= 25_000, "a slow initial load cannot lose its information");
    page.set_playback_feedback(player_hud::Busy::None, 30_000);
    assert!(!page.channel_change_hud);
    let until = page.hud.until;
    assert!(until >= 35_000);
    page.set_playback_feedback(player_hud::Busy::None, 40_000);
    assert_eq!(page.hud.until, until);
    assert!(!page.hud_drawn(&plx_media::route::PlaybackSession::IDLE, 40_000),
        "completed loading must not permanently pin the footer");
}

#[test]
fn dismissed_initial_load_and_failed_load_do_not_reopen_or_permanently_pin_footer() {
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    page.hud.hide(false);
    page.set_playback_feedback(working(), 10_000);
    assert!(!page.channel_change_hud);
    assert!(page.hud.dismissed);
    page.begin_channel_change(20_000);
    page.set_playback_feedback(working(), 25_000);
    page.set_playback_feedback(player_hud::Busy::Readout(StatusKind::Failed, c"Playback failed"), 30_000);
    assert!(!page.channel_change_hud);
    let until = page.hud.until;
    page.set_playback_feedback(player_hud::Busy::Readout(StatusKind::Failed, c"Playback failed"), 40_000);
    assert_eq!(page.hud.until, until);
}


#[test]
fn pending_target_footer_does_not_depend_on_outgoing_decoder_loading_or_timer() {
    use plx_data::metadata::NowPlaying;
    let _lock = plx_base::testlock::serial();
    let mut page = PlayerScreen::new(EntryId(1));
    let ps = &plx_media::route::PlaybackSession::IDLE;
    let was_paused = plx_media::player::TX.paused.swap(false, std::sync::atomic::Ordering::Relaxed);
    page.hud.until = 1;
    assert!(!page.hud_up(ps, 10_000));
    page.pending_channel_display = Some(NowPlaying {
        title: "Accepted target".into(), summary: String::new(), ott: None,
        is_episode: false, is_real_episode: false, ep_title: String::new(),
        season: 0, index: 0, year: 0, dur_ms: 0, rating: String::new(),
        thumb: String::new(), detail_rk: String::new(),
    });
    assert!(page.hud_up(ps, 10_000), "pending information cannot rely on the old decoder");
    assert!(page.hud_drawn(ps, 10_000));
    page.set_hud_policy(HudPolicy::Hidden);
    assert!(!page.hud_up(ps, 10_000), "a surface hiding transport remains authoritative");
    page.set_hud_policy(HudPolicy::Normal);
    assert!(!page.hud_up(ps, 10_000), "closing that surface preserves explicit dismissal");
    page.hud.dismissed = false;
    assert!(page.hud_up(ps, 10_000));
    page.hud.hide(false);
    assert!(!page.hud_up(ps, 10_000));
    page.hud.dismissed = false;
    page.pending_channel_display = None;
    assert!(!page.hud_up(ps, 10_000), "settled, failed or cancelled target cannot pin the footer");
    assert_eq!(page.hud.until, 1, "the pin never rewrites the ordinary auto-hide timer");
    plx_media::player::TX.paused.store(was_paused, std::sync::atomic::Ordering::Relaxed);
}

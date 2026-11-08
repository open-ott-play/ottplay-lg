//! Capability-driven alternatives to the existing player tabs and Info actions.
use super::*;

fn offered(action: PlaybackMenuAction) -> PlaybackMenuCapabilities {
    PlaybackMenuCapabilities {
        chapters: action == PlaybackMenuAction::Chapters,
        from_beginning: action == PlaybackMenuAction::FromBeginning,
        go_to_detail: action == PlaybackMenuAction::GoToDetail,
        ..Default::default()
    }
}

#[test]
fn accessibility_rows_are_independent_keyed_offers_and_retire_without_retargeting() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut menu = MoreMenuState::new(&ps);
    let alternatives = [PlaybackMenuAction::Chapters, PlaybackMenuAction::FromBeginning, PlaybackMenuAction::GoToDetail];
    assert_eq!(alternatives.map(|action| action.key()), [46, 47, 48]);
    let mut fingerprints = std::collections::BTreeSet::new();
    for action in alternatives {
        menu.set_playback_capabilities(&ps, offered(action));
        for candidate in alternatives {
            assert_eq!(menu.focus_key(candidate.key()), candidate == action);
        }
        assert!(menu.focus_key(action.key()));
        assert_eq!(menu.on_ok(&ps), MoreOk::Action(Action::Playback(action)));
        let mut canon = plx_machine::machine::Canon::new();
        menu.canon(&mut canon);
        assert!(fingerprints.insert(canon.finish()));
        assert!(menu.set_playback_capabilities(&ps, PlaybackMenuCapabilities::default()));
        assert!(!menu.focus_key(action.key()), "retired key must not identify a neighbouring row");
    }
    assert_eq!(&PlaybackMenuAction::ALL[..6], &[PlaybackMenuAction::Categories,
        PlaybackMenuAction::Guide, PlaybackMenuAction::Records, PlaybackMenuAction::TogglePause,
        PlaybackMenuAction::ReloadStream, PlaybackMenuAction::ReturnLive]);
}

#[test]
fn accessibility_rows_fit_the_shared_form_in_every_shipped_language() {
    let _lock = plx_base::testlock::serial();
    use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut out = Vec::new();
    for language in SHIPPED {
        let _language = language_on_this_thread_for_test(language);
        let mut form = FormTable::new(plx_ui::table_screen::BAND_BASE);
        form.table.compact = true;
        let rows = [PlaybackMenuAction::Chapters, PlaybackMenuAction::FromBeginning,
            PlaybackMenuAction::GoToDetail].map(Action::Playback);
        form.set_or_open(root_form_for(&ps, &rows, true, plx_media::route::Quality::Original,
            plx_media::player::RATE_NORMAL, &[], Default::default(), None), None);
        out.extend(form.table.menu_cap_failure(&plx_base::fontcov::advances::ShippedMeasure, language.tag()));
        out.extend(form.table.app_fit_failures(plx_ui::table::MENU_MAX_W, language.tag()));
        out.extend(form.table.app_fit_failures_hugged(language.tag()));
    }
    plx_ui::table::assert_no_fit_failures(&out);
}

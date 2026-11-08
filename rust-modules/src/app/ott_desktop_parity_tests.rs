//! Actual physical SDL ingress. The parent fixture holds all write/resolver jobs.
use super::*;

#[test]
fn physical_vertical_arrows_follow_the_remote_list_direction() {
    let _serial = plx_base::testlock::serial();
    for (sym, expected) in [(SDLK_UP, "second"), (SDLK_DOWN, "third")] {
        let _reset = Reset;
        let mut app = fixture();
        app.ott.as_mut().unwrap().selected = Some("third".into());
        let identity = plx_media::route::playback_identity(&app.player.session);
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let owner = app.pages.top_page();
        ingest(&mut app, physical(sym, 0, true, false));
        let epoch = app.ott.as_ref().unwrap().playback_epoch;
        ingest(&mut app, physical(sym, 0, true, true));
        ingest(&mut app, physical(sym, 0, false, false));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target.as_ref().map(|r| r.id.as_str()), Some(expected));
        assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
        assert_eq!(runtime.playback_epoch, epoch, "repeat/release cannot navigate again");
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(app.pages.top_page(), owner);
        assert!(runtime.resolver_busy && runtime.resolver_next.is_some());
        assert!(app.inputs.is_empty());
        assert_eq!(app.down_sym, 0);
    }
}

#[test]
fn combined_or_shifted_seek_modifiers_keep_the_page_input_owner() {
    let _serial = plx_base::testlock::serial();
    for modifiers in [0x0041, 0x0101, 0x0401, 0x0140, 0x0440, 0x0500] {
        let _reset = Reset;
        let mut app = fixture();
        tap(&mut app, SDLK_RIGHT, modifiers);
        assert_eq!(plx_media::player::seek_pending(), -1, "modifiers={modifiers:#x}");
        assert_eq!(app.player.volume.desired().level(), 50);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        assert_eq!(app.inputs.len(), 2, "unclaimed modifiers keep both page edges");
    }
}

//! Saved completion cannot revoke a newer native route or restart USB restore.
use super::*;

struct NativeReset;
impl Drop for NativeReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

#[test]
fn saved_usb_relocation_cannot_stop_new_native_route_or_requeue_restore() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let mut r = runtime();
    let mut a = app();
    let saved = stage(&mut r, &directory);
    arm_writer(&mut r);
    let epoch = r.playback_epoch;
    let generation = r.generation;
    let old_ticket = r.local_inventory.usb.ticket.expect("automatic restore ticket");
    assert!(plx_media::route::play_request_is_current(old_ticket));
    assert!(r.catalog_next.is_none());
    assert!(r.local_inventory.usb.pending.is_none());

    // Use the existing synchronous external-route fixture: no network request,
    // file scan, decoder or fabricated PlaybackIdentity is involved. This is a
    // native-only route replacement, deliberately keeping the OTT epoch intact.
    a.ott = Some(r);
    crate::app::ott_runtime::tests::local_integration::install(&mut a, None, "new-native-route");
    let mut r = a.ott.take().unwrap();
    // This ordinary route has no accepted temporary Local/Plex queue owner.
    r.playing.as_mut().unwrap().transient_owner = None;
    let identity = plx_media::route::playback_identity(&a.player.session)
        .expect("the replacement really installed a native route");
    let source = plx_media::route::external_source(&a.player.session)
        .expect("replacement external source");
    let ticket = plx_media::route::play_request_ticket();
    assert_eq!(r.playback_epoch, epoch);
    assert!(!plx_media::route::play_request_is_current(old_ticket));
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(playing_matches(r.playing.as_ref().unwrap(), &a));
    assert!(!r.usb_save_ready(&saved));
    let browse = (r.page, r.provider, r.profile_focus.clone());
    let revision = r.catalog_revision;
    let app = deliver(a, r, Ok(saved));
    let r = app.ott.as_ref().unwrap();

    assert!(plx_media::route::playback_identity_matches(&app.player.session, &identity));
    assert!(plx_media::route::external_source(&app.player.session) == Some(source));
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(r.playing.as_ref().is_some_and(|playing|
        playing.selection_id == "new-native-route" && playing_matches(playing, &app)));
    assert_eq!(r.playback_epoch, epoch);
    assert_eq!(r.generation, generation);
    assert_eq!((r.page, r.provider, r.profile_focus.clone()), browse);
    assert_eq!(r.catalog_revision, revision, "stale save must not refresh the catalogue");
    assert!(r.local_inventory.sources.is_empty(), "stale save must not install a USB grant");
    assert!(r.local_inventory.usb.install.is_none());
    assert!(r.local_inventory.usb.pending.is_none(), "stale save must not enqueue automatic restore again");
    assert!(r.catalog_next.is_none(), "generic reload must not leave another catalogue attempt");
    assert!(r.play_target.is_none());
    assert!(!r.catalog_done.load(Ordering::Acquire), "stale save must retain the occupied catalogue slot");
}

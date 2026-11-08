use super::*;
use plx_data::ott::favorites::Origin;
fn intent() -> Intent {
    Intent {
        attachment: None,
        armed: Stored::armed(
            Origin {
                provider: "m3u".into(),
                profile: "Home".into(),
                subprofile: None,
                source_alias: "Home".into(),
            },
            true,
        )
        .unwrap(),
        play_generation: 7,
        playback_epoch: 11,
        navigation_epoch: 3,
        selection_id: "selected".into(),
        source_fingerprint: "fixture-source".into(),
    }
}
#[test]
fn observed_choice_must_match_every_existing_intent_fence() {
    let i = intent();
    assert!(matches_intent(
        &i,
        &i.armed,
        7,
        11,
        3,
        "selected",
        "fixture-source"
    ));
    for (g, p, n, id, source) in [
        (8, 11, 3, "selected", "fixture-source"),
        (7, 12, 3, "selected", "fixture-source"),
        (7, 11, 4, "selected", "fixture-source"),
        (7, 11, 3, "previous", "fixture-source"),
        (7, 11, 3, "selected", "replacement"),
    ] {
        assert!(!matches_intent(&i, &i.armed, g, p, n, id, source));
    }
    assert!(matches_intent(
        &i,
        &i.armed.with_strict(false),
        7,
        11,
        3,
        "selected",
        "fixture-source"
    ));
}
#[test]
fn cancellation_never_discards_an_already_dispatched_capture() {
    let i = intent();
    let mut capture = Capture {
        intent: Some(i.clone()),
        writing: Some(Pending { target: i.armed }),
    };
    capture.cancel_unwritten();
    assert!(capture.intent.is_none());
    assert!(capture.pending());
}
#[test]
fn idle_capture_has_no_implicit_old_playback_claim() {
    let capture = Capture::default();
    assert!(capture.intent.is_none());
    assert!(!capture.pending());
}

#[test]
fn attached_explicit_choice_arms_parent_capture_with_exact_current_receipt() {
    use crate::app::ott_m3u_attachment::SETTINGS_KEY;
    use plx_data::ott::favorites::Locator;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    let parent = ProviderProfile {
        id: "home".into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: "https://tv.fixture.invalid/list.m3u".into(), source_alias: "Home".into(), enabled: true,
    };
    runtime.saved.profiles.profiles = vec![parent.clone()];
    runtime.saved.profiles.active_profile = Some(parent.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.saved = runtime.attachment_candidate(&parent,
        "portal::[key:fixture]https://portal.fixture.invalid/api/", Some("attached")).unwrap();
    runtime.observe_m3u_attachments();
    let favorite = Favorite {
        title: "Attached film".into(),
        origin: Origin { provider: "m3u".into(), profile: "Home".into(), source_alias: "Home".into(),
            subprofile: Some("Films/Family".into()) },
        locator: Locator::VPortal { id_field: "id".into(), id: "42".into() },
    };
    let receipt = runtime.attachment_for(&favorite).unwrap().unwrap();
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let armed = Stored::armed(Origin { subprofile: None, ..favorite.origin.clone() }, true).unwrap();
    runtime.saved.settings.insert(stored::KEY.into(), armed.encode().unwrap());
    let request = PlaybackRequest::ordinary(&format!("favorite:{}", favorite.identity()), 0);
    let request = runtime.prepare_play_request(request).unwrap().0;
    runtime.play_target = Some(request.clone());
    runtime.play_attachment = Some(receipt.clone());
    runtime.play_generation = 7;
    runtime.playback_epoch = 11;
    runtime.navigation_epoch = 3;
    explicit_admitted(&mut runtime);
    let intent = runtime.kiosk_capture.intent.as_ref().expect("M3U-parent Arm accepts attached choice");
    assert_eq!(intent.armed, armed);
    assert_eq!(intent.selection_id, request.id);
    assert_eq!((intent.play_generation, intent.playback_epoch, intent.navigation_epoch), (7, 11, 3));
    assert!(intent.attachment.as_ref() == Some(&receipt));
    assert!(!runtime.kiosk_capture.pending(), "resolution is not successful Playing capture");
    assert!(!runtime.saving);
    // Parent scope still matters even though both origins can project VPortal.
    runtime.saved.settings.insert(stored::KEY.into(), Stored::armed(Origin {
        provider: "vportal".into(), profile: "Home".into(), source_alias: "Home".into(), subprofile: None,
    }, true).unwrap().encode().unwrap());
    explicit_admitted(&mut runtime);
    assert!(runtime.kiosk_capture.intent.is_none());
    runtime.saved.settings.insert(stored::KEY.into(), armed.encode().unwrap());
    // Retirement of the actual attachment cancels the pending request. Merely
    // reusing its selection ID cannot restore the stale receipt or old intent.
    runtime.saved.settings.remove(SETTINGS_KEY);
    runtime.observe_m3u_attachments();
    explicit_admitted(&mut runtime);
    assert!(runtime.kiosk_capture.intent.is_none());
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

use super::*;
use plx_data::ott::favorites::{Locator, Origin};

struct Reset;
impl Drop for Reset { fn drop(&mut self) { plex::reset_servers_for_test(); } }
fn setup() -> (ProviderProfile, Favorite, &'static plex::Client, Reset) {
    plex::reset_servers_for_test();
    let origin = plex::Origin::parse("https://mv-plex.invalid:32400").unwrap();
    let sid = plex::register_pinned_with_client_id("mv-plex-server", &origin,
        "fixture-private-token", None, "mv-fixture", Default::default());
    let client = plex::client_for(sid).unwrap();
    let profile = ProviderProfile { id: "plex-fixture".into(), name: "Plex fixture".into(),
        kind: ProviderKind::Plex, source: "mv-plex-server".into(), source_alias: "Library".into(), enabled: true };
    let favorite = Favorite { title: "Film".into(), origin: Origin { provider: "plex".into(),
        profile: profile.name.clone(), subprofile: None, source_alias: profile.source_alias.clone() },
        locator: Locator::Plex { server_id: profile.source.clone(), rating_key: "77".into() } };
    (profile, favorite, client, Reset)
}
fn metadata() -> Metadata {
    serde_json::from_value(json!({"ratingKey":"77","type":"movie","title":"Film",
        "librarySectionTitle":"Documentary","summary":"Exact server description", "duration":9000,
        "Media":[{"id":1,"Part":[{"id":9,"key":"/library/parts/9/123/file.mkv"}]}]})).unwrap()
}
fn capture(profile: &ProviderProfile, favorite: &Favorite) -> Capture {
    Capture::capture(profile, favorite, "mv-fixture-1".into()).unwrap()
}

#[test]
fn captured_explicit_server_prepares_exact_direct_file_without_native_or_transcode_session() {
    let _serial = plx_base::testlock::serial();
    let (profile, favorite, _, _reset) = setup();
    let current = plex::current_server();
    let plan = capture(&profile, &favorite).prepare_with(&plx_base::task::OffFrame::for_test(),
        &AtomicBool::new(false), |_, key| { assert_eq!(key, "77"); Some(metadata()) }).unwrap();
    let (entry, mut witness) = plan.into_parts().unwrap();
    assert_eq!(entry.category, "Documentary");
    assert_eq!(entry.description, "Exact server description");
    assert_eq!(entry.duration_ms, Some(9000));
    assert!(witness.current());
    // No public URL accessor exists. Even corrupting the private retained target is detected.
    witness.expected_url.push_str("&different=1");
    assert!(!witness.current());
    assert_eq!(plex::current_server(), current);
    assert!(matches!(entry.media, MediaLocator::Plex { ref server_id, ref rating_key }
        if server_id == "mv-plex-server" && rating_key == "77"));
}

#[test]
fn token_replacement_regrant_and_cancel_refuse_old_prepared_sources() {
    let _serial = plx_base::testlock::serial();
    for case in 0..4 {
        let (profile, favorite, client, _reset) = setup();
        let original = capture(&profile, &favorite);
        let cancel = AtomicBool::new(case == 0);
        let result = original.prepare_with(&plx_base::task::OffFrame::for_test(), &cancel, |_, _| {
            assert_ne!(case, 0, "pre-cancelled work must not read metadata");
            if case == 1 { client.set_token("replacement"); }
            if case == 2 { client.set_token(""); client.set_token("fixture-private-token"); }
            if case == 3 { cancel.store(true, Ordering::Release); }
            Some(metadata())
        });
        assert!(matches!(result, Err(Error::Changed | Error::Cancelled)));
    }
}

#[test]
fn malformed_multipart_plugin_absolute_keys_and_wrong_item_fail_closed() {
    let _serial = plx_base::testlock::serial();
    for case in 0..6 {
        let (profile, favorite, _, _reset) = setup();
        let mut item = metadata();
        match case {
            0 => item.rating_key = "78".into(),
            1 => item.media[0].part.push(serde_json::from_value(json!({"key":"/library/parts/8/file.mkv"})).unwrap()),
            2 => item.media[0].part[0].key = "https://untrusted.invalid/file".into(),
            3 => item.media[0].part[0].key = "/services/iva/assets?url=private".into(),
            4 => item.media.clear(),
            _ => item.kind = "show".into(),
        }
        assert!(matches!(capture(&profile, &favorite).prepare_with(&plx_base::task::OffFrame::for_test(),
            &AtomicBool::new(false), |_, _| Some(item)), Err(Error::Invalid | Error::Unsupported)));
    }
    for bad in ["/library/parts/1/../file.mkv", "/library/parts/1/file.mkv?token=x",
        "/library/parts/1/%2e%2e/file.mkv", "//other/library/parts/1/file.mp4", "/library/parts/1/"] {
        assert!(!part_key(bad));
    }
}

#[test]
fn prepared_witness_remains_revocable_after_export_and_projects_ordinary_protection() {
    let _serial = plx_base::testlock::serial();
    let (profile, favorite, client, _reset) = setup();
    let mut item = metadata();
    item.library_section_title.clear();
    let plan = capture(&profile, &favorite).prepare_with(&plx_base::task::OffFrame::for_test(),
        &AtomicBool::new(false), |_, _| Some(item)).unwrap();
    let (entry, witness) = plan.into_parts().unwrap();
    assert_eq!(entry.protection, Some(true), "missing source category uses ordinary protection classification");
    assert!(witness.current());
    client.set_token("");
    assert!(!witness.current());
}

#[test]
fn capture_rejects_cross_server_or_nondecimal_identity_before_any_read() {
    let _serial = plx_base::testlock::serial();
    let (profile, mut favorite, _, _reset) = setup();
    favorite.locator = Locator::Plex { server_id: "other".into(), rating_key: "77".into() };
    assert!(matches!(Capture::capture(&profile, &favorite, "mv-owner".into()), Err(Error::Invalid)));
    favorite.locator = Locator::Plex { server_id: profile.source.clone(), rating_key: "../77".into() };
    assert!(matches!(Capture::capture(&profile, &favorite, "mv-owner".into()), Err(Error::Invalid)));
}

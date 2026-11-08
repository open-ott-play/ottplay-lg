use super::*;
use plx_data::ott::archive::{ArchiveInterval, CatchupMode, CatchupSource};

fn fixture() -> (ProviderProfile, CatalogEntry, ArchiveInterval) {
    let profile = ProviderProfile {
        id: "archive-fixture".into(),
        name: "Archive".into(),
        kind: ProviderKind::M3u,
        source: "https://catalog.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let entry = CatalogEntry {
        synopsis: None,
        id: "channel".into(),
        title: "Channel".into(),
        category: "Channels".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: "https://live.invalid/feed?token=original".into(),
        },
        duration_ms: None,
        provider_id: Some("channel-id".into()),
        provider_id_field: Some("tvg-id".into()),
        live: Some(ChannelMetadata {
            tvg_name: "Channel".into(),
            guide_shift_seconds: 0,
            catchup: Some(CatchupSource {
                mode: CatchupMode::Append,
                source: "?start={utc}&end={utcend}".into(),
                retention_seconds: 1000,
                correction_seconds: 0,
            }),
        }),
    };
    (
        profile,
        entry,
        ArchiveInterval {
            start: 1000,
            end: 1100,
        },
    )
}

#[test]
fn archive_uses_fresh_resolved_url_but_preserves_live_identity() {
    let (profile, entry, interval) = fixture();
    let original = entry.clone();
    let intent = Intent::new(&profile, &entry, interval, 1500).unwrap();
    assert_eq!(
        intent
            .apply(
                &profile,
                &entry,
                "https://renewed.invalid/feed?token=renewed",
                1600
            )
            .unwrap(),
        "https://renewed.invalid:443/feed?token=renewed&start=1000&end=1100"
    );
    assert_eq!(entry, original);
    assert!(!format!("{intent:?}").contains("original"));
}

#[test]
fn archive_rejects_changed_profile_locator_and_rule() {
    let (profile, entry, interval) = fixture();
    let intent = Intent::new(&profile, &entry, interval, 1500).unwrap();
    let mut edited = profile.clone();
    edited.source = "https://catalog.invalid/replacement.m3u".into();
    assert!(intent.validate(&edited, &entry, 1600).is_err());
    let mut edited = entry.clone();
    edited.media = MediaLocator::Http {
        url: "https://live.invalid/other".into(),
    };
    assert!(intent.validate(&profile, &edited, 1600).is_err());
    let mut edited = entry.clone();
    edited
        .live
        .as_mut()
        .unwrap()
        .catchup
        .as_mut()
        .unwrap()
        .correction_seconds = 1;
    assert!(intent.validate(&profile, &edited, 1600).is_err());
    let mut edited = entry.clone();
    edited.id = "other-channel".into();
    assert!(intent.validate(&profile, &edited, 1600).is_err());
}

#[test]
fn archive_rechecks_retention_after_resolution_delay() {
    let (profile, entry, interval) = fixture();
    let intent = Intent::new(&profile, &entry, interval, 1500).unwrap();
    assert!(intent.validate(&profile, &entry, 2000).is_ok());
    assert!(intent.validate(&profile, &entry, 2001).is_err());
    assert!(intent
        .apply(&profile, &entry, "file:///private/movie", 1600)
        .is_err());
}

#[test]
fn ordinary_or_other_provider_entries_do_not_gain_archive_capability() {
    let (mut profile, mut entry, interval) = fixture();
    profile.kind = ProviderKind::Xtream;
    assert!(Intent::new(&profile, &entry, interval, 1500).is_err());
    profile.kind = ProviderKind::M3u;
    entry.live.as_mut().unwrap().catchup = None;
    assert!(Intent::new(&profile, &entry, interval, 1500).is_err());
    entry.live = None;
    assert!(Intent::new(&profile, &entry, interval, 1500).is_err());
}

#[test]
fn programme_display_is_bound_to_interval_and_does_not_change_archive_target() {
    let (profile, entry, interval) = fixture();
    let intent = Intent::new(&profile, &entry, interval, 1500).unwrap();
    let programme = plx_data::ott::epg::Programme {
        channel: "epg-channel".into(),
        start: interval.start,
        end: interval.end,
        title: "Selected archive programme".into(),
        description: "Full selected description".into(),
    };
    let decorated = intent.clone().with_programme(&programme).unwrap();
    assert_eq!(decorated.programme(), Some(programme.clone()));
    assert_eq!(
        decorated.apply(&profile, &entry, "https://renewed.invalid/feed", 1500),
        intent.apply(&profile, &entry, "https://renewed.invalid/feed", 1500)
    );
    let mut different = programme;
    different.end += 1;
    assert!(intent.with_programme(&different).is_err());
}

#[test]
fn profile_fallback_resolves_missing_depth_without_mutating_its_binding() {
    let (profile, mut entry, interval) = fixture();
    entry
        .live
        .as_mut()
        .unwrap()
        .catchup
        .as_mut()
        .unwrap()
        .retention_seconds = 0;
    let original = entry.clone();
    assert!(Intent::new(&profile, &entry, interval, 1500).is_err());
    let intent = Intent::new_with_fallback(&profile, &entry, interval, 1500, 1).unwrap();
    assert_eq!(
        intent
            .apply(&profile, &entry, "https://renewed.invalid/feed", 1600)
            .unwrap(),
        "https://renewed.invalid:443/feed?start=1000&end=1100"
    );
    assert!(intent.validate(&profile, &entry, 4600).is_ok());
    assert!(intent.validate(&profile, &entry, 4601).is_err());
    assert_eq!(entry, original);
    let mut changed = entry.clone();
    changed
        .live
        .as_mut()
        .unwrap()
        .catchup
        .as_mut()
        .unwrap()
        .retention_seconds = 3600;
    assert!(
        intent.validate_binding(&profile, &changed).is_err(),
        "equivalent effective depth does not replace the captured original channel"
    );
    let mut changed_profile = profile.clone();
    changed_profile.source.push_str("?changed=1");
    assert!(intent.validate_binding(&changed_profile, &entry).is_err());
    assert!(!format!("{intent:?}").contains("token"));
}

#[test]
fn declared_depth_overrides_profile_zero_and_positive_values() {
    let (profile, entry, interval) = fixture();
    let automatic = Intent::new_with_fallback(&profile, &entry, interval, 1500, 0).unwrap();
    let fallback = Intent::new_with_fallback(&profile, &entry, interval, 1500, 8760).unwrap();
    assert_eq!(
        automatic, fallback,
        "irrelevant fallback is not part of the effective intent"
    );
    assert!(fallback.validate(&profile, &entry, 2000).is_ok());
    assert!(
        fallback.validate(&profile, &entry, 2001).is_err(),
        "profile depth must not extend advertised depth"
    );
}

#[test]
fn fallback_is_bounded_and_zero_never_grants_missing_depth() {
    let (profile, mut entry, interval) = fixture();
    entry
        .live
        .as_mut()
        .unwrap()
        .catchup
        .as_mut()
        .unwrap()
        .retention_seconds = 0;
    assert!(Intent::new_with_fallback(&profile, &entry, interval, 1500, 0).is_err());
    assert!(Intent::new_with_fallback(&profile, &entry, interval, 1500, 8760).is_ok());
    for hours in [8761, u64::MAX] {
        assert_eq!(
            Intent::new_with_fallback(&profile, &entry, interval, 1500, hours),
            Err("Use whole archive hours from 0 to 8760")
        );
    }
    assert_ne!(
        Intent::new_with_fallback(&profile, &entry, interval, 1500, 1).unwrap(),
        Intent::new_with_fallback(&profile, &entry, interval, 1500, 2).unwrap()
    );
}

#[test]
fn parser_to_intent_applies_profile_depth_only_to_omitted_retention() {
    let (profile, _, interval) = fixture();
    for mode in ["default", "append", "shift", "flussonic"] {
        let source = if mode == "default" {
            "https://video.invalid/archive?utc={utc}"
        } else {
            "?utc={utc}"
        };
        let text = format!("#EXTM3U catchup=\"{mode}\" catchup-source=\"{source}\"\n#EXTINF:-1,News\nhttps://video.invalid/index.m3u8");
        let catalog = plx_data::ott::parse_m3u(&text, &profile).unwrap();
        assert!(
            Intent::new_with_fallback(&profile, &catalog.entries[0], interval, 1500, 1).is_ok(),
            "{mode}"
        );
    }
    for rule in [
        "catchup=\"none\"",
        "catchup=\"unknown\"",
        "catchup=\"shift\" catchup-days=\"0\"",
        "catchup=\"shift\" catchup-days=\"NaN\"",
        "catchup=\"append\"",
        "",
    ] {
        let text = format!("#EXTM3U {rule}\n#EXTINF:-1,News\nhttps://video.invalid/index.m3u8");
        let catalog = plx_data::ott::parse_m3u(&text, &profile).unwrap();
        assert!(
            Intent::new_with_fallback(&profile, &catalog.entries[0], interval, 1500, 8760).is_err(),
            "{rule}"
        );
    }
}

#[test]
fn fallback_does_not_grant_archive_to_files_or_other_provider_kinds() {
    let (mut profile, mut entry, interval) = fixture();
    entry
        .live
        .as_mut()
        .unwrap()
        .catchup
        .as_mut()
        .unwrap()
        .retention_seconds = 0;
    profile.kind = ProviderKind::Xtream;
    assert!(Intent::new_with_fallback(&profile, &entry, interval, 1500, 1).is_err());
    profile.kind = ProviderKind::M3u;
    entry.live = None;
    assert!(Intent::new_with_fallback(&profile, &entry, interval, 1500, 1).is_err());
    let text = "#EXTM3U catchup=\"shift\"\n#EXTINF:120,Movie\nhttps://video.invalid/movie.mp4";
    let catalog = plx_data::ott::parse_m3u(text, &profile).unwrap();
    assert!(Intent::new_with_fallback(&profile, &catalog.entries[0], interval, 1500, 1).is_err());
}

//! Read the real immutable Runtime view: no provider, renderer or worker runs.
use super::*;
use plx_screens::ott::{OttMediaKind, OttMediaProgress};

fn listed(entries: Vec<CatalogEntry>) -> Runtime {
    let mut runtime = runtime_fixture();
    let profile = ProviderProfile {
        id: "local-fixture".into(), name: "Fixture".into(), kind: ProviderKind::Local,
        source: "/fixture/videos".into(), source_alias: "Fixture".into(), enabled: true,
    };
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles.push(profile);
    runtime.catalog.entries = entries;
    runtime.page = "channels";
    runtime
}
fn leaf(id: &str, locator: MediaLocator, duration_ms: Option<u64>) -> CatalogEntry {
    CatalogEntry { synopsis: None, id: id.into(), title: "Movie S01E01 Live TV".into(), category: "Folder".into(),
        description: "Description retained in full.".into(), poster: String::new(), media: locator,
        duration_ms, provider_id: None, provider_id_field: None, live: None }
}
fn local(id: &str, duration_ms: Option<u64>) -> CatalogEntry {
    leaf(id, MediaLocator::Local { relative_path: format!("Folder/{id}.mkv") }, duration_ms)
}
fn xtream(id: &str, stream_kind: &str, series_id: Option<&str>) -> CatalogEntry {
    leaf(id, MediaLocator::Xtream { stream_id: id.into(), stream_kind: stream_kind.into(),
        extension: "mp4".into(), series_id: series_id.map(str::to_owned), has_direct_source: false }, Some(10_000))
}
fn assert_neutral(kind: OttMediaKind) {
    assert!(kind != OttMediaKind::Movie && kind != OttMediaKind::Series
        && kind != OttMediaKind::Episode && kind != OttMediaKind::LiveChannel
        && kind != OttMediaKind::LocalVideo, "unknown subtype must stay neutral");
}

#[test]
fn admitted_local_rows_expose_only_available_facts_and_keep_identity_order() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = listed(vec![local("one", Some(12_345)), local("two", None), local("zero", Some(0))]);
    runtime.selected = Some("two".into());
    let view = runtime.view();
    assert_eq!(view.rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), ["one", "two", "zero"]);
    assert_eq!(view.selected_row.as_deref(), Some("two"));
    for (index, row) in view.rows.iter().enumerate() {
        assert_eq!(row.action, OttAction::Play(row.id.clone()));
        assert_eq!(row.title, format!("{}  Movie S01E01 Live TV", index + 1));
        assert_eq!(row.description, "Description retained in full.");
        assert!(row.poster.is_none());
        let media = row.media.as_ref().expect("admitted local video facts");
        assert_eq!(media.kind, OttMediaKind::LocalVideo);
        assert_eq!(media.duration_ms, if index == 0 { Some(12_345) } else { None });
        assert!(media.progress.is_none());
    }
}

#[test]
fn plex_portal_and_m3u_do_not_guess_live_or_episode_from_titles_or_provider() {
    let _serial = plx_base::testlock::serial();
    let rows = vec![
        leaf("plex", MediaLocator::Plex { server_id: "fixture".into(), rating_key: "42".into() }, Some(10_000)),
        leaf("portal", MediaLocator::VPortal { request: json!({"id":"42"}) }, None),
        leaf("m3u", MediaLocator::Http { url: "https://media.invalid/channel.m3u8".into() }, None),
    ];
    let runtime = listed(rows);
    for row in runtime.view().rows { assert_neutral(row.media.expect("playable neutral video").kind); }
}

#[test]
fn explicit_provider_kinds_are_preserved_and_unknown_values_are_not_invented() {
    let _serial = plx_base::testlock::serial();
    let mut rows = vec![xtream("live", "live", None), xtream("movie", "movie", None),
        xtream("episode", "series", Some("show")), xtream("unknown", "future-kind", None)];
    rows.push(leaf("itv", MediaLocator::Stalker { cmd: "fixture".into(), stream_kind: "itv".into(), series: "0".into() }, Some(10_000)));
    rows.push(leaf("episode2", MediaLocator::Stalker { cmd: "fixture".into(), stream_kind: "vod".into(), series: "3".into() }, None));
    rows.push(leaf("vod", MediaLocator::Stalker { cmd: "fixture".into(), stream_kind: "vod".into(), series: "0".into() }, None));
    let runtime = listed(rows);
    let view = runtime.view();
    assert_eq!(view.rows[0].media.as_ref().unwrap().kind, OttMediaKind::LiveChannel);
    assert!(view.rows[0].media.as_ref().unwrap().duration_ms.is_none());
    assert_eq!(view.rows[1].media.as_ref().unwrap().kind, OttMediaKind::Movie);
    assert_eq!(view.rows[2].media.as_ref().unwrap().kind, OttMediaKind::Episode);
    assert!(view.rows[3].media.is_none());
    assert_eq!(view.rows[4].media.as_ref().unwrap().kind, OttMediaKind::LiveChannel);
    assert_eq!(view.rows[5].media.as_ref().unwrap().kind, OttMediaKind::Episode);
    assert_neutral(view.rows[6].media.as_ref().unwrap().kind);
}

#[test]
fn checkpoint_progress_requires_same_row_current_source_and_usable_duration() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = listed(vec![local("one", Some(10_000)), local("two", Some(10_000))]);
    let fingerprint = profile_fingerprint(runtime.active().unwrap());
    runtime.saved.resume.insert("local-fixture".into(), SavedResume {
        entry_id: "one".into(), position_ms: 2_500, source_fingerprint: fingerprint,
    });
    let view = runtime.view();
    assert_eq!(view.rows[0].media.as_ref().unwrap().progress,
        Some(OttMediaProgress { position_ms: 2_500, duration_ms: 10_000 }));
    assert!(view.rows[1].media.as_ref().unwrap().progress.is_none());
    for position in [0, 10_000, u64::MAX] {
        runtime.saved.resume.get_mut("local-fixture").unwrap().position_ms = position;
        assert!(runtime.view().rows[0].media.as_ref().unwrap().progress.is_none());
    }
    runtime.saved.resume.get_mut("local-fixture").unwrap().position_ms = 2_500;
    runtime.saved.profiles.profiles[0].source = "/fixture/other".into();
    assert!(runtime.view().rows[0].media.as_ref().unwrap().progress.is_none());
}

#[test]
fn partial_failed_and_complete_catalogues_report_the_actual_visible_count() {
    let _serial = plx_base::testlock::serial();
    let mut other = local("other", None); other.category = "Other".into();
    let mut runtime = listed(vec![local("one", None), other]);
    runtime.saved.category = "Folder".into();
    runtime.catalog_loading = true;
    runtime.status = "2 videos".into();
    let view = runtime.view();
    let progress = view.catalog_progress.expect("catalogue progress");
    assert_eq!((progress.loaded, progress.total, progress.loading, progress.complete), (1, None, true, false));
    assert!(view.status.is_empty(), "typed count replaces only the generated count");
    runtime.catalog_loading = false;
    runtime.status = "Provider page failed".into();
    let view = runtime.view();
    let progress = view.catalog_progress.unwrap();
    assert_eq!((progress.loaded, progress.total, progress.loading, progress.complete), (1, None, false, false));
    assert_eq!(view.status, "Provider page failed");
    runtime.catalog_complete = true;
    runtime.status.clear();
    assert_eq!(runtime.view().catalog_progress.unwrap().total, Some(1));
    runtime.saved.category = "No match".into();
    let progress = runtime.view().catalog_progress.unwrap();
    assert_eq!((progress.loaded, progress.total), (2, Some(2)),
        "a missing remembered category projects All without erasing the choice");
    assert_eq!(runtime.saved.category, "No match");
    runtime.catalog.entries.clear();
    let progress = runtime.view().catalog_progress.unwrap();
    assert_eq!((progress.loaded, progress.total), (0, Some(0)));
}

#[test]
fn mixed_favorites_are_complete_and_metadata_belongs_only_to_its_exact_row() {
    use plx_data::ott::favorites::{Locator, Origin};
    let _serial = plx_base::testlock::serial();
    let mut runtime = listed(Vec::new());
    let local = Favorite { title: "Local".into(), origin: Origin { provider: "local".into(), profile: "Fixture".into(),
        subprofile: None, source_alias: "Fixture".into() }, locator: Locator::Local { root_alias: "Fixture".into(), relative_path: "Folder/one.mkv".into() } };
    let channel = Favorite { title: "Channel or video".into(), origin: Origin { provider: "m3u".into(), profile: "Other".into(),
        subprofile: None, source_alias: "Other".into() }, locator: Locator::Channel { channel_id: "one".into() } };
    runtime.saved.favorites.toggle(local.clone()).unwrap();
    runtime.saved.favorites.toggle(channel.clone()).unwrap();
    let id = format!("favorite:{}", local.identity());
    runtime.favorite_preview = Some((id.clone(), leaf("provider-row", MediaLocator::Local { relative_path: "Folder/one.mkv".into() }, Some(12_000))));
    runtime.page = "favorites";
    runtime.saved.favorites_selected = true;
    runtime.catalog_loading = true;
    let view = runtime.view();
    let progress = view.catalog_progress.unwrap();
    assert_eq!((progress.loaded, progress.total, progress.loading, progress.complete), (2, Some(2), false, true));
    assert_eq!(view.rows[0].id, id);
    assert_eq!(view.rows[0].media.as_ref().unwrap().duration_ms, Some(12_000));
    assert_neutral(view.rows[1].media.as_ref().unwrap().kind);
    assert!(view.rows[1].media.as_ref().unwrap().duration_ms.is_none());
    assert!(view.rows.iter().all(|row| row.favorite));
    runtime.favorite_preview.as_mut().unwrap().0 = "favorite:retired".into();
    assert!(runtime.view().rows[0].media.as_ref().unwrap().duration_ms.is_none());
}

#[test]
fn navigation_rows_never_turn_into_media_or_catalogue_progress() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = listed(vec![local("one", Some(10_000))]);
    for page in ["providers", "profiles", "categories", "settings"] {
        runtime.page = page;
        let view = runtime.view();
        assert!(view.catalog_progress.is_none());
        assert!(view.rows.iter().all(|row| row.media.is_none()));
        assert!(view.rows.iter().all(|row| row.content_title.is_none()));
    }
}


#[test]
fn content_title_preserves_real_leading_digits_and_the_numbered_catalog_label() {
    let _serial = plx_base::testlock::serial();
    let titles = ["2001: A Space Odyssey", "007 — A title", "42 кота"];
    let entries = titles.iter().enumerate().map(|(index, title)| {
        let mut entry = local(&format!("file-{index}"), Some(12_345));
        entry.title = (*title).into();
        entry
    }).collect();
    let runtime = listed(entries);
    let view = runtime.view();
    for (index, (row, title)) in view.rows.iter().zip(titles).enumerate() {
        assert_eq!(row.content_title.as_deref(), Some(title));
        assert_eq!(row.title, format!("{}  {title}", index + 1));
        assert_eq!(row.id, format!("file-{index}"));
        assert_eq!(row.action, OttAction::Play(row.id.clone()));
        assert_eq!(row.detail, "Folder");
        assert_eq!(row.description, "Description retained in full.");
        assert!(row.poster.is_none());
    }
}

#[test]
fn portal_content_title_preserves_full_description_and_typed_artwork() {
    let _serial = plx_base::testlock::serial();
    let mut entry = leaf("portal-film", MediaLocator::VPortal {
        request: json!({"id":"film-1984"}),
    }, Some(9_000));
    entry.title = "1984 — Film".into();
    entry.description = "2026\n00:00:09\n12+\nFIRST sentence. SECOND sentence.\n\nFINAL paragraph.".into();
    entry.synopsis = plx_data::ott::SynopsisSpan::new(&entry.description, entry.description.find("FIRST").unwrap());
    entry.poster = "https://fixture.invalid:443/fixed-film.png".into();
    let mut runtime = listed(vec![entry.clone()]);
    let profile = &mut runtime.saved.profiles.profiles[0];
    profile.kind = ProviderKind::VPortal;
    profile.source = "portal::[key:fixture]https://fixture.invalid/vportal".into();
    let view = runtime.view();
    let row = &view.rows[0];
    assert_eq!(row.content_title.as_deref(), Some(entry.title.as_str()));
    assert_eq!(row.title, "1  1984 — Film");
    assert_eq!(row.description, entry.description);
    assert!(matches!(row.poster.as_ref(), Some(plx_screens::ott::OttPoster::Http { url })
        if url == &entry.poster));
    assert_eq!(row.media.as_ref().unwrap().kind, OttMediaKind::Video);
    assert_eq!(row.media.as_ref().unwrap().duration_ms, Some(9_000));
    assert_eq!(row.media.as_ref().unwrap().synopsis.as_ref().unwrap().get(&row.description),
        Some("FIRST sentence. SECOND sentence.\n\nFINAL paragraph."));

    runtime.catalog.entries[0].description = "Updated text without the old prefix".into();
    let view = runtime.view();
    assert!(view.rows[0].media.as_ref().unwrap().synopsis.is_none(), "stale annotations must not crop new prose");
}

#[test]
fn favorite_content_title_keeps_its_own_label_and_exact_preview_metadata() {
    use plx_data::ott::favorites::{Locator, Origin};
    let _serial = plx_base::testlock::serial();
    let mut runtime = listed(Vec::new());
    let favorite = Favorite {
        title: "007 — My saved title".into(),
        origin: Origin { provider: "m3u".into(), profile: "Fixture".into(),
            subprofile: None, source_alias: "Fixture".into() },
        locator: Locator::Channel { channel_id: "one".into() },
    };
    let other = Favorite { title: "42 — Another title".into(),
        locator: Locator::Channel { channel_id: "two".into() }, ..favorite.clone() };
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.saved.favorites.toggle(other.clone()).unwrap();
    let id = format!("favorite:{}", favorite.identity());
    let mut preview = leaf("resolved-source-row", MediaLocator::Http {
        url: "https://fixture.invalid/video.mp4".into(),
    }, Some(12_000));
    preview.title = "Different server title".into();
    preview.description = "2026\nFIRST sentence. SECOND sentence.\n\nFINAL paragraph.".into();
    preview.synopsis = plx_data::ott::SynopsisSpan::new(&preview.description, "2026\n".len());
    preview.poster = "https://fixture.invalid:443/fixed-film.png".into();
    runtime.favorite_preview = Some((id.clone(), preview.clone()));
    runtime.page = "favorites";
    runtime.saved.favorites_selected = true;
    let view = runtime.view();
    let row = view.rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.content_title.as_deref(), Some(favorite.title.as_str()));
    // The display ordinal is separate from the exact saved content title.
    assert_eq!(row.title, format!("1  {}", favorite.title));
    assert_eq!(row.description, preview.description);
    assert_eq!(row.media.as_ref().unwrap().synopsis.as_ref().unwrap().get(&row.description),
        Some("FIRST sentence. SECOND sentence.\n\nFINAL paragraph."));
    assert!(matches!(row.poster.as_ref(), Some(plx_screens::ott::OttPoster::Http { url })
        if url == &preview.poster));
    assert_eq!(row.action, OttAction::Play(id.clone()));
    let other_id = format!("favorite:{}", other.identity());
    let other_row = view.rows.iter().find(|row| row.id == other_id).unwrap();
    assert_eq!(other_row.content_title.as_deref(), Some(other.title.as_str()));
    assert_eq!(other_row.title, format!("2  {}", other.title));
    assert_eq!(other_row.action, OttAction::Play(other_id.clone()));
    assert!(other_row.description.is_empty());
    assert!(other_row.poster.is_none());
    runtime.favorite_preview.as_mut().unwrap().0 = "favorite:retired".into();
    let view = runtime.view();
    let row = view.rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.content_title.as_deref(), Some(favorite.title.as_str()));
    assert!(row.description.is_empty());
    assert!(row.poster.is_none());
}

fn projection_profile(kind: ProviderKind) -> ProviderProfile {
    let source = match kind {
        ProviderKind::M3u => "https://fixture.invalid/list.m3u",
        ProviderKind::Xtream => r#"{"server":"https://fixture.invalid","username":"fixture","password":"fixture"}"#,
        ProviderKind::Stalker => r#"{"portal":"https://fixture.invalid/c/","mac":"00:1a:79:aa:bb:cc"}"#,
        ProviderKind::VPortal => "portal::[key:fixture]https://fixture.invalid/vportal",
        ProviderKind::Local => "/fixture/videos",
        ProviderKind::Plex => "fixture-server",
    };
    let profile = ProviderProfile { id: "projection-fixture".into(), name: "Fixture".into(),
        kind, source: source.into(), source_alias: "Fixture".into(), enabled: true };
    profile.validate().expect("valid synthetic provider profile");
    profile
}

/// The catalogue row before context hoisting: serves as an output oracle for
/// the exact flags, ordinal, action and defaults, including invalid profiles.
fn previous_catalog_row(runtime: &Runtime, profile: Option<&ProviderProfile>, index: usize,
    entry: &CatalogEntry, favorite: bool) -> OttRow
{
    let mut row = super::super::ui_projection::row(entry.id.clone(),
        format!("{}  {}", index + 1, entry.title), entry.category.clone(),
        OttAction::Play(entry.id.clone()), favorite);
    row.guide_available = profile.is_some_and(|profile| guide_ui::is_live(profile, entry));
    row.records_available = profile.is_some_and(|profile| guide_ui::records_available(runtime, profile, entry));
    row
}

#[test]
fn catalog_context_twenty_thousand_vportal_rows_do_not_query_records() {
    let _serial = plx_base::testlock::serial();
    let runtime = Box::new(listed(Vec::new()));
    let profile = projection_profile(ProviderKind::VPortal);
    let context = super::super::ui_projection::CatalogContext::new(Some(&profile));
    let mut queries = 0;
    for index in 0..20_000 {
        let entry = leaf(&format!("portal-{index}"),
            MediaLocator::VPortal { request: json!({"id": index}) }, Some(90_000));
        let row = context.row(index, &entry, index % 2 == 0, |profile, entry| {
            queries += 1;
            guide_ui::records_available(&runtime, profile, entry)
        });
        assert_eq!(row.id, entry.id);
        assert_eq!(row.title, format!("{}  {}", index + 1, entry.title));
        assert_eq!(row.action, OttAction::Play(entry.id.clone()));
        assert_eq!(row.favorite, index % 2 == 0);
        assert!(!row.guide_available && !row.records_available);
    }
    assert_eq!(queries, 0, "a VPortal row must never enter the M3U Records query");
}

#[test]
fn catalog_context_m3u_uses_real_records_query_and_keeps_all_rejections() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Box::new(listed(Vec::new()));
    runtime.wall_clock_ms = 2_000_000_000_000;
    let profile = projection_profile(ProviderKind::M3u);
    let catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 catchup=\"shift\" catchup-days=\"7\",Archive\nhttps://fixture.invalid/archive\n\
         #EXTINF:-1,Live only\nhttps://fixture.invalid/live", &profile).unwrap();
    let mut finite = catalog.entries[0].clone();
    finite.duration_ms = Some(30_000);
    let mut local = catalog.entries[0].clone();
    local.media = MediaLocator::Local { relative_path: "Archive.mp4".into() };
    let entries = [&catalog.entries[0], &catalog.entries[1], &finite, &local];
    for variant in 0..4 {
        let mut profile = profile.clone();
        match variant {
            1 => profile.enabled = false,
            2 => profile.source = "invalid".into(),
            3 => profile.source = "portal::[key:fixture]https://fixture.invalid/vportal".into(),
            _ => {},
        }
        let context = super::super::ui_projection::CatalogContext::new(Some(&profile));
        let mut queries = 0;
        for (index, entry) in entries.iter().enumerate() {
            let row = context.row(index, entry, false, |profile, entry| {
                queries += 1;
                guide_ui::records_available(&runtime, profile, entry)
            });
            assert_eq!(row, previous_catalog_row(&runtime, Some(&profile), index, entry, false));
            assert_eq!(row.records_available, variant == 0 && index == 0);
        }
        assert_eq!(queries, entries.len(), "M3U retains the full Records validator");
    }
}

#[test]
fn catalog_context_matches_previous_rows_for_every_provider_and_missing_profile() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Box::new(listed(Vec::new()));
    runtime.wall_clock_ms = 2_000_000_000_000;
    let entries = [
        leaf("http", MediaLocator::Http { url: "https://fixture.invalid/live".into() }, None),
        leaf("video", MediaLocator::Http { url: "https://fixture.invalid/video.mp4".into() }, Some(1_000)),
        xtream("xtream-live", "live", None),
        xtream("xtream-video", "movie", None),
        leaf("stalker-live", MediaLocator::Stalker { cmd: "fixture".into(), stream_kind: "itv".into(), series: "0".into() }, None),
        leaf("stalker-api", MediaLocator::StalkerApi { channel_id: "7".into() }, None),
        leaf("portal", MediaLocator::VPortal { request: json!({"id": 7}) }, None),
        leaf("plex", MediaLocator::Plex { server_id: "fixture-server".into(), rating_key: "7".into() }, None),
        local("local", Some(1_000)),
    ];
    for kind in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
        ProviderKind::VPortal, ProviderKind::Local, ProviderKind::Plex]
    {
        for variant in 0..3 {
            let mut profile = projection_profile(kind);
            if variant == 1 { profile.enabled = false; }
            if variant == 2 { profile.source = "invalid\nsource".into(); }
            let context = super::super::ui_projection::CatalogContext::new(Some(&profile));
            let mut queries = 0;
            for (index, entry) in entries.iter().enumerate() {
                let favorite = index % 2 == 0;
                let row = context.row(index + 11, entry, favorite, |profile, entry| {
                    queries += 1;
                    guide_ui::records_available(&runtime, profile, entry)
                });
                assert_eq!(row, previous_catalog_row(&runtime, Some(&profile), index + 11, entry, favorite));
                if (kind == ProviderKind::Stalker && index == 4)
                    || (kind == ProviderKind::Xtream && index == 2)
                {
                    assert!(row.guide_available, "non-M3U live Guide capability stays intact");
                }
            }
            assert_eq!(queries, if kind == ProviderKind::M3u { entries.len() } else { 0 });
        }
    }
    let context = super::super::ui_projection::CatalogContext::new(None);
    for (index, entry) in entries.iter().enumerate() {
        let row = context.row(index, entry, true, |_, _| panic!("missing profile cannot query Records"));
        assert_eq!(row, previous_catalog_row(&runtime, None, index, entry, true));
    }
}

#[test]
fn catalog_runtime_context_preserves_filtered_ordinals_metadata_and_favorite_stars() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Box::new(listed(Vec::new()));
    let profile = projection_profile(ProviderKind::M3u);
    let mut inactive = projection_profile(ProviderKind::VPortal);
    inactive.id = "inactive-portal".into();
    inactive.enabled = false;
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles = vec![inactive, profile.clone()];
    runtime.wall_clock_ms = 2_000_000_000_000;
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Other\",Hidden\nhttps://fixture.invalid/hidden\n\
         #EXTINF:-1 tvg-id=\"archive\" group-title=\"Selected\" catchup=\"shift\" catchup-days=\"7\",Archive\nhttps://fixture.invalid/archive\n\
         #EXTINF:-1 tvg-id=\"video\" group-title=\"Selected\",Video\nhttps://fixture.invalid/video.mp4", &profile).unwrap();
    runtime.catalog_complete = true;
    runtime.catalog.entries[1].description = "First sentence. Second sentence.".into();
    runtime.catalog.entries[1].poster = "https://fixture.invalid/poster.jpg".into();
    runtime.catalog.entries[2].duration_ms = Some(10_000);
    let favorite = selection::capture(&profile, &runtime.catalog.entries[1]).unwrap();
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.saved.category = "Selected".into();
    runtime.selected = Some(runtime.catalog.entries[1].id.clone());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let state = (runtime.generation, runtime.catalog_revision, runtime.navigation_epoch, runtime.play_generation);
    let view = runtime.raw_view();
    assert_eq!(view.rows.len(), 2);
    assert_eq!(view.selected_row, runtime.selected);
    for (index, row) in (1..3).zip(&view.rows) {
        let entry = &runtime.catalog.entries[index];
        let mut expected = previous_catalog_row(&runtime, Some(&profile), index, entry, index == 1);
        expected.content_title = Some(entry.title.clone());
        expected.media = super::super::ui_projection::entry(entry, &entry.id, None);
        expected.description = entry.description.clone();
        expected.poster = entry_poster(entry);
        assert_eq!(row, &expected);
    }
    assert!(view.rows[0].favorite && view.rows[0].records_available);
    assert!(!view.rows[1].favorite && !view.rows[1].records_available);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!((runtime.generation, runtime.catalog_revision, runtime.navigation_epoch, runtime.play_generation), state);
    assert!(runtime.resolver_next.is_none() && !runtime.catalog_loading && !runtime.saving);
}

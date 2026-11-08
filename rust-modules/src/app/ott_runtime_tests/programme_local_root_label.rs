//! Real held Local grants and the existing Now Playing publisher. These tests
//! model route admission; they do not start a decoder, scanner or Saved writer.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{self as local, BoundSource};

fn local_fixture(relative: &str) -> (local_integration::TempDir, Box<App>, BoundSource) {
    let (directory, mut app, root) = local_integration::setup();
    local_integration::install(&mut app, Some(&root), "first");
    app.running = true;
    if relative != "first.mp4" {
        // Keep the installed native source consistent with the injected row.
        playback::replace_player_in_place(
            &mut app.player, &mut app.adapters.player, &mut app.repause_at, &mut app.pages,
        );
        route::reset_player_control_for_test(&app.player.session);
        let mut info = unsupported_info();
        info.video_codec = "h264".into();
        info.audio_codec = "aac".into();
        info.audio_channels = 2;
        info.audio_sample_rate = 48000;
        info.audio_bit_depth = 16;
        route::request_external_source(
            &mut app.player.session, &info,
            plx_media::ff::ExternalSource::Local(
                std::path::Path::new(root.canonical_path.as_deref().unwrap()).join(relative),
            ),
            "Current file", "Movies", plx_platform::devcaps::assumed_shared(),
        ).unwrap();
    }
    let runtime = app.ott.as_mut().unwrap();
    let mut profile = runtime.playing.as_ref().unwrap().profile.clone();
    profile.id = "local1".into();
    profile.name = "local1".into();
    let files = [relative, "next.mp4"].map(|path| plx_data::ott::LocalFile {
        relative_path: path.into(), size: 1, duration_ms: Some(60_000),
    });
    let catalog = plx_data::ott::catalog_from_local_files(&files, &profile).unwrap();
    let entry = catalog.entries.iter().find(|entry|
        matches!(&entry.media, MediaLocator::Local { relative_path } if relative_path == relative)
    ).unwrap().clone();
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile.clone();
    playing.entry = entry.clone();
    playing.selection_id = entry.id.clone();
    playing.identity = route::playback_identity(&app.player.session);
    playing.source = route::external_source(&app.player.session);
    if let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() {
        session.profile = profile;
        let items = catalog.entries.iter().map(|entry| local::LocalItem {
            id: entry.id.clone(), playable: true,
        }).collect::<Vec<_>>();
        session.context.update(local::OwnerToken(root.generation), &local::CatalogSnapshot {
            source_id: &root.id, authority_ref: root.authority_ref, source_generation: root.generation,
            state: local::ScanState::Complete, items: &items,
        }).unwrap();
        session.context.note_intent(local::OwnerToken(root.generation), local::LocalIntent::Selected(&entry.id)).unwrap();
    }
    runtime.catalog = catalog;
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.selected = Some(entry.id.clone());
    runtime.continuity = continuity::Continuity::new(runtime.generation, None);
    runtime.continuity.user_selected(runtime.generation, &entry.id);
    let installed = runtime.playing.clone().unwrap();
    assert!(playing_matches(&installed, &app));
    (directory, app, root)
}

fn assert_label_without_source_changes(app: &mut App, expected: &str) {
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let catalog = runtime.catalog.clone();
    let selection = runtime.playing.as_ref().unwrap().selection_id.clone();
    let identity = runtime.playing.as_ref().unwrap().identity.clone();
    publish(app);
    assert_eq!(projection(app).collection_label, expected);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.catalog, catalog);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, selection);
    assert_eq!(runtime.playing.as_ref().unwrap().identity, identity);
}

#[test]
fn local_root_programme_uses_held_folder_with_explicit_or_profile_binding() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (_directory, mut app, root) = local_fixture("first.mp4");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_eq!(runtime.active().unwrap().kind, ProviderKind::M3u);
    assert_eq!(runtime.playing.as_ref().unwrap().entry.category, "local1");
    assert_eq!(runtime.current_local_root(
        runtime.playing.as_ref().unwrap().local_selection.as_ref().unwrap()
    ).unwrap().id, root.id);
    assert_label_without_source_changes(&mut app, "Movies");
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().title, "first.mp4");
    assert_eq!(projection(&app).next.unwrap().title, "next.mp4");

    app.ott.as_mut().unwrap().playing.as_mut().unwrap().local_selection = None;
    assert_label_without_source_changes(&mut app, "Movies");
    assert_eq!(projection(&app).next.unwrap().title, "next.mp4");
}

#[test]
fn local_root_programme_preserves_nested_and_nonlocal_collection_labels() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (_directory, mut app, _) = local_fixture("Series/first.mp4");
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().entry.category, "Series");
    assert_label_without_source_changes(&mut app, "Series");
    drop(app);
    let mut nonlocal = fixture();
    assert_label_without_source_changes(&mut nonlocal, "Collection");
}

#[test]
fn local_root_programme_keeps_single_file_grant_fallback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (_directory, mut app, root) = local_fixture("first.mp4");
    let path = std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4");
    std::fs::write(&path, b"fixture").unwrap();
    let file = BoundSource {
        id: "single".into(), name: "Single file display".into(), kind: local::SourceKind::File,
        available: true, authority_ref: local::AuthorityRef(2), generation: root.generation,
        canonical_path: Some(path.to_str().unwrap().into()),
    };
    let source = LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(), file.clone(), &mut || true,
    ).unwrap();
    assert!(source.is_live());
    assert!(source.root_handle().is_none());
    let runtime = app.ott.as_mut().unwrap();
    runtime.local_inventory.install(vec![source], root.generation);
    let selected = runtime.bind_local_selection(&file).unwrap();
    assert!(runtime.current_local_source(&selected).is_some());
    assert!(runtime.current_local_root(&selected).is_none());
    let playing = runtime.playing.as_mut().unwrap();
    playing.local_selection = Some(selected);
    playing.profile.source = file.canonical_path.unwrap();
    assert_label_without_source_changes(&mut app, "local1");
}

#[test]
fn local_root_programme_uses_playing_owner_and_never_repairs_stale_selection() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (directory, mut app, root) = local_fixture("first.mp4");
    let other = BoundSource {
        id: "other".into(), name: "Other folder".into(), kind: local::SourceKind::Folder,
        available: true, authority_ref: local::AuthorityRef(3), generation: root.generation,
        canonical_path: Some(directory.folder("Other")),
    };
    let runtime = app.ott.as_mut().unwrap();
    let first = runtime.local_inventory.sources[0].clone();
    runtime.local_inventory.install(
        vec![first, local_integration::acquire_root(other.clone())], root.generation,
    );
    let other_profile = transient::source_profile(&other).unwrap();
    runtime.saved.profiles.active_profile = Some(other_profile.id.clone());
    runtime.saved.profiles.profiles.push(other_profile);
    runtime.transient.accepted = None;
    runtime.catalog.entries[0].category = "Other browsing collection".into();
    runtime.catalog.entries[0].title = "Other browsing title".into();
    assert_ne!(runtime.effective_profile().unwrap().source, runtime.playing.as_ref().unwrap().profile.source);
    assert_label_without_source_changes(&mut app, "Movies");
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().title, "first.mp4");

    // An explicit selection for one root cannot borrow the profile's other root.
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().profile.source = other.canonical_path.unwrap();
    assert_label_without_source_changes(&mut app, "local1");
    let runtime = app.ott.as_mut().unwrap();
    runtime.playing.as_mut().unwrap().profile.source = root.canonical_path.unwrap();
    runtime.playing.as_mut().unwrap().local_selection.as_mut().unwrap().generation += 1;
    assert!(runtime.local_profile_selection(&runtime.playing.as_ref().unwrap().profile).is_some());
    assert_label_without_source_changes(&mut app, "local1");
}

#[test]
fn local_root_programme_keeps_fallback_when_source_or_root_label_is_untrusted() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for case in ["missing", "withdrawn", "empty_name", "control_name", "invalid_relative"] {
        let (_directory, mut app, mut root) = local_fixture("first.mp4");
        let runtime = app.ott.as_mut().unwrap();
        match case {
            "missing" => runtime.local_inventory.install(Vec::new(), root.generation),
            "withdrawn" => runtime.local_inventory.sources[0].withdraw(),
            "empty_name" | "control_name" => {
                root.name = if case == "empty_name" { "" } else { "Movies\nprivate" }.into();
                runtime.local_inventory.install(vec![local_integration::acquire_root(root.clone())], root.generation);
            }
            "invalid_relative" => runtime.playing.as_mut().unwrap().entry.media = MediaLocator::Local {
                relative_path: "C:private.mp4".into(),
            },
            _ => unreachable!(),
        }
        assert_label_without_source_changes(&mut app, "local1");
    }
}

    use super::*;
    use plx_media::{ff::hls_presentation::HlsPlayback, route};

    fn admitted(playback: &HlsPlayback) -> route::PlaybackSession {
        let mut ps = route::PlaybackSession::IDLE;
        route::reset_player_control_for_test(&ps);
        route::request_external_source(&mut ps, &playback.media_info_for_test(),
            plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
            "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
        route::reset_player_control_for_test(&ps);
        ps
    }
    #[test]
    fn hls_choices_do_not_borrow_embedded_rows_or_mutate_checkmarks_on_ok() {
        let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
        let ps = admitted(&HlsPlayback::fixture_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 0, Vec::new());
        assert_eq!(menu.ids(), vec![TrackRow::HlsAudio(0), TrackRow::HlsAudio(1)]);
        assert_ne!(TrackRow::HlsAudio(0).key(), TrackRow::Audio(0).key());
        assert!(menu.focus_id(TrackRow::HlsAudio(1)));
        assert!(matches!(menu.on_ok_addressed(meta.view()), TrackOk::Commit { commit: TrackCommit::Hls(_), .. }));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::HlsAudio(0)));
        assert_eq!(menu.audio_stream_id(meta.view()), 0);
        menu.focus_tab(&ps, meta.view(), 1);
        assert!(menu.ids().is_empty(), "no unsupported subtitle or Plex controls");
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
    }
    #[test]
    fn publication_refreshes_selected_row_and_reopen_invalidates_address() {
        let _serial = plx_base::testlock::serial();
    let _globals = route::HlsTrackRequest::test_globals_guard();
        let playback = HlsPlayback::fixture_for_test();
        let mut ps = admitted(&playback);
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 0, Vec::new());
        let source = TrackCommitSource::capture(&ps);
        let catalog = route::hls_track_catalog(&ps).unwrap();
        let choice = catalog.choice(catalog.rows()[1].id).unwrap();
        let request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
        let prepared = request.prepared_for_test();
        route::complete_hls_track_request(&mut ps, &request, prepared).unwrap();
        assert!(menu.poll_hls_catalog(&ps, meta.view()));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::HlsAudio(1)));
        assert!(source.matches(&ps), "selection does not change presentation identity");
        let other = admitted(&playback);
        assert!(!source.matches(&other), "same URL does not preserve playback authority");
        assert!(menu.poll_hls_catalog(&other, meta.view()));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::HlsAudio(0)));
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
    }

    #[test]
    fn hls_subtitles_have_owned_rows_off_and_shared_text_appearance() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let catalog = route::hls_track_catalog(&ps).unwrap();
        assert_eq!(catalog.subtitle_rows().len(), 2);
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        assert_eq!(menu.ids(), vec![TrackRow::SubOff, TrackRow::HlsSubtitle(0),
            TrackRow::HlsSubtitle(1), TrackRow::Style]);
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::SubOff));
        assert_ne!(TrackRow::HlsSubtitle(0).key(), TrackRow::HlsAudio(0).key());
        assert_ne!(TrackRow::HlsSubtitle(0).key(), TrackRow::Sub(0).key());
        assert!(menu.focus_id(TrackRow::HlsSubtitle(1)));
        let TrackOk::Commit { commit: TrackCommit::Hls(pick), .. } = menu.on_ok_addressed(meta.view())
            else { panic!("owned HLS subtitle choice") };
        assert_eq!(pick.target(), route::HlsTrackTarget::Subtitle(Some(catalog.subtitle_rows()[1].id)));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::SubOff), "pick must not publish a checkmark");
        assert!(menu.focus_id(TrackRow::SubOff));
        let TrackOk::Commit { commit: TrackCommit::Hls(off), .. } = menu.on_ok_addressed(meta.view())
            else { panic!("Off remains an intent even while already selected") };
        assert_eq!(off.target(), route::HlsTrackTarget::Subtitle(None));
        assert!(menu.focus_id(TrackRow::Style));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Navigated);
        assert!(menu.focus_id(TrackRow::OpenField(StyleField::Font)));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Navigated);
        let alternate = (menu.appearance.font.index() as usize + 1) % StyleField::Font.rungs();
        assert!(menu.focus_id(TrackRow::Choice(StyleField::Font, alternate)));
        assert!(matches!(menu.on_ok_addressed(meta.view()),
            TrackOk::Commit { commit: TrackCommit::TextAppearance(_), keep_open: true }));
    }

    #[test]
    fn hls_subtitle_rejection_keeps_prior_checkmark_and_published_pick_refreshes() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let mut ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let catalog = route::hls_track_catalog(&ps).unwrap();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        assert!(menu.focus_id(TrackRow::HlsSubtitle(0)));
        let TrackOk::Commit { commit, keep_open } = menu.on_ok_addressed(meta.view()) else { panic!("subtitle") };
        menu.show_commit_failure(TrackCommitFailure::Unavailable, commit, keep_open);
        assert_eq!(menu.pages.top(), Some(TrackPage::CommitFailure));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::SubOff));
        assert!(menu.pop(&ps, meta.view()));
        let choice = catalog.subtitle_choice(Some(catalog.subtitle_rows()[0].id)).unwrap();
        let request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
        route::complete_hls_track_request(&mut ps, &request, request.prepared_for_test()).unwrap();
        assert!(menu.poll_hls_catalog(&ps, meta.view()));
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::HlsSubtitle(0)));
        assert!(menu.focus_id(TrackRow::Style));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Navigated);
        let off = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(None).unwrap()).unwrap();
        route::complete_hls_track_request(&mut ps, &off, off.prepared_for_test()).unwrap();
        assert!(menu.poll_hls_catalog(&ps, meta.view()));
        assert_eq!(menu.pages.top(), Some(TrackPage::Style), "publication must not replace the open style form");
        assert!(menu.ids().contains(&TrackRow::OpenField(StyleField::Font)));
    }

    #[test]
    fn hls_subtitle_app_labels_fit_all_shipped_languages() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let mut ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut failures = Vec::new();
        for selected in [false, true] {
            if selected {
                let catalog = route::hls_track_catalog(&ps).unwrap();
                let choice = catalog.subtitle_choice(Some(catalog.subtitle_rows()[0].id)).unwrap();
                let request = route::begin_hls_track_request(&mut ps, &choice).unwrap();
                route::complete_hls_track_request(&mut ps, &request, request.prepared_for_test()).unwrap();
            }
            for language in plx_platform::i18n::SHIPPED {
                let _language = plx_platform::i18n::language_on_this_thread_for_test(language);
                let menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
                failures.extend(menu.form.table.app_fit_failures(plx_ui::table::MENU_MAX_W, language.tag()));
                failures.extend(menu.form.table.app_fit_failures_hugged(language.tag()));
            }
        }
        plx_ui::table::assert_no_fit_failures(&failures);
    }

    #[test]
    fn hls_subtitle_and_off_canon_are_distinct_and_audio_canon_is_unchanged() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
        let catalog = route::hls_track_catalog(&ps).unwrap();
        let fingerprint = |commit: TrackCommit| {
            let mut canon = Canon::new();
            commit.write_feedback_canon(&mut canon);
            canon.finish()
        };
        let audio_id = catalog.rows()[0].id;
        let audio = fingerprint(TrackCommit::Hls(catalog.choice(audio_id).unwrap()));
        let mut old_audio = Canon::new();
        old_audio.u32(10).u32(audio_id.opaque_key());
        assert_eq!(audio, old_audio.finish());
        let off = fingerprint(TrackCommit::Hls(catalog.subtitle_choice(None).unwrap()));
        let first = fingerprint(TrackCommit::Hls(catalog.subtitle_choice(Some(catalog.subtitle_rows()[0].id)).unwrap()));
        let second = fingerprint(TrackCommit::Hls(catalog.subtitle_choice(Some(catalog.subtitle_rows()[1].id)).unwrap()));
        assert_ne!(off, first);
        assert_ne!(first, second);
        assert_ne!(audio, off);
        assert_ne!(audio, first);
    }

    #[test]
    fn hls_audio_rendition_labels_use_source_text_layout_policy() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let ps = admitted(&HlsPlayback::fixture_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let menu = TrackMenuState::new(&ps, meta.view(), 0, Vec::new());
        let index = menu.form.index_of(&TrackRow::HlsAudio(0)).unwrap();
        let row = menu.form.table.sections.iter().flat_map(|section| &section.rows).nth(index).unwrap();
        assert_eq!(row.label_origin, plx_ui::table::Origin::Server);
        assert_eq!(row.detail_origin, plx_ui::table::Origin::Server);
        assert!(!row.label.is_empty());
        assert!(!row.detail.is_empty());
        assert!(row.checked);
    }

    #[test]
    fn hls_subtitle_forced_badge_uses_retained_manifest_declaration() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let ps = admitted(&HlsPlayback::fixture_with_subtitles_for_test());
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        for (ordinal, forced) in [(0, false), (1, true)] {
            let index = menu.form.index_of(&TrackRow::HlsSubtitle(ordinal)).unwrap();
            let row = menu.form.table.sections.iter().flat_map(|section| &section.rows).nth(index).unwrap();
            assert_eq!(row.badges.iter().any(|badge| matches!(badge, plx_ui::table::Badge::Forced)), forced);
        }
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::SubOff));
    }

    #[test]
    fn manifest_subtitles_and_same_language_observed_pids_are_distinct_picker_rows() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let playback = HlsPlayback::fixture_live_with_subtitles_for_test();
        let scope = plx_media::ff::hls_observed::Scope::new("https://example.invalid/master.m3u8",
            "https://example.invalid/video.m3u8", 1, 0).unwrap();
        let observed = plx_media::ff::hls_observed::Snapshot::fixture_for_test(scope);
        let mut info = playback.media_info_for_test();
        info.tracks = observed.inventory().clone();
        let mut ps = route::PlaybackSession::IDLE;
        route::reset_player_control_for_test(&ps);
        route::request_external_source(&mut ps, &info,
            plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
            "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
        route::reset_player_control_for_test(&ps);
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 0, Vec::new());
        assert_eq!(menu.ids(), vec![TrackRow::Audio(0), TrackRow::Audio(1)]);
        assert_eq!(menu.selected_hls_row(), Some(TrackRow::Audio(0)));
        assert!(!route::external_tracks(&ps).unwrap().is_complete());
        assert!(menu.focus_id(TrackRow::Audio(1)));
        assert!(matches!(menu.on_ok_addressed(meta.view()), TrackOk::Commit { commit: TrackCommit::External(_), .. }));
        menu.focus_tab(&ps, meta.view(), 1);
        assert_eq!(menu.ids().iter().filter(|id| **id == TrackRow::SubOff).count(), 1);
        assert!(menu.ids().contains(&TrackRow::HlsSubtitle(0)));
        assert!(menu.ids().contains(&TrackRow::HlsSubtitle(1)));
        assert!(menu.ids().contains(&TrackRow::Sub(0)), "unsupported observed teletext remains visible without becoming selectable");
        assert!(matches!(menu.pick_external_track(plx_media::ff::inspect::TrackKind::Subtitle, Some(0)), TrackOk::Inert));
    }

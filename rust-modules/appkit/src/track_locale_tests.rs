use super::*;
use super::tests::{store_with, store_with_audio, stream};
use plx_platform::i18n::{language_on_this_thread_for_test, Preference};

// Isolate the existing container-name input and restore it even on an assertion failure.
struct Names(plx_media::player::TrackNames);
impl Names {
    fn empty() -> Self {
        Self(std::mem::replace(&mut *plx_media::player::SHARED.track_names.lock().unwrap(),
            plx_media::player::TrackNames::new()))
    }
}
impl Drop for Names {
    fn drop(&mut self) {
        *plx_media::player::SHARED.track_names.lock().unwrap() = std::mem::take(&mut self.0);
    }
}

fn row(menu: &TrackMenuState, id: TrackRow) -> &Row {
    let i = menu.form.index_of(&id).expect("retained row");
    menu.form.table.sections.iter().flat_map(|s| &s.rows).nth(i).unwrap()
}
fn fingerprint(menu: &TrackMenuState) -> String {
    let mut w = plx_machine::machine::Canon::new();
    menu.canon(&mut w);
    format!("{:?}", w.finish())
}

#[test]
fn locale_relabels_audio_from_retained_facts_without_changing_raw_names_or_selection() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with_audio(vec![
        metadata::Stream { codec: "aac".into(), channels: 2, default: true, ..Default::default() },
        metadata::Stream { lang: "Unknown".into(), title: "My custom mix".into(), ..Default::default() },
    ]);
    let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
    menu.focus_row(menu.form.index_of(&TrackRow::Audio(1)).unwrap() as c_int);
    let before = fingerprint(&menu);
    let ids = menu.ids();
    let old = row(&menu, TrackRow::Audio(0)).label.clone();
    let (raw_label, old_detail) = {
        let raw = row(&menu, TrackRow::Audio(1));
        (raw.label.clone(), raw.detail.clone())
    };
    assert_eq!(raw_label, "Unknown");
    assert_eq!(old_detail,
        plx_platform::i18n::msg::widgets_tracks_converted_by_server("My custom mix"));
    drop(store);
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_ne!(row(&menu, TrackRow::Audio(0)).label, old);
    assert_eq!(row(&menu, TrackRow::Audio(0)).label,
        plx_platform::i18n::msg::widgets_tracks_original(plx_platform::i18n::msg::widgets_tracks_unknown()));
    assert!(row(&menu, TrackRow::Audio(0)).detail.contains(plx_platform::i18n::msg::widgets_tracks_stereo()));
    assert_eq!(row(&menu, TrackRow::Audio(1)).label, raw_label);
    // The provider title stays exact; the surrounding conversion note is localized.
    assert_ne!(row(&menu, TrackRow::Audio(1)).detail, old_detail);
    assert_eq!(row(&menu, TrackRow::Audio(1)).detail,
        plx_platform::i18n::msg::widgets_tracks_converted_by_server("My custom mix"));
    assert_eq!(menu.ids(), ids);
    assert_eq!(fingerprint(&menu), before);
}

#[test]
fn locale_relabels_other_languages_without_resorting_or_reassigning_slot_keys() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with(vec![
        stream(11, 0, "", "", "First mix"),
        stream(12, 1, "", "", "Second mix"),
        stream(21, 2, "French", "fra", "Studio mix"),
    ]);
    let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
    menu.focus_row(menu.form.index_of(&TrackRow::OpenOther).unwrap() as c_int);
    assert_eq!(menu.on_ok(store.view()), TrackOk::Navigated);
    let unknown = menu.other.iter().find(|l| l.tracks.iter().any(|t| t.i == 0)).unwrap().id;
    menu.focus_row(menu.form.index_of(&TrackRow::OpenLang(unknown)).unwrap() as c_int);
    let keys: Vec<_> = menu.ids().iter().map(|id| id.key().0).collect();
    let before = fingerprint(&menu);
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_eq!(menu.ids().iter().map(|id| id.key().0).collect::<Vec<_>>(), keys);
    assert_eq!(fingerprint(&menu), before);
    assert_eq!(row(&menu, TrackRow::OpenLang(unknown)).label, plx_platform::i18n::msg::widgets_tracks_unknown());
    assert_eq!(row(&menu, TrackRow::Sub(2)).label, "French");
    assert_eq!(menu.on_ok(store.view()), TrackOk::Navigated);
    assert_eq!(menu.page_path(), [TrackPage::OtherLanguages, TrackPage::Language(unknown.stream)]);
    assert_eq!(menu.form.table.title(), Some(plx_platform::i18n::msg::widgets_tracks_unknown()));
    assert_eq!(row(&menu, TrackRow::Sub(0)).label, "First mix");
}

#[test]
fn locale_relabels_style_picker_without_committing_or_resetting_page_and_focus() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with(Vec::new());
    let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
    menu.focus_row(menu.form.index_of(&TrackRow::Style).unwrap() as c_int);
    menu.push(TrackPage::Style);
    menu.focus_row(menu.form.index_of(&TrackRow::OpenField(StyleField::Size)).unwrap() as c_int);
    menu.push(TrackPage::Picker(StyleField::Size));
    let ids = menu.ids();
    let before = fingerprint(&menu);
    let appearance = menu.appearance;
    let old = menu.form.table.title().unwrap().to_string();
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_ne!(menu.form.table.title(), Some(old.as_str()));
    assert_eq!(menu.ids(), ids);
    assert_eq!(fingerprint(&menu), before);
    assert_eq!(menu.appearance, appearance);
    assert_eq!(menu.page_path(), [TrackPage::Style, TrackPage::Picker(StyleField::Size)]);
}

#[test]
fn locale_relabels_commit_failure_without_losing_the_exact_retry() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with(Vec::new());
    let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
    let commit = TrackCommit::SubtitleTone(SubtitleTone::Silver);
    menu.show_commit_failure(TrackCommitFailure::SaveFailed, commit.clone(), true);
    let before = fingerprint(&menu);
    let old = row(&menu, TrackRow::CommitRetry).label.clone();
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_ne!(row(&menu, TrackRow::CommitRetry).label, old);
    assert_eq!(fingerprint(&menu), before);
    assert_eq!(menu.on_ok(store.view()), TrackOk::Commit { commit, keep_open: true });
}

#[test]
fn locale_relabels_external_catalog_without_rebinding_its_choice() {
    use plx_media::ff::inspect::{InspectedTrack, TrackInventory, TrackKind};
    use plx_media::route::{ExternalTracks, TrackAvailability};
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with(Vec::new());
    let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
    let track = InspectedTrack {
        kind: TrackKind::Audio, stream_index: 3, ordinal: 0, codec: "aac".into(),
        language: "".into(), title: "User title".into(), channels: 2, sample_rate: 48000,
        bit_depth: 16, decoder_available: true, disposition_known: false, default: false, forced: false,
    };
    let catalog = ExternalTracks::new(TrackInventory::from_partial(vec![track], vec![]),
        vec![TrackAvailability::Supported], vec![], None).unwrap();
    menu.install_external_catalog(catalog.clone());
    menu.rebuild(&ps, store.view(), 0, false);
    let before = fingerprint(&menu);
    let old = row(&menu, TrackRow::Audio(0)).label.clone();
    let choice = menu.on_ok(store.view());
    assert!(matches!(&choice, TrackOk::Commit { commit: TrackCommit::External(_), .. }));
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_ne!(row(&menu, TrackRow::Audio(0)).label, old);
    assert!(row(&menu, TrackRow::Audio(0)).detail.contains("User title"));
    assert!(menu.external.as_ref().unwrap().same_session(&catalog));
    assert_eq!(fingerprint(&menu), before);
    assert_eq!(menu.on_ok(store.view()), choice);
}

#[test]
fn locale_relabels_hls_unavailable_note_without_changing_rendition_authority() {
    use plx_media::{ff::hls_presentation::HlsPlayback, route};
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    let en = language_on_this_thread_for_test(Preference::En);
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    let playback = HlsPlayback::fixture_for_test();
    route::request_external_source(&mut ps, &playback.media_info_for_test(),
        plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    let store = store_with(Vec::new());
    let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
    let catalog = menu.hls.as_ref().unwrap().clone();
    assert!(menu.focus_id(TrackRow::HlsAudio(1)));
    let choice = menu.on_ok_addressed(store.view());
    let before = fingerprint(&menu);
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_eq!(fingerprint(&menu), before);
    assert!(menu.hls.as_ref().unwrap().same_session(&catalog));
    assert_eq!(menu.on_ok_addressed(store.view()), choice);
    menu.focus_tab(&ps, store.view(), 1);
    let es_note = menu.form.table.sections[0].rows[0].label.clone();
    drop(_es);
    let _en = language_on_this_thread_for_test(Preference::En);
    menu.relabel();
    assert!(menu.ids().is_empty());
    assert_ne!(menu.form.table.sections[0].rows[0].label, es_note);
    assert!(menu.hls.as_ref().unwrap().same_session(&catalog));
    route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
}

#[test]
fn locale_refreshes_subtitle_root_but_does_not_translate_server_named_unknown() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let store = store_with(vec![
        stream(11, 0, "", "eng", ""),
        stream(12, 1, "Unknown", "fra", "User subtitle"),
    ]);
    let mut menu = TrackMenuState::new(&ps, store.view(), 1, vec!["eng".into(), "fra".into()]);
    assert!(menu.focus_id(TrackRow::Sub(1)));
    let before = fingerprint(&menu);
    let old_header = menu.form.table.sections[0].header.clone();
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    assert_ne!(menu.form.table.sections[0].header, old_header);
    assert_eq!(row(&menu, TrackRow::Sub(0)).label, plx_platform::i18n::msg::widgets_tracks_unknown());
    assert_eq!(row(&menu, TrackRow::Sub(1)).label, "Unknown");
    assert_eq!(row(&menu, TrackRow::Sub(1)).detail, "User subtitle");
    assert_eq!(fingerprint(&menu), before);
}

#[test]
fn locale_relabels_only_region_fallback_and_preserves_same_spelling_provider_title() {
    let _serial = plx_base::testlock::serial();
    let _names = Names::empty();
    let en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut inferred = stream(11, 0, "Spanish", "spa", "");
    inferred.language_tag = "es-419".into();
    let mut authored = stream(12, 1, "Spanish", "spa", "Latin America");
    authored.language_tag = "es-419".into();
    let store = store_with(vec![inferred, authored]);
    let mut menu = TrackMenuState::new(&ps, store.view(), 1, vec!["spa".into()]);
    let before = fingerprint(&menu);
    let keys = menu.keys();
    assert!(row(&menu, TrackRow::Sub(0)).label.contains("Latin America"));
    assert!(row(&menu, TrackRow::Sub(1)).label.contains("Latin America"));
    drop(en);
    let _es = language_on_this_thread_for_test(Preference::Es);
    menu.relabel();
    let expected = track_label::region_detail("es-419").unwrap();
    assert_ne!(expected, "Latin America");
    assert!(row(&menu, TrackRow::Sub(0)).label.contains(&expected));
    assert!(row(&menu, TrackRow::Sub(1)).label.contains("Latin America"));
    assert_eq!(menu.keys(), keys);
    assert_eq!(fingerprint(&menu), before);
}

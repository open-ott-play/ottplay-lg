//! Native non-Plex track rows in the shared picker. The catalog, not Plex metadata,
//! owns their labels, capabilities, selections and opaque playback identity.

use super::*;
use plx_media::ff::inspect::{InspectedTrack, TrackKind};
use plx_media::route::{ExternalTracks, LocalSubtitleTrack, TrackAvailability};

fn language(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("und") || raw.eq_ignore_ascii_case("zxx") {
        return plx_platform::i18n::msg::widgets_tracks_unknown().to_string();
    }
    let Some(key) = metadata::lang_key(raw) else {
        return raw.to_string();
    };
    let label = plx_plex::plex::languages::label(&key);
    if label == key.as_ref() {
        raw.to_string()
    } else {
        label.to_string()
    }
}

fn track_row(track: &InspectedTrack, selected: bool, available: bool) -> Row {
    use plx_platform::i18n::msg;
    let mut details = Vec::new();
    if !track.title.trim().is_empty() {
        details.push(track.title.trim().to_string());
    }
    let mut codec = friendly_codec(&track.codec);
    if track.kind == TrackKind::Audio && track.channels > 0 {
        let channels = match track.channels {
            1 => msg::widgets_tracks_mono().to_string(),
            2 => msg::widgets_tracks_stereo().to_string(),
            count => msg::widgets_tracks_channels(count),
        };
        if !codec.is_empty() {
            codec.push(' ');
        }
        codec.push_str(&channels);
    }
    if !codec.is_empty() {
        details.push(codec);
    }
    details.push(msg::widgets_tracks_track_ordinal(
        i64::from(track.ordinal) + 1,
    ));
    if !available {
        details.push(msg::widgets_tracks_unavailable().to_string());
    }
    let mut row = Row::new(language(&track.language))
        .detail(details.join(" · "))
        .checked(selected);
    // A container's default flag is not an original-language claim. Unknown
    // dispositions must not acquire a Forced badge either.
    if track.disposition_known && track.forced {
        row = row.badge(Badge::Forced);
    }
    row
}

fn local_row(track: &LocalSubtitleTrack, selected: bool) -> Row {
    Row::new(language(&track.language))
        .detail(format!(
            "{} · {}",
            track.title.trim(),
            friendly_codec(track.format.codec())
        ))
        .checked(selected)
}

fn has_subtitle(catalog: &ExternalTracks) -> bool {
    selected_subtitle(catalog).is_some() || catalog.selected_local_subtitle().is_some()
}

fn selected_subtitle(catalog: &ExternalTracks) -> Option<&InspectedTrack> {
    let ordinal = catalog.selected_subtitle()?;
    (catalog.availability(TrackKind::Subtitle, ordinal) == TrackAvailability::Supported)
        .then(|| catalog.inventory().subtitles.get(ordinal as usize))
        .flatten()
}

impl TrackMenuState {
    pub(super) fn install_external_catalog(&mut self, catalog: ExternalTracks) {
        self.active_audio = catalog.selected_audio().unwrap_or(-1);
        self.active_sub = catalog.selected_subtitle().unwrap_or(-1);
        self.renderer = catalog
            .selected_local_subtitle()
            .and_then(|target| {
                catalog
                    .local_subtitles()
                    .iter()
                    .find(|track| &track.target == target)
            })
            .map(|track| SubRenderer::of_codec(track.format.codec()))
            .or_else(|| {
                selected_subtitle(&catalog).map(|track| SubRenderer::of_codec(&track.codec))
            })
            .unwrap_or(SubRenderer::Text);
        self.root_tracks = catalog
            .inventory()
            .subtitles
            .iter()
            .map(|track| track.ordinal as usize)
            .collect();
        self.forced_only = false;
        self.forced_lang.clear();
        self.other.clear();
        self.item_sidecars.clear();
        self.search = None;
        self.search_watching = false;
        self.sub_sig = None;
        self.enhance_shown = None;
        self.enhance_route = None;
        self.enhance_disabled = None;
        self.enhance_subtitle_effect = plx_media::route::SubtitleEffect::None;
        self.sticky_audio_target = None;
        self.external = Some(catalog);
    }

    /// Same-catalog selections change after admission without changing its Arc.
    /// A new catalog, including a reopen of the same URL, invalidates pushed pages.
    pub(super) fn poll_external_catalog(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        meta: metadata::MetadataView<'_>,
    ) -> bool {
        let live = plx_media::route::external_tracks(ps);
        let same = match (self.external.as_ref(), live) {
            (Some(old), Some(new)) => {
                old.same_session(new)
                    && old.selected_audio() == new.selected_audio()
                    && old.selected_subtitle() == new.selected_subtitle()
                    && old.selected_local_subtitle() == new.selected_local_subtitle()
            }
            (None, None) => true,
            _ => false,
        };
        if same {
            return live.is_some();
        }
        let Some(live) = live else {
            self.external = None;
            self.sync_item(ps, meta);
            self.rebuild(ps, meta, self.tab, false);
            return false;
        };
        let identity_changed = self
            .external
            .as_ref()
            .is_none_or(|old| !old.same_session(live));
        let prior_renderer = self.renderer;
        let had_subtitle = self.external.as_ref().is_some_and(has_subtitle);
        self.install_external_catalog(live.clone());
        let has_subtitle = has_subtitle(live);
        let pages_invalid =
            identity_changed || prior_renderer != self.renderer || had_subtitle != has_subtitle;
        if pages_invalid {
            self.pages.clear();
            self.form.table.set_title(None);
            self.motion.cancel_slide();
        }
        if identity_changed {
            self.retire_background_prewarm();
        }
        if let Some(page) = self.pages.top() {
            let form = self.page_form(page);
            self.form.refresh(form);
        } else {
            let form = self.external_form(self.tab);
            let selected = if self.tab == 0 {
                TrackRow::Audio(self.active_audio.max(0) as usize)
            } else {
                self.active_sub_id()
            };
            if identity_changed {
                self.form.set(form, Some(&selected));
            } else {
                self.form.refresh_with(form, None, Some(&selected));
            }
        }
        true
    }

    pub(super) fn external_form(&self, tab: c_int) -> TrackForm {
        use plx_platform::i18n::msg;
        let Some(catalog) = self.external.as_ref() else {
            return Form::new();
        };
        let (kind, rows, selected, heading) = if tab == 0 {
            (
                TrackKind::Audio,
                &catalog.inventory().audio,
                catalog.selected_audio(),
                msg::widgets_tracks_audio(),
            )
        } else {
            (
                TrackKind::Subtitle,
                &catalog.inventory().subtitles,
                catalog.selected_subtitle(),
                msg::widgets_tracks_subtitles(),
            )
        };
        let mut section = FormSection::new(heading);
        let any_supported = rows
            .iter()
            .any(|track| catalog.availability(kind, track.ordinal) == TrackAvailability::Supported);
        let local_selected = catalog.selected_local_subtitle();
        let has_local = !catalog.local_subtitles().is_empty();
        if kind == TrackKind::Subtitle
            && (any_supported || selected.is_some() || has_local || local_selected.is_some())
        {
            section = section.item(
                TrackRow::SubOff,
                RowKind::Choice,
                (),
                Row::new(msg::widgets_tracks_off())
                    .checked(selected.is_none() && local_selected.is_none()),
            );
        }
        for track in rows {
            let available =
                catalog.availability(kind, track.ordinal) == TrackAvailability::Supported;
            let id = if kind == TrackKind::Audio {
                TrackRow::Audio(track.ordinal as usize)
            } else {
                TrackRow::Sub(track.ordinal as usize)
            };
            section = section
                .item(
                    id,
                    RowKind::Choice,
                    (),
                    track_row(track, selected == Some(track.ordinal), available),
                )
                .disabled(!available);
        }
        if kind == TrackKind::Subtitle {
            for (index, track) in catalog.local_subtitles().iter().enumerate() {
                section = section.item(
                    TrackRow::LocalSub(index),
                    RowKind::Choice,
                    (),
                    local_row(track, local_selected == Some(&track.target)),
                );
            }
        }
        if rows.is_empty() && (kind != TrackKind::Subtitle || !has_local) {
            section = section.note(msg::widgets_tracks_unavailable());
        }
        let mut form = Form::new().section(section);
        if kind == TrackKind::Subtitle && has_subtitle(catalog) {
            form = form.section(
                FormSection::new("")
                    .item(
                        TrackRow::Timing,
                        RowKind::Button,
                        (),
                        Row::new(msg::widgets_tracks_timing()).value(format_offset(self.offset_ms)),
                    )
                    .item(
                        TrackRow::Style,
                        RowKind::Nav(TrackPage::Style),
                        (),
                        Row::new(msg::widgets_tracks_style()),
                    ),
            );
        }
        form
    }

    pub(super) fn external_subtitle_active(&self) -> bool {
        self.external.as_ref().is_some_and(has_subtitle)
    }

    pub(super) fn active_local_sub_id(&self) -> Option<TrackRow> {
        let catalog = self.external.as_ref()?;
        let selected = catalog.selected_local_subtitle()?;
        catalog
            .local_subtitles()
            .iter()
            .position(|track| &track.target == selected)
            .map(TrackRow::LocalSub)
    }

    pub(super) fn pick_local_subtitle(&self, index: usize) -> TrackOk {
        let Some(catalog) = self.external.as_ref() else {
            return TrackOk::Inert;
        };
        let Some(track) = catalog.local_subtitles().get(index) else {
            return TrackOk::Inert;
        };
        let Ok(choice) = catalog.local_choice(&track.target) else {
            return TrackOk::Inert;
        };
        // Reselect retries the same bounded read after an earlier read/parse failure.
        // Only the native owner knows whether that asset currently has usable cues.
        TrackOk::Commit {
            commit: TrackCommit::External(choice),
            keep_open: false,
        }
    }

    pub(super) fn pick_external_track(&self, kind: TrackKind, ordinal: Option<usize>) -> TrackOk {
        let Some(catalog) = self.external.as_ref() else {
            return TrackOk::Inert;
        };
        let ordinal = match ordinal {
            Some(value) => match i32::try_from(value) {
                Ok(value) => Some(value),
                Err(_) => return TrackOk::Inert,
            },
            None => None,
        };
        let Ok(choice) = catalog.choice(kind, ordinal) else {
            return TrackOk::Inert;
        };
        let current = match kind {
            TrackKind::Audio => catalog.selected_audio(),
            TrackKind::Subtitle => catalog.selected_subtitle(),
        };
        if current == ordinal
            && (kind != TrackKind::Subtitle || catalog.selected_local_subtitle().is_none())
        {
            return TrackOk::Dismiss;
        }
        TrackOk::Commit {
            commit: TrackCommit::External(choice),
            keep_open: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn admitted_external_audio_is_visible_without_plex_metadata() {
        let _guard = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut session = plx_media::route::PlaybackSession::IDLE;
        plx_media::route::reset_player_control_for_test(&session);
        let info = plx_media::ff::inspect::MediaInfo {
            resolved_source: None,
            hls_playback: None,
            tracks: plx_media::ff::inspect::TrackInventory::from_partial(
                vec![plx_media::ff::inspect::InspectedTrack {
                    kind: plx_media::ff::inspect::TrackKind::Audio,
                    stream_index: 1,
                    ordinal: 0,
                    codec: "aac".into(),
                    language: "eng".into(),
                    title: String::new(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_depth: 16,
                    default: false,
                    forced: false,
                    disposition_known: false,
                    decoder_available: true,
                }],
                Vec::new(),
            ),
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1280,
            height: 720,
            fps: 24.0,
            duration_ns: 60_000_000_000,
            audio_channels: 2,
            audio_sample_rate: 48_000,
            audio_bit_depth: 16,
            dovi: metadata::Dovi::NONE,
        };
        let admitted = plx_media::route::request_external_source(
            &mut session,
            &info,
            plx_media::ff::ExternalSource::Local("/media/example.mp4".into()),
            "Example",
            "Local",
            plx_platform::devcaps::assumed_shared(),
        );
        assert!(
            admitted.is_ok(),
            "fixture must enter a real admitted external route"
        );
        let metadata = plx_data::stores::metadata::MetadataStore::default();
        assert!(
            metadata.view().playing().is_none(),
            "external sources have no Plex item"
        );
        let menu = TrackMenuState::new(&session, metadata.view(), 0, Vec::new());
        let audio_rows = (0..menu.form.table.n_rows() as usize)
            .filter(|&i| matches!(menu.form.id_at(i), Some(TrackRow::Audio(_))))
            .count();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        assert_eq!(
            audio_rows, 1,
            "an admitted AAC track must be visible without a Plex item"
        );
    }

    use plx_media::ff::inspect::{InspectedTrack, MediaInfo, TrackInventory, TrackKind};
    use plx_media::route::{ExternalTrackApplyError, ExternalTrackChoice, ExternalTrackError};
    use std::sync::atomic::Ordering::Relaxed;

    struct Globals {
        audio: i32,
        subtitle: i32,
        audio_switch: u8,
        audio_pick_ms: i64,
        offset: i64,
        restore_bitmaps: Option<Box<dyn FnOnce()>>,
    }
    impl Globals {
        fn capture() -> Self {
            let shared = &plx_media::player::SHARED;
            let bitmaps = std::mem::take(&mut *shared.sub_bitmaps.lock().unwrap());
            Self {
                audio: shared.desired_audio_idx.load(Relaxed),
                subtitle: shared.desired_sub_idx.load(Relaxed),
                audio_switch: shared.audio_switch.load(Relaxed),
                audio_pick_ms: shared.audio_pick_ms.load(Relaxed),
                offset: plx_media::player::subtitle_offset_ms(),
                restore_bitmaps: Some(Box::new(move || {
                    *plx_media::player::SHARED.sub_bitmaps.lock().unwrap() = bitmaps;
                })),
            }
        }
    }
    impl Drop for Globals {
        fn drop(&mut self) {
            plx_media::route::reset_player_control_for_test(
                &plx_media::route::PlaybackSession::IDLE,
            );
            let shared = &plx_media::player::SHARED;
            shared.desired_audio_idx.store(self.audio, Relaxed);
            shared.desired_sub_idx.store(self.subtitle, Relaxed);
            shared.audio_switch.store(self.audio_switch, Relaxed);
            shared.audio_pick_ms.store(self.audio_pick_ms, Relaxed);
            plx_media::player::restore_subtitle_offset(self.offset);
            if let Some(restore) = self.restore_bitmaps.take() {
                restore();
            }
        }
    }

    fn track(kind: TrackKind, index: i32, ordinal: i32, codec: &str) -> InspectedTrack {
        InspectedTrack {
            kind,
            stream_index: index,
            ordinal,
            codec: codec.into(),
            language: if ordinal == 0 { "eng" } else { "spa" }.into(),
            title: format!("Native track {ordinal}"),
            channels: if kind == TrackKind::Audio { 2 } else { 0 },
            sample_rate: if kind == TrackKind::Audio { 48_000 } else { 0 },
            bit_depth: 16,
            decoder_available: true,
            disposition_known: false,
            default: true,
            forced: true,
        }
    }

    fn multitrack() -> MediaInfo {
        MediaInfo {
            resolved_source: None,
            hls_playback: None,
            tracks: TrackInventory::from_partial(
                vec![
                    track(TrackKind::Audio, 1, 0, "aac"),
                    track(TrackKind::Audio, 3, 1, "aac"),
                ],
                vec![
                    track(TrackKind::Subtitle, 2, 0, "subrip"),
                    track(TrackKind::Subtitle, 4, 1, "ass"),
                    track(TrackKind::Subtitle, 5, 2, "hdmv_pgs_subtitle"),
                    track(TrackKind::Subtitle, 6, 3, "unknown"),
                ],
            ),
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1280,
            height: 720,
            fps: 24.0,
            duration_ns: 60_000_000_000,
            audio_channels: 2,
            audio_sample_rate: 48_000,
            audio_bit_depth: 16,
            dovi: metadata::Dovi::NONE,
        }
    }

    fn admit(info: &MediaInfo, hls: bool) -> plx_media::route::PlaybackSession {
        let mut ps = plx_media::route::PlaybackSession::IDLE;
        plx_media::route::reset_player_control_for_test(&ps);
        let source = if hls {
            plx_media::ff::ExternalSource::Hls("https://example.invalid/media.m3u8".into())
        } else {
            plx_media::ff::ExternalSource::Local("/media/example.mkv".into())
        };
        assert_eq!(
            plx_media::route::request_external_source(
                &mut ps,
                info,
                source,
                "Native film",
                "Local",
                plx_platform::devcaps::assumed_shared()
            ),
            Ok(())
        );
        // Settle the pure controller only. No engine, file read or network request is launched.
        plx_media::route::reset_player_control_for_test(&ps);
        assert!(plx_media::route::external_tracks(&ps).is_some());
        ps
    }

    fn row(menu: &TrackMenuState, id: TrackRow) -> &Row {
        let i = menu.form.index_of(&id).expect("row must exist");
        menu.form
            .table
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .nth(i)
            .unwrap()
    }
    fn press(menu: &mut TrackMenuState, id: TrackRow, meta: metadata::MetadataView<'_>) -> TrackOk {
        let i = menu
            .form
            .index_of(&id)
            .expect("press must target a real row");
        menu.focus_row(i as c_int);
        assert_eq!(menu.selected_id(), Some(id));
        menu.on_ok(meta)
    }
    fn choice(result: TrackOk) -> ExternalTrackChoice {
        match result {
            TrackOk::Commit {
                commit: TrackCommit::External(choice),
                ..
            } => choice,
            other => panic!("expected bound external choice, got {other:?}"),
        }
    }
    fn tick(
        menu: &mut TrackMenuState,
        ps: &plx_media::route::PlaybackSession,
        meta: metadata::MetadataView<'_>,
    ) {
        menu.update(0.016, &plx_ui::fixture::FixtureMeasure, ps, meta);
    }
    fn apply(ps: &mut plx_media::route::PlaybackSession, choice: &ExternalTrackChoice) {
        plx_media::route::reset_player_control_for_test(ps);
        assert_eq!(plx_media::route::apply_external_track(ps, choice), Ok(true));
    }

    #[test]
    fn external_catalog_wins_over_stale_plex_metadata_and_unknown_dispositions() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let ps = admit(&multitrack(), false);
        let mut item = metadata::PlayingItem::with_subs(vec![metadata::Stream {
            id: 901,
            lang: "Stale Plex subtitle".into(),
            codec: "srt".into(),
            ..Default::default()
        }]);
        item.audio = vec![metadata::Stream {
            id: 902,
            lang: "Stale Plex audio".into(),
            ..Default::default()
        }];
        let mut store = plx_data::stores::metadata::MetadataStore::default();
        assert!(
            store.run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
                Some(item)
            ))
        );
        let mut menu = TrackMenuState::new_with(&ps, store.view(), 0, vec!["eng".into()], true);
        assert_eq!(row(&menu, TrackRow::Audio(0)).label, "English");
        // Code-only container tags use the shared catalog's native display names.
        assert_eq!(row(&menu, TrackRow::Audio(1)).label, "Español");
        assert!(
            row(&menu, TrackRow::Audio(0)).badges.is_empty(),
            "unknown flags cannot imply Original or Forced"
        );
        assert!(row(&menu, TrackRow::Audio(0)).checked);
        assert_eq!(
            menu.audio_stream_id(store.view()),
            0,
            "external playback has no Plex stream identity"
        );
        assert!(menu.form.index_of(&TrackRow::Boost).is_none());
        assert!(menu.form.index_of(&TrackRow::Loudness).is_none());
        menu.focus_tab(&ps, store.view(), 1);
        assert!(menu.form.index_of(&TrackRow::Sub(3)).is_some());
        assert!(menu.form.index_of(&TrackRow::SubForcedOnly).is_none());
        assert!(menu.form.index_of(&TrackRow::OpenSearch).is_none());
        assert!(menu.search().is_none());
        assert_eq!(menu.sub_stream_id(store.view()), 0);
        assert!(menu
            .form
            .table
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .all(|row| !row.label.contains("Stale Plex")));
    }

    #[test]
    fn same_catalog_audio_acknowledgement_refreshes_checkmark_without_stealing_focus() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let initial = plx_media::route::external_tracks(&ps).unwrap().clone();
        let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
        let requested = choice(press(&mut menu, TrackRow::Audio(1), store.view()));
        assert_eq!(requested.kind(), TrackKind::Audio);
        assert_eq!(requested.ordinal(), Some(1));
        assert!(
            row(&menu, TrackRow::Audio(0)).checked,
            "a request does not pretend to be admitted"
        );
        apply(&mut ps, &requested);
        assert!(plx_media::route::external_tracks(&ps)
            .unwrap()
            .same_session(&initial));
        menu.focus_row(menu.form.index_of(&TrackRow::Audio(0)).unwrap() as c_int);
        tick(&mut menu, &ps, store.view());
        assert_eq!(
            menu.selected_id(),
            Some(TrackRow::Audio(0)),
            "background ACK preserves focus"
        );
        assert_eq!(menu.active_audio(), 1);
        assert!(row(&menu, TrackRow::Audio(1)).checked);
        assert!(!row(&menu, TrackRow::Audio(0)).checked);
    }

    #[test]
    fn external_subtitle_off_and_renderer_capabilities_follow_admitted_selection() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
        assert!(row(&menu, TrackRow::SubOff).checked);
        assert!(menu.form.index_of(&TrackRow::Style).is_none());
        assert!(row(&menu, TrackRow::Sub(3)).dim);
        assert_eq!(
            press(&mut menu, TrackRow::Sub(3), store.view()),
            TrackOk::Inert
        );
        assert_eq!(
            menu.commit_sub_track(usize::MAX, store.view()),
            TrackOk::Inert,
            "an overflowing ordinal must not become Off"
        );
        for (ordinal, renderer) in [
            (0, SubRenderer::Text),
            (1, SubRenderer::Styled),
            (2, SubRenderer::Image),
        ] {
            let requested = choice(menu.commit_sub_track(ordinal, store.view()));
            apply(&mut ps, &requested);
            tick(&mut menu, &ps, store.view());
            assert_eq!(menu.active_sub(), ordinal as i32);
            assert_eq!(menu.renderer, renderer);
            assert!(row(&menu, TrackRow::Sub(ordinal)).checked);
            assert_eq!(
                press(&mut menu, TrackRow::Style, store.view()),
                TrackOk::Navigated
            );
            for field in [
                StyleField::Font,
                StyleField::Size,
                StyleField::Position,
                StyleField::TextColor,
                StyleField::Outline,
                StyleField::Background,
            ] {
                assert_eq!(
                    row(&menu, TrackRow::OpenField(field)).dim,
                    renderer != SubRenderer::Text
                );
            }
            assert!(!row(&menu, TrackRow::OpenField(StyleField::Color)).dim);
            // The next renderer change is observed while Style is still open;
            // it must invalidate that page before its old controls can be used.
        }
        let off = choice(menu.pick_external_track(TrackKind::Subtitle, None));
        apply(&mut ps, &off);
        tick(&mut menu, &ps, store.view());
        assert!(menu.pages.is_empty());
        assert!(row(&menu, TrackRow::SubOff).checked);
        assert!(menu.form.index_of(&TrackRow::Style).is_none());
        assert_eq!(menu.active_sub(), -1);
    }

    #[test]
    fn reopening_same_source_invalidates_old_choice_and_style_page() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
        let subtitles = choice(press(&mut menu, TrackRow::Sub(0), store.view()));
        apply(&mut ps, &subtitles);
        tick(&mut menu, &ps, store.view());
        assert_eq!(
            press(&mut menu, TrackRow::Style, store.view()),
            TrackOk::Navigated
        );
        let stale = choice(menu.pick_external_track(TrackKind::Audio, Some(1)));
        let mut replacement = admit(&multitrack(), false);
        assert_eq!(
            plx_media::route::apply_external_track(&mut replacement, &stale),
            Err(ExternalTrackApplyError::InvalidChoice(
                ExternalTrackError::StaleSession
            ))
        );
        tick(&mut menu, &replacement, store.view());
        assert!(menu.pages.is_empty());
        assert!(row(&menu, TrackRow::SubOff).checked);
        assert_eq!(menu.active_audio(), 0);
    }

    #[test]
    fn known_hls_aac_audio_uses_shared_picker_and_keeps_subtitles_unavailable() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let ps = admit(&multitrack(), true);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
        assert!(!row(&menu, TrackRow::Audio(1)).dim);
        let pick = choice(press(&mut menu, TrackRow::Audio(1), store.view()));
        assert_eq!(pick.kind(), TrackKind::Audio);
        assert_eq!(pick.ordinal(), Some(1));
        menu.focus_tab(&ps, store.view(), 1);
        assert!(row(&menu, TrackRow::Sub(0)).dim);
        assert!(menu.form.index_of(&TrackRow::Style).is_none());
    }

    #[test]
    fn unsupported_hls_switches_show_disabled_rows_without_dead_style_actions() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut info = multitrack();
        info.tracks.audio[1].language = info.tracks.audio[0].language.clone();
        info.tracks.audio[1].title = info.tracks.audio[0].title.clone();
        let ps = admit(&info, true);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 0, Vec::new());
        assert!(
            row(&menu, TrackRow::Audio(0)).checked,
            "default may play even when switching is unavailable"
        );
        assert!(row(&menu, TrackRow::Audio(0)).dim);
        assert_eq!(
            press(&mut menu, TrackRow::Audio(1), store.view()),
            TrackOk::Inert
        );
        menu.focus_tab(&ps, store.view(), 1);
        assert!(row(&menu, TrackRow::Sub(0)).dim);
        assert!(menu.form.index_of(&TrackRow::SubOff).is_none());
        assert!(menu.form.index_of(&TrackRow::Style).is_none());
        assert!(menu.form.index_of(&TrackRow::Timing).is_none());
    }

    fn local(
        format: plx_media::route::SidecarFormat,
        title: &str,
    ) -> plx_media::route::LocalSubtitleTrack {
        plx_media::route::LocalSubtitleTrack {
            target: Default::default(),
            format,
            title: title.into(),
            language: "eng".into(),
        }
    }
    fn publish_local(
        ps: &mut plx_media::route::PlaybackSession,
        rows: Vec<plx_media::route::LocalSubtitleTrack>,
        complete: bool,
    ) {
        let identity = plx_media::route::playback_identity(ps).unwrap();
        let catalog = plx_media::route::external_tracks(ps).unwrap().clone();
        plx_media::route::install_external_local_subtitles(
            ps, &identity, &catalog, rows, complete, 128, 64,
        )
        .unwrap();
    }

    #[test]
    fn local_subtitle_has_distinct_row_and_off_remains_effective_during_incomplete_discovery() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        let local = local(plx_media::route::SidecarFormat::Srt, "Film English");
        publish_local(&mut ps, vec![local.clone()], false);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
        assert!(menu.form.index_of(&TrackRow::Sub(0)).is_some());
        assert!(menu.form.index_of(&TrackRow::LocalSub(0)).is_some());
        assert_ne!(TrackRow::Sub(0).key(), TrackRow::LocalSub(0).key());
        assert!(row(&menu, TrackRow::SubOff).checked);
        let selected = choice(press(&mut menu, TrackRow::LocalSub(0), store.view()));
        assert_eq!(selected.ordinal(), None, "Local is not a demux ordinal");
        assert_eq!(selected.local_target(), Some(&local.target));
        apply(&mut ps, &selected);
        tick(&mut menu, &ps, store.view());
        assert!(row(&menu, TrackRow::LocalSub(0)).checked);
        assert!(!row(&menu, TrackRow::SubOff).checked);
        assert_eq!(menu.active_sub_id(), TrackRow::LocalSub(0));
        assert_eq!(
            press(&mut menu, TrackRow::Timing, store.view()),
            TrackOk::OpenTiming
        );
        assert_eq!(
            choice(press(&mut menu, TrackRow::LocalSub(0), store.view())).local_target(),
            Some(&local.target),
            "reselect must retry a failed Local read"
        );
        let off = choice(press(&mut menu, TrackRow::SubOff, store.view()));
        assert!(off.local_target().is_none());
        assert!(off.ordinal().is_none());
        apply(&mut ps, &off);
        tick(&mut menu, &ps, store.view());
        assert!(row(&menu, TrackRow::SubOff).checked);
        plx_media::route::forget_external_source(&mut ps);
    }

    #[test]
    fn local_ass_uses_existing_renderer_controls_and_replacement_rejects_old_choice() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        publish_local(
            &mut ps,
            vec![local(plx_media::route::SidecarFormat::Ass, "Film Styled")],
            true,
        );
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
        let selected = choice(press(&mut menu, TrackRow::LocalSub(0), store.view()));
        apply(&mut ps, &selected);
        tick(&mut menu, &ps, store.view());
        assert_eq!(menu.renderer, SubRenderer::Styled);
        assert_eq!(
            press(&mut menu, TrackRow::Timing, store.view()),
            TrackOk::OpenTiming
        );
        assert_eq!(
            press(&mut menu, TrackRow::Style, store.view()),
            TrackOk::Navigated
        );
        let replacement = local(plx_media::route::SidecarFormat::Srt, "Film Plain");
        publish_local(&mut ps, vec![replacement.clone()], true);
        assert_eq!(
            plx_media::route::apply_external_track(&mut ps, &selected),
            Err(ExternalTrackApplyError::InvalidChoice(
                ExternalTrackError::StaleSession
            ))
        );
        tick(&mut menu, &ps, store.view());
        assert!(menu.pages.is_empty());
        assert!(row(&menu, TrackRow::SubOff).checked);
        assert_eq!(
            choice(press(&mut menu, TrackRow::LocalSub(0), store.view())).local_target(),
            Some(&replacement.target)
        );
        plx_media::route::forget_external_source(&mut ps);
    }

    #[test]
    fn removed_local_rows_cannot_be_selected_by_old_menu_index() {
        let _serial = plx_base::testlock::serial();
        let _globals = Globals::capture();
        let mut ps = admit(&multitrack(), false);
        publish_local(
            &mut ps,
            vec![local(plx_media::route::SidecarFormat::Vtt, "Film Web")],
            true,
        );
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, store.view(), 1, Vec::new());
        publish_local(&mut ps, Vec::new(), true);
        tick(&mut menu, &ps, store.view());
        assert!(menu.form.index_of(&TrackRow::LocalSub(0)).is_none());
        assert_eq!(menu.pick_local_subtitle(0), TrackOk::Inert);
        plx_media::route::forget_external_source(&mut ps);
    }
}

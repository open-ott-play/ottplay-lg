//! HLS rendition choices belong to a retained presentation, not embedded stream ordinals.
use super::*;
use plx_media::route::HlsTrackCatalog;

impl TrackMenuState {
    pub(super) fn install_hls_catalog(&mut self, catalog: HlsTrackCatalog) {
        self.external = None;
        self.active_audio = -1;
        self.active_sub = -1;
        self.renderer = SubRenderer::Text;
        self.root_tracks.clear();
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
        self.hls = Some(catalog);
    }

    pub(super) fn selected_hls_row(&self) -> Option<TrackRow> {
        let catalog = self.hls.as_ref()?;
        if self.tab == 0 {
            catalog.rows().iter().position(|row| row.selected).map(TrackRow::HlsAudio)
        } else {
            let rows = catalog.subtitle_rows();
            if rows.is_empty() { return None; }
            Some(rows.iter().position(|row| row.selected).map_or(TrackRow::SubOff, TrackRow::HlsSubtitle))
        }
    }

    pub(super) fn poll_hls_catalog(
        &mut self, ps: &plx_media::route::PlaybackSession, meta: metadata::MetadataView<'_>,
    ) -> bool {
        let live = plx_media::route::hls_track_catalog(ps);
        let unchanged = match (&self.hls, &live) {
            (Some(old), Some(new)) => old.same_session(new) && old.rows() == new.rows()
                && old.subtitle_rows() == new.subtitle_rows(),
            (None, None) => true,
            _ => false,
        };
        if unchanged { return live.is_some(); }
        let Some(live) = live else {
            self.hls = None;
            self.sync_item(ps, meta);
            self.rebuild(ps, meta, self.tab, false);
            return false;
        };
        let changed = self.hls.as_ref().is_none_or(|old| !old.same_session(&live));
        self.install_hls_catalog(live);
        if changed {
            self.pages.clear();
            self.form.table.set_title(None);
            self.motion.cancel_slide();
            self.retire_background_prewarm();
        }
        let form = self.pages.top().map_or_else(|| self.hls_form(self.tab), |page| self.page_form(page));
        let selected = self.pages.is_empty().then(|| self.selected_hls_row()).flatten();
        if changed { self.form.set(form, selected.as_ref()); }
        else { self.form.refresh_with(form, None, selected.as_ref()); }
        true
    }

    pub(super) fn hls_form(&self, tab: c_int) -> TrackForm {
        use plx_platform::i18n::msg;
        let Some(catalog) = &self.hls else { return Form::new(); };
        let mut section = FormSection::new(if tab == 0 {
            msg::widgets_tracks_audio()
        } else { msg::widgets_tracks_subtitles() });
        if tab != 0 {
            let rows = catalog.subtitle_rows();
            if rows.is_empty() { return Form::new().section(section.note(msg::widgets_tracks_unavailable())); }
            section = section.item(TrackRow::SubOff, RowKind::Choice, (),
                Row::new(msg::widgets_tracks_off()).checked(catalog.selected_subtitle().is_none()));
            for (index, row) in rows.iter().enumerate() {
                let name = row.name.trim();
                let language = row.language.trim();
                let label = if !name.is_empty() { name } else if !language.is_empty() { language }
                    else { msg::widgets_tracks_unknown() };
                let mut display = Row::new(label).checked(row.selected);
                if !name.is_empty() || !language.is_empty() { display = display.server_label(); }
                if !language.is_empty() && language != label { display = display.detail(language).server_detail(); }
                section = section.item(TrackRow::HlsSubtitle(index), RowKind::Choice, (), display);
            }
            let mut controls = FormSection::new("");
            if catalog.selected_subtitle().is_some() {
                controls = controls.item(TrackRow::Timing, RowKind::Button, (),
                    Row::new(msg::widgets_tracks_timing()).value(format_offset(self.offset_ms)));
            }
            return Form::new().section(section).section(controls
                .item(TrackRow::Style, RowKind::Nav(TrackPage::Style), (), Row::new(msg::widgets_tracks_style())));
        }
        for (index, row) in catalog.rows().iter().enumerate() {
            let name = row.name.trim();
            let language = row.language.trim();
            let label = if !name.is_empty() { name } else if !language.is_empty() { language }
                else { msg::widgets_tracks_unknown() };
            let mut display = Row::new(label).checked(row.selected);
            if !language.is_empty() && language != label { display = display.detail(language); }
            section = section.item(TrackRow::HlsAudio(index), RowKind::Choice, (), display);
        }
        Form::new().section(section)
    }

    pub(super) fn pick_hls_track(&self, index: usize) -> TrackOk {
        let Some(catalog) = &self.hls else { return TrackOk::Inert; };
        let rows = catalog.rows();
        let Some(row) = rows.get(index) else { return TrackOk::Inert; };
        // Even selecting the current language is an intent: it cancels a prior
        // asynchronous choice. The route, not the checked row, owns admission.
        match catalog.choice(row.id) {
            Ok(choice) => TrackOk::Commit { commit: TrackCommit::Hls(choice), keep_open: false },
            Err(_) => TrackOk::Inert,
        }
    }

    pub(super) fn pick_hls_subtitle(&self, index: Option<usize>) -> TrackOk {
        let Some(catalog) = &self.hls else { return TrackOk::Inert; };
        let rows = catalog.subtitle_rows();
        if rows.is_empty() { return TrackOk::Inert; }
        let target = match index {
            Some(index) => match rows.get(index) { Some(row) => Some(row.id), None => return TrackOk::Inert },
            None => None,
        };
        match catalog.subtitle_choice(target) {
            Ok(choice) => TrackOk::Commit { commit: TrackCommit::Hls(choice), keep_open: false },
            Err(_) => TrackOk::Inert,
        }
    }
}


#[cfg(test)]
#[path = "hls_track_menu_tests.rs"]
mod tests;

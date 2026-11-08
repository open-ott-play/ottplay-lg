//! Presentation snapshots for live locale changes. No playback/session capability is retained.
use super::*;
use plx_data::metadata::sub_layout::SubSection;

pub(super) type RegionLabels = std::collections::BTreeMap<usize, String>;

#[derive(Default)]
pub(super) struct AudioPresentation {
    pub(super) rows: Option<Vec<AudioRow>>,
}

pub(super) struct AudioRow {
    language: String,
    original: bool,
    description: AudioDescription,
    converted: bool,
    decoded_in_app: bool,
    ad: bool,
}

enum AudioDescription {
    Name(String),
    Descriptor { codec: String, layout: String, channels: i64 },
}

impl AudioPresentation {
    /// Called only where the ordinary form builder already reads these admitted inputs.
    pub(super) fn capture(ps: &plx_media::route::PlaybackSession, meta: metadata::MetadataView<'_>) -> Self {
        let rows = tracks(meta).map(|item| {
            let names = plx_media::player::SHARED.track_names.lock().unwrap();
            item.audio.iter().enumerate().map(|(i, s)| {
                let language = if s.lang.is_empty() { plx_platform::i18n::msg::widgets_tracks_unknown() } else { &s.lang };
                let name = track_label::track_name(&s.title,
                    names.audio(plx_data::metadata::audio_ordinal(&item.audio, i)), language);
                let description = if name.is_empty() {
                    AudioDescription::Descriptor { codec: s.codec.clone(), layout: s.layout.clone(), channels: s.channels }
                } else { AudioDescription::Name(name) };
                let (converted, decoded_in_app) = audio_conversion(ps, s);
                AudioRow { language: s.lang.clone(), original: s.default, description,
                    converted, decoded_in_app, ad: s.ad }
            }).collect()
        });
        Self { rows }
    }
}

impl AudioRow {
    pub(super) fn row(&self, checked: bool) -> Row {
        use plx_platform::i18n::msg;
        let language = if self.language.is_empty() { msg::widgets_tracks_unknown() } else { &self.language };
        let label = if self.original { msg::widgets_tracks_original(language) } else { language.to_string() };
        let detail = match &self.description {
            AudioDescription::Name(name) => name.clone(),
            AudioDescription::Descriptor { codec, layout, channels } => audio_descriptor(codec, layout, *channels),
        };
        let detail = audio_sub_line_for(detail, self.converted, self.decoded_in_app);
        let mut row = Row::new(label).checked(checked);
        if !detail.is_empty() { row = row.detail(detail); }
        if self.ad { row = row.badge(Badge::Ad); }
        row
    }
}

pub(super) struct SubtitlePresentation {
    sections: Vec<SubSection>,
    unknown: Vec<usize>,
    regions: RegionLabels,
    burn: Option<plx_media::route::BurnReason>,
    memory_note: bool,
    forced: Option<String>,
    forced_unknown: bool,
}

impl SubtitlePresentation {
    pub(super) fn capture(menu: &TrackMenuState, ps: &plx_media::route::PlaybackSession,
        meta: metadata::MetadataView<'_>, model: &SubModel, regions: RegionLabels) -> Self {
        let unknown = tracks(meta).map(|item| item.subs.iter().enumerate()
            .filter_map(|(i, s)| s.lang.trim().is_empty().then_some(i)).collect()).unwrap_or_default();
        let burn = match plx_media::route::subtitle_presenter(ps) {
            plx_media::route::SubtitlePresenter::ServerBurn(why) => Some(why),
            _ => None,
        };
        Self {
            sections: model.sections.clone(), unknown, regions, burn,
            memory_note: plx_media::route::cur_memory_note(ps),
            forced: TrackMenuState::forced_readout(ps, meta, menu.active_sub, menu.forced_only),
            forced_unknown: menu.forced_only && TrackMenuState::forced_readout_source(ps, meta, menu.active_sub)
                .is_none_or(|s| s.lang.trim().is_empty()),
        }
    }

    /// Called under the SAME normal-build name snapshot used by sub_sections. A raw
    /// title that happens to spell a region name must never become app-owned text.
    pub(super) fn regions(subs: &[metadata::Stream], offered: &[usize],
        names: &plx_data::metadata::track_names::TrackNames) -> RegionLabels {
        offered.iter().filter_map(|&i| {
            let s = subs.get(i)?;
            track_label::region_detail(&s.language_tag)?;
            let language = if s.lang.trim().is_empty() {
                plx_platform::i18n::msg::widgets_tracks_unknown()
            } else { &s.lang };
            let merged = track_label::track_name(&s.title, names.sub(metadata::sub_render_ordinal(subs, i)), language);
            track_label::parse(&merged, language, s.forced, s.sdh).source.is_empty()
                .then(|| (i, s.language_tag.clone()))
        }).collect()
    }

    fn relabel(&mut self, other: &mut [OtherLang]) {
        let label = plx_platform::i18n::msg::widgets_tracks_unknown();
        let unknown = &self.unknown;
        for section in &mut self.sections {
            // A language header was derived from its first track, not from arbitrary text equality.
            let first_unknown = section.rows.iter().find_map(|row| match row {
                SubRow::Flat(t) | SubRow::InLanguage(t) => Some(unknown.binary_search(&t.i).is_ok()),
                _ => None,
            }).unwrap_or(false);
            if first_unknown {
                if let SubHeader::Language { name, .. } = &mut section.header { *name = label.to_string(); }
            }
            for row in &mut section.rows {
                if let SubRow::Flat(t) | SubRow::InLanguage(t) = row {
                    if unknown.binary_search(&t.i).is_ok() { t.lang = label.to_string(); }
                    if let Some(detail) = self.regions.get(&t.i).and_then(|tag| track_label::region_detail(tag)) {
                        t.detail = detail;
                    }
                }
            }
        }
        for group in other {
            if group.tracks.first().is_some_and(|t| unknown.binary_search(&t.i).is_ok()) { group.name = label.to_string(); }
            for track in &mut group.tracks {
                if unknown.binary_search(&track.i).is_ok() { track.lang = label.to_string(); }
                if let Some(detail) = self.regions.get(&track.i).and_then(|tag| track_label::region_detail(tag)) {
                    track.detail = detail;
                }
            }
        }
        if self.forced_unknown { self.forced = Some(sub_layout::forced_only_readout(None, None)); }
    }

    fn form(&self, menu: &TrackMenuState) -> TrackForm {
        let model = SubModel { sections: self.sections.clone(), other: menu.other.clone() };
        let mut form = table_form(&model, menu.active_sub, menu.offset_ms, self.burn, self.memory_note, self.forced.as_deref());
        if menu.search.is_some() {
            form = form.section(FormSection::new("").item(TrackRow::OpenSearch,
                RowKind::Nav(TrackPage::Search), (), Row::new(plx_platform::i18n::msg::widgets_tracks_search_open())));
        }
        form
    }
}

impl TrackMenuState {
    /// Refresh already-admitted presentation only. Row identities/order, page stack, selected
    /// track, pending commit and playback authority do not change, and no new input is queried.
    pub fn relabel(&mut self) {
        if let Some(input) = &mut self.subtitle_presentation {
            input.relabel(&mut self.other);
            // This is a presentation signature too: prevent a locale-only poll mismatch from
            // regrouping/reordering the unchanged catalog on the next normal update.
            if let Some(sig) = &mut self.sub_sig { sig.forced_only.clone_from(&input.forced); }
        }
        let form = if let Some(page) = self.pages.top() {
            self.page_form(page)
        } else if self.hls.is_some() {
            self.hls_form(self.tab)
        } else if self.external.is_some() {
            self.external_form(self.tab)
        } else if self.tab == 0 {
            self.audio_form_presented(&self.audio_presentation,
                (self.enhance_shown, self.enhance_route, self.enhance_disabled, self.enhance_subtitle_effect))
        } else if let Some(input) = &self.subtitle_presentation {
            input.form(self)
        } else { return };
        self.form.refresh(form);
        self.form.table.set_title(self.pages.top().map(|page| self.page_title(page)));
        // Outgoing layers contain old rendered words, never logical navigation state.
        self.motion.cancel_slide();
    }
}

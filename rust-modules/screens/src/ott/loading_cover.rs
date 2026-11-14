//! Opaque, paint-only startup/target cover. The runtime owns restoration and
//! source admission; an outlined footer here does not create a Player session.
use super::*;
use plx_data::metadata::{NowPlaying, OttLivePlaying, OttNowPlaying};

impl OttScreen {
    pub(super) fn loading_cover(&self) -> bool {
        self.editor.is_none() && (self.snapshot.startup_pending
            || self.snapshot.pending_play.is_some() || self.snapshot.page_id == "loading")
    }

    fn loading_playing(&self) -> Option<NowPlaying> {
        // Runtime projection remains available even when the active category or
        // progressive row window does not contain the admitted destination.
        let fallback;
        let display = if let Some(display) = self.snapshot.pending_display.as_ref() { display }
        else {
            let id = self.snapshot.pending_play.as_ref().or(self.snapshot.startup_target.as_ref())?;
            let row = self.snapshot.rows.iter().find(|row| &row.id == id && row.enabled
                && matches!(row.action, OttAction::Play(_)))?;
            fallback = OttPendingDisplay {
                title: row.content_title.as_deref().unwrap_or(&row.title).to_owned(),
                description: row.description.clone(),
                live: self.snapshot.pending_play_ordinal.is_some() || classic::catalogue_live(row),
                ordinal: self.snapshot.pending_play_ordinal,
                guide: row.guide.clone(), poster: row.poster.clone(),
            };
            &fallback
        };
        let title = display.title.clone();
        let live = display.live.then(|| OttLivePlaying {
            channel_title: title.clone(), ordinal: display.ordinal, guide: display.guide.clone(),
        });
        Some(NowPlaying {
            ott: Some(OttNowPlaying { live, archive: None, collection_label: title.clone(),
                confirmed_start_unix_ms: None, next: None, art: display.poster.clone() }),
            title, summary: display.description.clone(), is_episode: false, is_real_episode: false,
            ep_title: String::new(), season: 0, index: 0, year: 0, dur_ms: 0,
            rating: String::new(), thumb: String::new(), detail_rk: String::new(),
        })
    }

    pub(super) fn draw_loading_cover<H: AppLike>(&mut self, frame: &mut DrawFrame<'_, '_, H>) {
        use plx_ui::View;
        if let Some(playing) = self.loading_playing() {
            let now = self.wall_now_s.or_else(|| playing.ott.as_ref()
                .and_then(|ott| ott.live.as_ref()).and_then(|live| live.guide.as_ref()).map(|guide| guide.now_s));
            self.pending_footer.draw(frame.painter, &playing, now, self.snapshot.osd_opacity,
                if self.classic_mode { plx_machine::machine::UiMode::Classic }
                else { plx_machine::machine::UiMode::Modern }, frame.measure);
        }
        plx_ui::placeholder::note(frame.painter, plx_ui::placeholder::Reason::WorkingReadout, msg::browse_library_loading());
        plx_ui::widgets::Spinner::ott_orbit(Rect::FULL.w * 0.5, Rect::FULL.h * 0.5)
            .phase(frame.cx.tick.ms).draw(&plx_ui::Env::inert(), frame.painter);
    }
}

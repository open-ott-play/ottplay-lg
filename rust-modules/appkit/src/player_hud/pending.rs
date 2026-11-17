//! Paint-only target footer before a native playback route exists.
//! The owner supplies source-fenced metadata; this helper grants no admission,
//! input, focus, timer or decoder identity and never borrows outgoing metrics.
use super::{classic, modern, motion::HudMotion, model::OsdOpacity, Busy,
    ControlSlot, Controls, TransportMark};
use plx_data::metadata::NowPlaying;
use plx_machine::machine::{Measure, UiMode};
use plx_ui::{theme, Painter};

pub struct PendingFooter { motion: HudMotion }
impl Default for PendingFooter {
    fn default() -> Self { Self { motion: HudMotion::new() } }
}
impl PendingFooter {
    pub fn draw(&mut self, painter: Painter, playing: &NowPlaying, wall_now_s: Option<i64>,
        opacity: OsdOpacity, mode: UiMode, measure: &dyn Measure) {
        self.draw_with_clock(painter, playing, wall_now_s, None, opacity, mode, measure);
    }

    /// Draw the ordinary information band with a playback clock from this source only.
    pub fn draw_with_clock(&mut self, painter: Painter, playing: &NowPlaying, wall_now_s: Option<i64>,
        decoder: Option<(i64, i64)>, opacity: OsdOpacity, mode: UiMode, measure: &dyn Measure) {
        let slot = ControlSlot::Discs(Controls::NONE);
        let busy = Busy::Readout(plx_ui::widgets::StatusKind::Working, c"");
        self.motion.ott_clock_labels.refresh_raster((0, 0));
        self.motion.ott_clock_labels.refresh_media_rate(None);
        let rail = if mode == UiMode::Modern {
            let geometry = classic::geometry(slot, &self.motion, 0, measure);
            let layout = modern::compose(geometry, slot, &self.motion, None, "",
                Some(playing), wall_now_s, measure);
            modern::draw_panel(painter, &layout, opacity);
            modern::draw_information(painter, &layout, Some(playing), wall_now_s,
                &mut self.motion.ott_clock_labels, measure, busy, TransportMark::None);
            layout.geometry.playbar
        } else {
            let layout = classic::ott_classic_layout(slot, &self.motion, 0, Some(playing), measure);
            let color = theme::with_a(plx_ui::palette::color(plx_ui::palette::Role::Background,
                theme::CLASSIC_BACKGROUND), opacity.background_alpha(0.78));
            painter.rect(layout.panel, 0.0, color, color, 0.0);
            super::ott_footer::draw_classic(painter, playing, playing.ott.as_ref(),
                &mut self.motion.ott_clock_labels, &layout, measure, wall_now_s,
                decoder, busy, TransportMark::None);
            layout.geometry.playbar
        };
        let timeline = decoder.filter(|(pos, dur)| *pos >= 0 && *dur > 0)
            .or_else(|| super::ott_footer::live_timeline(playing, wall_now_s));
        if let Some((pos_ns, dur_ns)) = timeline {
            classic::draw_playbar_track(painter, classic::Playbar {
                pos_ns, dur_ns, knob: classic::Knob::Tick,
                mark: TransportMark::None, now: 0, clock_grow: 0.0,
            }, rail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::metadata::{OttGuideProgramme, OttLivePlaying, OttNowNext, OttNowPlaying};
    use plx_machine::machine::{FontScale, TypographyMeasure};

    fn target(ordinal: Option<u32>) -> NowPlaying {
        let programme = |title: &str, start_s, end_s| OttGuideProgramme {
            title: title.into(), description: String::new(), start_s, end_s,
        };
        NowPlaying {
            title: "Target channel".into(), summary: String::new(),
            ott: Some(OttNowPlaying {
                live: Some(OttLivePlaying { channel_title: "Target channel".into(), ordinal,
                    guide: Some(OttNowNext { now_s: 150,
                        current: Some(programme("Now", 100, 200)),
                        next: Some(programme("Next", 200, 300)) }) }),
                archive: None, collection_label: String::new(), confirmed_start_unix_ms: None,
                next: None, art: None,
            }),
            is_episode: false, is_real_episode: false, ep_title: String::new(),
            season: 0, index: 0, year: 0, dur_ms: 0, rating: String::new(),
            thumb: String::new(), detail_rk: String::new(),
        }
    }

    #[test]
    fn pending_footer_paints_target_identity_and_guide_in_both_layouts_at_all_font_scales() {
        let _guard = plx_base::testlock::serial();
        for mode in [UiMode::Classic, UiMode::Modern] {
            for scale in [20, 100, 200] {
                let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
                measure.set_font_scale(FontScale::from_percent(scale).unwrap());
                let mut footer = PendingFooter::default();
                // Even a retained helper cannot carry the outgoing decoder's
                // raster/rate into an unadmitted target's footer.
                footer.motion.ott_clock_labels.refresh_raster((1920, 1080));
                footer.motion.ott_clock_labels.refresh_media_rate(Some(9_000_000));
                let runs = plx_ui::draw_census::capture_runs(|| footer.draw(Painter::recording(),
                    &target(Some(32)), Some(150), OsdOpacity::default(), mode, &measure));
                for expected in ["32", "Target channel", "Now", "Next"] {
                    assert!(runs.iter().any(|run| run.text == expected),
                        "{mode:?} {scale}% missing {expected}: {runs:?}");
                }
                assert!(!runs.iter().any(|run| run.text.contains("Mbps") || run.text.contains("1920")));
                assert!(runs.iter().all(|run| run.at.y.is_finite()));
                let runs = plx_ui::draw_census::capture_runs(|| footer.draw(Painter::recording(),
                    &target(None), Some(150), OsdOpacity::default(), mode, &measure));
                assert!(!runs.iter().any(|run| run.text == "32" || run.text == "0"));
            }
        }
    }
}

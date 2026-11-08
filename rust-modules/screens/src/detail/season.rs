//! Season-strip layout and delayed loading for [`super::DetailScreen`].

use std::hash::{Hash, Hasher};

use plx_data::metadata::Detail;
use plx_machine::machine::{GroupId, Measure};
use plx_ui::widgets::{self, SelMark, StripLay, TabGround, TabStrip};
use plx_ui::{theme, Painter, Rect};

pub const SEASON_ELEM_RANGE_START: u32 = super::SECTION_BLOCK;
pub const SEASON_ELEM_RANGE_END: u32 = 2 * super::SECTION_BLOCK;
pub const SEASON_GROUP: GroupId = GroupId(1);
pub const ROW_H: f32 = plx_ui::widgets::StatusOverlay::CTRL_H;
pub const SETTLE_S: f32 = 0.2;
/// Content-space x of the first season's LABEL — one pill padding in from the content edge, so the
/// first pill's frame starts exactly on `MARGIN_X` at scroll 0.
const LEAD_LABEL_X: f32 = plx_ui::consts::MARGIN_X + widgets::STRIP_PAD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStep {
    Wait,
    Request(usize),
    Ready,
    Retire,
}

pub fn restore_step(
    detail: Option<&Detail>,
    season_number: Option<i64>,
    requested: bool,
    loading: bool,
) -> RestoreStep {
    let Some(number) = season_number else {
        return RestoreStep::Ready;
    };
    let Some(detail) = detail else {
        return RestoreStep::Wait;
    };
    let Some(index) = detail
        .seasons
        .iter()
        .position(|season| season.index == number)
    else {
        return RestoreStep::Retire;
    };
    if detail.cur_season == index && !loading {
        RestoreStep::Ready
    } else if !requested {
        RestoreStep::Request(index)
    } else if loading {
        RestoreStep::Wait
    } else {
        RestoreStep::Retire
    }
}

pub fn elem(index: usize) -> Option<u32> {
    (index < (SEASON_ELEM_RANGE_END - SEASON_ELEM_RANGE_START) as usize)
        .then_some(SEASON_ELEM_RANGE_START + index as u32)
}

pub fn locate(key: u32) -> Option<usize> {
    (SEASON_ELEM_RANGE_START..SEASON_ELEM_RANGE_END)
        .contains(&key)
        .then(|| (key - SEASON_ELEM_RANGE_START) as usize)
}

pub fn watch_state(season: &plx_data::metadata::Season) -> plx_ui::widgets::PosterMark {
    if season.watched() {
        plx_ui::widgets::PosterMark::Watched
    } else if season.viewed_leaf_count > 0 {
        plx_ui::widgets::PosterMark::InProgress
    } else {
        plx_ui::widgets::PosterMark::None
    }
}

pub struct Metrics {
    dirty: bool,
    fingerprint: u64,
    // Only the plain inputs of the shared season formatter, not the Detail or its media state.
    inputs: Vec<(i64, String)>,
    layout_dirty: bool,
    lays: Vec<StripLay>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            dirty: true,
            fingerprint: 0,
            inputs: Vec::new(),
            layout_dirty: false,
            lays: Vec::new(),
        }
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn locale_changed(&mut self) {
        self.layout_dirty = true;
    }

    pub fn update(&mut self, d: &Detail, measure: &dyn Measure) {
        if self.dirty {
            self.dirty = false;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            d.sid.raw().hash(&mut h);
            d.rk.hash(&mut h);
            for s in &d.seasons {
                s.index.hash(&mut h);
                s.title.hash(&mut h);
            }
            let fp = h.finish().max(1);
            if fp != self.fingerprint {
                self.fingerprint = fp;
                self.inputs = d.seasons.iter().map(|season| (season.index, season.title.clone())).collect();
                self.layout_dirty = true;
            }
        }
        self.prepare(measure);
    }

    /// Remeasure presentation after a locale change even when no metadata event or Tick arrived.
    /// Metadata invalidation stays pending until `update` receives the current Detail.
    pub fn prepare(&mut self, measure: &dyn Measure) {
        if !self.layout_dirty {
            return;
        }
        self.layout_dirty = false;
        self.lays = widgets::strip_layout_measured(
            self.inputs.iter().map(|(index, title)| plx_ui::fmt::season_identity(*index, title)),
            // The LABEL origin: the first pill's FRAME then starts on the page's content edge
            // (`MARGIN_X`), the same leading edge as the hero buttons above it and the season
            // group's own focus extent — the Library's strip lays out from the same origin.
            // Passing `MARGIN_X` here put the first pill's plate `STRIP_PAD` outside that edge
            // (issue 19).
            LEAD_LABEL_X,
            theme::size::BODY,
            widgets::STRIP_GAP,
            measure,
        );
    }

    pub fn lays(&self) -> &[StripLay] {
        &self.lays
    }

    pub fn rect(&self, index: usize, top: f32, scroll: f32) -> Option<Rect> {
        self.lays.get(index).map(|lay| {
            let r = widgets::strip_pill_rect(lay, top, ROW_H);
            Rect::new(r.x - scroll, r.y, r.w, r.h)
        })
    }

    /// The tabs whose pill is on the axis at `scroll`, as an index range. The pills are laid out left
    /// to right, so two binary searches find the range: its cost is the visible count, not the
    /// strip's length. The test is the one `widgets::draw_strip` culls its paint by.
    pub fn visible(&self, scroll: f32) -> std::ops::Range<usize> {
        let left_edge_and_width = |lay: &StripLay| {
            let r = widgets::strip_pill_rect(lay, 0.0, ROW_H);
            (r.x - scroll, r.w.max(1.0))
        };
        let first = self.lays.partition_point(|lay| {
            let (x, w) = left_edge_and_width(lay);
            x + w <= 0.0
        });
        let end = self.lays.partition_point(|lay| left_edge_and_width(lay).0 < plx_ui::consts::SCR_W);
        first..end.max(first)
    }

    pub fn scroll_target(&self, current: f32, index: usize) -> f32 {
        let Some(r) = self.rect(index, 0.0, 0.0) else {
            return current;
        };
        let lo = r.x + r.w + widgets::STRIP_ADVANCE
            - (plx_ui::consts::SCR_W - plx_ui::consts::MARGIN_X);
        let hi = r.x - widgets::STRIP_ADVANCE - plx_ui::consts::MARGIN_X;
        plx_ui::cards::reveal(current, lo, hi, f32::MAX)
    }
}

pub fn draw(
    p: Painter,
    metrics: &Metrics,
    tabs: TabStrip,
    selected: usize,
    focused: Option<usize>,
    scroll: f32,
    pop: f32,
) {
    widgets::draw_strip(p, &tabs, metrics.lays(), 0.0, ROW_H, scroll, TabGround::Plated { pop });
    let _ = (selected, focused);
}

pub fn update_tabs(
    tabs: &mut TabStrip,
    metrics: &Metrics,
    selected: Option<usize>,
    focused: Option<usize>,
    dt: f32,
) {
    tabs.update(
        selected.map(|i| i as i32).unwrap_or(-1),
        focused.map(|i| i as i32).unwrap_or(-1),
        |i| widgets::strip_span(metrics.lays(), i, ROW_H),
        SelMark::Lands,
        dt,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_frozen_season_keys_round_trip() {
        // a list of any length: every tab in the block has its id, and the block ends at its width
        for i in 0..200 {
            assert_eq!(locate(elem(i).unwrap()), Some(i));
        }
        assert_eq!(elem(SEASON_ELEM_RANGE_END as usize - SEASON_ELEM_RANGE_START as usize), None);
    }

    fn detail() -> Detail {
        Detail {
            seasons: vec![
                plx_data::metadata::Season {
                    rk: "s1".into(),
                    index: 1,
                    title: "Season 1".into(),
                    leaf_count: 1,
                    viewed_leaf_count: 0,
                },
                plx_data::metadata::Season {
                    rk: "s3".into(),
                    index: 3,
                    title: "Season 3".into(),
                    leaf_count: 1,
                    viewed_leaf_count: 0,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn the_latch_waits_for_the_show_then_asks_once_and_retires_on_the_season() {
        let d = detail();
        assert_eq!(restore_step(None, Some(3), false, false), RestoreStep::Wait);
        assert_eq!(
            restore_step(Some(&d), Some(3), false, false),
            RestoreStep::Request(1)
        );
        assert_eq!(
            restore_step(Some(&d), Some(3), true, true),
            RestoreStep::Wait
        );
        let mut landed = d;
        landed.cur_season = 1;
        assert_eq!(
            restore_step(Some(&landed), Some(3), true, false),
            RestoreStep::Ready
        );
    }

    #[test]
    fn a_failed_season_fetch_retires_the_latch_instead_of_asking_again() {
        assert_eq!(
            restore_step(Some(&detail()), Some(3), true, false),
            RestoreStep::Retire
        );
    }

    #[test]
    fn a_superseding_open_and_a_missing_season_both_retire_it() {
        assert_eq!(
            restore_step(Some(&detail()), Some(99), false, false),
            RestoreStep::Retire
        );
        assert_eq!(
            restore_step(Some(&detail()), None, false, false),
            RestoreStep::Ready
        );
    }

    /// **Issue 19: the season row starts on the page's content edge.** Before any scrolling the
    /// first pill's PLATE (not its label) stands on `MARGIN_X` — the hero buttons' leading edge
    /// and the season group's own focus extent. It used to stand `STRIP_PAD` (26px) to the left of
    /// it, because the layout origin it was handed was the label's and not the frame's.
    #[test]
    fn the_first_season_pill_starts_on_the_content_edge_at_scroll_zero() {
        let mut metrics = Metrics::new();
        metrics.update(&detail(), &plx_ui::fixture::FixtureMeasure);
        let first = metrics.rect(0, 0.0, 0.0).expect("a first pill");
        assert_eq!(first.x, plx_ui::consts::MARGIN_X, "first pill plate x at scroll 0");
        // and the scroll target for that pill leaves the row where it is
        assert_eq!(metrics.scroll_target(0.0, 0), 0.0);
    }

    #[test]
    fn season_tab_pills_cover_their_note_and_never_overlap() {
        let d = detail();
        let mut metrics = Metrics::new();
        metrics.update(&d, &plx_ui::fixture::FixtureMeasure);
        for pair in metrics.lays().windows(2) {
            let left = widgets::strip_pill_rect(&pair[0], 0.0, ROW_H);
            let right = widgets::strip_pill_rect(&pair[1], 0.0, ROW_H);
            assert!(left.x + left.w + widgets::STRIP_GAP <= right.x + 0.01);
        }
    }

    #[test]
    fn season_watch_state_uses_both_container_endpoints() {
        let mut season = detail().seasons.remove(0);
        season.leaf_count = 2;
        assert_eq!(watch_state(&season), plx_ui::widgets::PosterMark::None);
        season.viewed_leaf_count = 1;
        assert_eq!(
            watch_state(&season),
            plx_ui::widgets::PosterMark::InProgress
        );
        season.viewed_leaf_count = season.leaf_count;
        assert_eq!(
            watch_state(&season),
            plx_ui::widgets::PosterMark::Watched
        );
    }

    /// **A season tab is named by the ONE identity rule the captions use** (`fmt::season_identity`):
    /// its own title, else "Season N", else "Specials" for the season numbered 0. A server that gave
    /// season 0 no title used to put "Season 0" on its tab while the same season's caption said
    /// "Specials".
    #[test]
    fn an_untitled_season_zero_tab_reads_specials_not_season_zero() {
        let mut d = detail();
        d.seasons[0].index = 0;
        d.seasons[0].title = String::new();
        d.seasons[1].title = String::new();
        let mut metrics = Metrics::new();
        metrics.update(&d, &plx_ui::fixture::FixtureMeasure);
        let labels: Vec<_> = metrics.lays().iter().map(|lay| lay.label.to_str().unwrap().to_string()).collect();
        assert_eq!(labels, ["Specials", "Season 3"]);
    }

    #[test]
    fn locale_refresh_remeasures_retained_season_labels_without_metadata() {
        use plx_platform::i18n::{self, Preference};
        let _english = i18n::language_on_this_thread_for_test(Preference::En);
        let measure = plx_ui::fixture::FixtureMeasure;
        let mut d = detail();
        d.seasons[0].index = 0;
        d.seasons[0].title.clear();
        d.seasons[1].title.clear();
        let mut metrics = Metrics::new();
        metrics.update(&d, &measure);
        let fingerprint = metrics.fingerprint;
        let labels = metrics.lays().iter().map(|lay| lay.label.to_str().unwrap().to_owned()).collect::<Vec<_>>();
        let width = metrics.rect(1, 0.0, 0.0).unwrap().w;
        drop(d);
        let _spanish = i18n::language_on_this_thread_for_test(Preference::Es);
        // A covered page gets the presentation hook and prepares again without a Tick or store read.
        metrics.locale_changed();
        metrics.prepare(&measure);
        assert_eq!(metrics.fingerprint, fingerprint);
        assert!(!metrics.dirty);
        let translated = metrics.lays().iter().map(|lay| lay.label.to_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(translated, [plx_ui::fmt::season_identity(0, ""), plx_ui::fmt::season_identity(3, "")]);
        assert_ne!(translated, labels);
        assert_ne!(metrics.rect(1, 0.0, 0.0).unwrap().w, width);
        let expected = widgets::strip_layout_measured(
            translated.into_iter(), LEAD_LABEL_X, theme::size::BODY, widgets::STRIP_GAP, &measure,
        );
        for (actual, expected) in metrics.lays().iter().zip(&expected) {
            assert_eq!(widgets::strip_pill_rect(actual, 0.0, ROW_H), widgets::strip_pill_rect(expected, 0.0, ROW_H));
        }
    }

    #[test]
    fn locale_prepare_preserves_pending_metadata_and_reuses_unchanged_layout() {
        use plx_platform::i18n::{self, Preference};
        let _english = i18n::language_on_this_thread_for_test(Preference::En);
        struct CountMeasure(std::cell::Cell<usize>);
        impl Measure for CountMeasure {
            fn width(&self, text: &std::ffi::CStr, size: i32, bold: bool) -> f32 {
                self.0.set(self.0.get() + 1);
                plx_ui::fixture::FixtureMeasure.width(text, size, bold)
            }
            fn cap_h(&self, size: i32) -> f32 {
                plx_ui::fixture::FixtureMeasure.cap_h(size)
            }
            fn line_h(&self, size: i32) -> f32 {
                plx_ui::fixture::FixtureMeasure.line_h(size)
            }
        }
        let measure = CountMeasure(std::cell::Cell::new(0));
        let mut d = detail();
        let mut metrics = Metrics::new();
        metrics.update(&d, &measure);
        let measured = measure.0.get();
        assert!(measured > 0);
        metrics.prepare(&measure);
        assert_eq!(measure.0.get(), measured);
        metrics.invalidate();
        let fingerprint = metrics.fingerprint;
        let _spanish = i18n::language_on_this_thread_for_test(Preference::Es);
        metrics.locale_changed();
        metrics.prepare(&measure);
        assert!(metrics.dirty, "presentation cannot consume a pending metadata landing");
        assert_eq!(metrics.fingerprint, fingerprint);
        assert_eq!(metrics.lays()[0].label.to_str().unwrap(), "Season 1", "server titles are not translated");
        let measured_after_locale = measure.0.get();
        assert!(measured_after_locale > measured);
        // Even a dirty metadata update with the same fingerprint keeps the prepared layout.
        metrics.update(&d, &measure);
        assert!(!metrics.dirty);
        assert_eq!(measure.0.get(), measured_after_locale);
        d.seasons[0].title = "Server title changed".into();
        metrics.invalidate();
        metrics.update(&d, &measure);
        assert_eq!(metrics.lays()[0].label.to_str().unwrap(), "Server title changed");
    }

    #[test]
    fn locale_prepare_before_metadata_keeps_the_strip_empty() {
        let mut metrics = Metrics::new();
        metrics.locale_changed();
        metrics.prepare(&plx_ui::fixture::FixtureMeasure);
        assert!(metrics.dirty);
        assert!(metrics.inputs.is_empty() && metrics.lays().is_empty());
        assert_eq!(metrics.fingerprint, 0);
    }

    #[test]
    fn season_metrics_rebuild_once_after_a_same_identity_landing() {
        let measure = plx_ui::fixture::FixtureMeasure;
        let mut metrics = Metrics::new();
        let mut d = detail();
        metrics.update(&d, &measure);
        let before = metrics.lays()[0].label.to_str().unwrap().to_string();
        d.seasons[0].title = "Specials".into();
        metrics.update(&d, &measure);
        assert_eq!(metrics.lays()[0].label.to_str().unwrap(), before);
        metrics.invalidate();
        metrics.update(&d, &measure);
        assert_eq!(metrics.lays()[0].label.to_str().unwrap(), "Specials");
    }
}

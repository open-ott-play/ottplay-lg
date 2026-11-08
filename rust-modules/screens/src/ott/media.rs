//! Typed OTT media rendered by the shared native card stack. These borrowed
//! projections never fetch, copy or re-order the runtime's catalogue.
use super::{OttAction, OttMediaKind, OttPoster, OttView};
use std::collections::HashMap;
use plx_machine::machine::{Cx, FocusKey, GroupId};
use plx_ui::cards::{self, CardSource, GridSpec, Kind, SectionSpec, Stack, StackPage, TileLabel};
use plx_ui::screen::{At, Dir, DrawFrame, Focusable, GroupSpec, Part, Placed, Step};
use plx_ui::widgets::Art;
use crate::registry::{AppLike, PageMemory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section { Head, Items }

pub(super) fn presentation(view: &OttView) -> Option<bool> {
    if view.rows.is_empty() { return None; }
    let mut landscape = true;
    for row in &view.rows {
        let media = row.media.as_ref()?;
        if !row.enabled || !matches!(row.action, OttAction::Play(_)) { return None; }
        landscape &= matches!(media.kind, OttMediaKind::LiveChannel | OttMediaKind::Episode);
    }
    Some(landscape)
}

pub(super) fn style(landscape: bool) -> cards::RowStyle {
    if landscape { cards::RowStyle::EPISODE } else { cards::GRID_STYLE }
}

pub(super) fn columns(landscape: bool) -> usize {
    let frame = super::navigation::route().content;
    let style = style(landscape);
    ((frame.w + style.gap) / (style.w + style.gap)).floor().max(1.0) as usize
}

pub(super) struct Page<'a> {
    pub presentation: Option<bool>,
    pub snapshot: &'a OttView,
    pub keys: &'a [u32],
    pub index: &'a HashMap<u32, usize>,
}

pub(super) struct Cards<'a> {
    landscape: bool,
    snapshot: &'a OttView,
    keys: &'a [u32],
    index: &'a HashMap<u32, usize>,
}

impl<H: AppLike<Memory = PageMemory>> CardSource<H> for Cards<'_> {
    fn len(&self) -> usize { self.snapshot.rows.len() }
    fn elem(&self, i: usize) -> u32 { self.keys[i] }
    fn index_of(&self, elem: &u32) -> Option<usize> { self.index.get(elem).copied() }
    fn art(&self, i: usize) -> Art<'_> {
        let landscape = matches!(self.snapshot.rows[i].media.as_ref().map(|media| media.kind), Some(OttMediaKind::LiveChannel | OttMediaKind::Episode));
        let res = if landscape { (640, 360) } else { (420, 630) };
        match &self.snapshot.rows[i].poster {
            Some(OttPoster::Plex { server, key }) => Art::Thumb { sid: *server, key, res },
            Some(OttPoster::Http { url }) => Art::Http { url, res },
            Some(OttPoster::Local { key }) => Art::Local { key },
            None => Art::Poster(None),
        }
    }
    fn label(&self, i: usize) -> TileLabel {
        label(&self.snapshot.rows[i], self.landscape, &crate::registry::captions::policy())
    }
    fn progress(&self, i: usize) -> Option<f32> {
        let media = self.snapshot.rows[i].media.as_ref()?;
        let progress = media.progress?;
        if media.duration_ms.is_some_and(|duration| duration != progress.duration_ms) { return None; }
        (progress.duration_ms > 0 && progress.position_ms > 0 && progress.position_ms < progress.duration_ms)
            .then(|| progress.position_ms as f32 / progress.duration_ms as f32)
    }
    fn overlay(&self, painter: plx_ui::Painter, i: usize, tile: &cards::Tile,
        measure: &dyn plx_machine::machine::Measure) {
        let row = &self.snapshot.rows[i];
        if row.poster.is_none() {
            // An absent image cannot identify the tile. Keep its canonical name
            // on the empty face even when the focused caption is elsewhere.
            plx_ui::widgets::still_line(painter, tile.rect, plx_ui::widgets::PosterMark::None,
                card_title(row), "", false, <Self as CardSource<H>>::progress(self, i).is_some(), measure);
        }
        if row.favorite {
            // The same corner treatment as watched/watchlist marks; follows the
            // drawn tile through focus/press scaling and stays visible at rest.
            plx_ui::widgets::corner_mark(painter, tile.rect, tile.radius,
                plx_ui::icons::Icon::BookmarkFill);
        }
    }
    fn revision(&self) -> Option<u64> { Some(self.snapshot.revision) }
    // Runtime scans append snapshots. There is no unimplemented Load more action.
    fn more(&self) -> bool { false }
}

impl<H: AppLike<Memory = PageMemory>> StackPage<H> for Page<'_> {
    type Key = Section;
    type Cards<'a> = Cards<'a> where Self: 'a;
    fn revision(&self, _cx: &Cx<'_, H>) -> u64 { self.snapshot.revision }
    fn sections(&self, _cx: &Cx<'_, H>, out: &mut Vec<SectionSpec<Section>>) {
        let Some(landscape) = self.presentation else { return; };
        let frame = super::navigation::route().content;
        out.push(SectionSpec::new(Section::Head, Kind::Custom { height: frame.y, focusable: false }, GroupId(2)));
        out.push(SectionSpec::new(Section::Items, Kind::Grid {
            spec: GridSpec::resting(frame.y).columns(columns(landscape), style(landscape), frame.x),
        }, GroupId(0)));
    }
    fn fallback(&self, _cx: &Cx<'_, H>, out: &mut Vec<Section>) { out.push(Section::Items); }
    fn cards<'a>(&'a self, _cx: &'a Cx<'_, H>, section: Section) -> Option<Cards<'a>> {
        (section == Section::Items && self.presentation.is_some()).then_some(Cards {
            landscape: self.presentation == Some(true), snapshot: self.snapshot, keys: self.keys, index: self.index,
        })
    }
    fn card_has_menu(&self, _cx: &Cx<'_, H>, _section: Section, _elem: &u32) -> bool { false }
    fn caption_policy(&self, _section: Section) -> Option<plx_ui::caption_policy::CaptionPolicy> {
        (self.presentation == Some(false)).then(crate::registry::captions::policy)
    }
}

/// Own only the small borrowed page declaration so callers can delegate focus
/// and paint without storing another catalogue or card cursor.
pub(super) struct View<'a> {
    pub stack: &'a Stack<Section>,
    pub page: Page<'a>,
}
impl<H: AppLike<Memory = PageMemory>> Focusable<H> for View<'_> {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        Focusable::<H>::groups(&self.stack.view(&self.page), cx, out);
    }
    fn group_of(&self, key: &u32, cx: &Cx<'_, H>) -> Option<GroupId> {
        Focusable::<H>::group_of(&self.stack.view(&self.page), key, cx)
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, cx: &Cx<'_, H>) -> Step<u32> {
        Focusable::<H>::neighbour(&self.stack.view(&self.page), key, dir, cx)
    }
    fn place(&self, key: &u32, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        Focusable::<H>::place(&self.stack.view(&self.page), key, cx, at)
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, H>) -> FocusKey<u32> {
        Focusable::<H>::reconcile(&self.stack.view(&self.page), want, cx)
    }
    fn seat(&self, group: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<u32> {
        Focusable::<H>::seat(&self.stack.view(&self.page), group, from, cx)
    }
}
impl<H: AppLike<Memory = PageMemory>> Part<H> for View<'_> {
    fn prepare(&mut self, _b: &mut plx_ui::frame::Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, frame: &mut DrawFrame<'_, '_, H>, rect: plx_ui::Rect) {
        let _clip = plx_ui::screen::ClipScope::open_in(frame.painter, rect);
        Part::<H>::draw(&mut self.stack.view(&self.page), frame, rect);
    }
}

/// Preserve the file identity when the poster policy leaves only a details line.
/// A provider/kind label is not a per-file state, so it must not replace that identity.
fn card_title(row: &super::OttRow) -> &str {
    if row.id.starts_with("favorite:") {
        row.content_title.as_deref().filter(|title| !title.trim().is_empty()).unwrap_or(&row.title)
    } else { &row.title }
}

fn label(row: &super::OttRow, landscape: bool, policy: &plx_ui::caption_policy::CaptionPolicy) -> TileLabel {
    // Favorite list ordinals are navigation labels, not part of Modern card titles.
    let title = card_title(row);
    if landscape {
        return crate::registry::captions::still_block(title, &detail(row)).within_tile();
    }
    let kind = row.media.as_ref().map(|media| media.kind);
    let facts = plx_ui::caption_policy::CaptionFacts {
        title: Some(title.to_owned()),
        is_movie: kind == Some(OttMediaKind::Movie), is_show: kind == Some(OttMediaKind::Series),
        duration_min: row.media.as_ref().and_then(|media| media.duration_ms)
            .filter(|duration| *duration >= 60_000).and_then(|duration| u32::try_from(duration / 60_000).ok()),
        ..Default::default()
    };
    let state = (!facts.is_movie && !facts.is_show).then(||
        if !policy.title || row.detail.is_empty() { title.to_owned() } else { row.detail.clone() });
    crate::registry::captions::label(policy, &facts, state, true).within_tile()
}

pub(super) fn detail(row: &super::OttRow) -> String {
    let duration = row.media.as_ref().and_then(|media| media.duration_ms).filter(|duration| *duration > 0);
    let Some(duration) = duration else { return row.detail.clone(); };
    let seconds = duration / 1000;
    let time = if seconds >= 3600 { format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60) }
        else { format!("{}:{:02}", seconds / 60, seconds % 60) };
    if row.detail.is_empty() { time } else { format!("{} · {time}", row.detail) }
}

pub(super) fn status(view: &OttView) -> String {
    // placeholder-exempt: copy builder only; note_status accounts at each visible status paint.
    use plx_platform::i18n::msg;
    let Some(progress) = view.catalog_progress else { return view.status.clone(); };
    let loaded = i64::try_from(progress.loaded).unwrap_or(i64::MAX);
    let count = match progress.total.filter(|total| *total >= progress.loaded) {
        Some(total) => msg::ott_loaded_total(loaded, i64::try_from(total).unwrap_or(i64::MAX)),
        None => msg::ott_loaded_count(loaded),
    };
    let count = if progress.loading && !progress.complete { format!("{} {count}", msg::browse_library_loading()) }
        else { count };
    if view.status.is_empty() { count } else { format!("{}\n{count}", view.status) }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use plx_ui::caption_policy::{CaptionPolicy, DetailsKind};
    fn file() -> super::super::OttRow {
        super::super::OttRow {
            current_programme: None, id: "file-1".into(), title: "7 Fixture One.mp4".into(), content_title: Some("Fixture One.mp4".into()),
            detail: "Local videos".into(), description: String::new(), poster: None, guide: None,
            guide_available: false, records_available: false, media: Some(super::super::OttMedia { kind: OttMediaKind::LocalVideo,
                duration_ms: Some(12_000), progress: None }),
            action: OttAction::Play("file-1".into()), enabled: true, favorite: false,
        }
    }
    fn text(label: &TileLabel) -> Vec<String> {
        [&label.title, &label.caption].into_iter().flatten().map(|s| s.to_string_lossy().into_owned()).collect()
    }
    #[test]
    fn local_card_keeps_file_name_in_the_only_enabled_caption_line() {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
        for kind in [OttMediaKind::LocalVideo, OttMediaKind::Video] {
            let mut row = file(); row.media.as_mut().unwrap().kind = kind;
            for details in [DetailsKind::YearLength, DetailsKind::Genre, DetailsKind::Video, DetailsKind::AgeRatingScore] {
                let rendered = label(&row, false, &CaptionPolicy { title: false, details });
                assert_eq!(text(&rendered), vec!["7 Fixture One.mp4"], "{kind:?} / {details:?}");
                assert!(rendered.glyph);
            }
        }
    }
    #[test]
    fn card_title_policy_and_unknown_title_fallback_remain_explicit() {
        let mut row = file();
        assert_eq!(text(&label(&row, false, &CaptionPolicy { title: true, details: DetailsKind::YearLength })),
            vec!["7 Fixture One.mp4", "Local videos"]);
        assert!(text(&label(&row, false, &CaptionPolicy { title: false, details: DetailsKind::None })).is_empty());
        row.content_title = Some("  ".into());
        assert_eq!(text(&label(&row, false, &CaptionPolicy { title: false, details: DetailsKind::YearLength })),
            vec!["7 Fixture One.mp4"]);
        assert_eq!(text(&label(&file(), true, &CaptionPolicy { title: false, details: DetailsKind::None }))[0], "7 Fixture One.mp4");
    }

    #[test]
    fn every_media_kind_keeps_the_projected_number_and_real_leading_digits() {
        for kind in [OttMediaKind::LiveChannel, OttMediaKind::Episode, OttMediaKind::LocalVideo,
            OttMediaKind::Video, OttMediaKind::Movie, OttMediaKind::Series] {
            for title in ["2001: A Space Odyssey", "007 — A title", "42 кота"] {
                let mut row = file();
                row.id = "filtered-only-visible-row".into();
                row.title = format!("3450  {title}");
                row.content_title = Some(title.into());
                row.media.as_mut().unwrap().kind = kind;
                let landscape = matches!(kind, OttMediaKind::LiveChannel | OttMediaKind::Episode);
                let rendered = label(&row, landscape, &CaptionPolicy { title: true, details: DetailsKind::None });
                let mut expected = vec![format!("3450  {title}")];
                if landscape { expected.push(detail(&row)); }
                assert_eq!(text(&rendered), expected, "{kind:?}: a filtered row keeps its source number");
                assert_eq!(row.content_title.as_deref(), Some(title));
                assert_eq!(row.title, format!("3450  {title}"));
                assert_eq!(row.id, "filtered-only-visible-row");
            }
        }
    }

    #[test]
    fn favorite_cards_keep_the_supplied_label_without_inventing_a_number() {
        for kind in [OttMediaKind::LiveChannel, OttMediaKind::Episode, OttMediaKind::LocalVideo,
            OttMediaKind::Video, OttMediaKind::Movie, OttMediaKind::Series] {
            let mut row = file();
            row.id = "favorite:opaque-identity".into();
            row.title = "2  007 — Favorite title".into();
            row.content_title = Some("007 — Favorite title".into());
            row.favorite = true;
            row.media.as_mut().unwrap().kind = kind;
            let landscape = matches!(kind, OttMediaKind::LiveChannel | OttMediaKind::Episode);
            let rendered = label(&row, landscape, &CaptionPolicy { title: true, details: DetailsKind::None });
            let mut expected = vec!["007 — Favorite title".into()];
            if landscape { expected.push(detail(&row)); }
            assert_eq!(text(&rendered), expected, "{kind:?}");
            assert!(row.favorite);
            assert_eq!(row.id, "favorite:opaque-identity");
        }
    }

    #[test]
    fn projected_numbers_do_not_enable_hidden_poster_lines_or_prefix_detail_metadata() {
        for kind in [OttMediaKind::LocalVideo, OttMediaKind::Video, OttMediaKind::Movie, OttMediaKind::Series] {
            let mut row = file();
            row.media.as_mut().unwrap().kind = kind;
            let hidden = label(&row, false, &CaptionPolicy { title: false, details: DetailsKind::None });
            assert!(text(&hidden).is_empty(), "{kind:?}");
            assert!(!hidden.glyph);
            let policy = CaptionPolicy { title: true, details: DetailsKind::YearLength };
            let numbered = label(&row, false, &policy);
            row.title = row.content_title.clone().unwrap();
            let unnumbered = label(&row, false, &policy);
            assert_eq!(numbered.caption, unnumbered.caption, "{kind:?}: metadata is not a numbered identity");
        }
    }

    struct OverlayHost;
    impl plx_machine::machine::Host for OverlayHost {
        type Arg = crate::registry::AppArg;
        type Fx = crate::registry::AppFx;
        type Msg = crate::registry::AppMsg;
        type Elem = u32;
        type Views<'a> = ();
        type Init = crate::registry::AppArg;
        type Memory = PageMemory;
    }
    fn overlay_fixture(row: super::super::OttRow, focused: bool,
        read: impl FnOnce(&Cards<'_>, &cards::Tile)) {
        let landscape = matches!(row.media.as_ref().unwrap().kind,
            OttMediaKind::LiveChannel | OttMediaKind::Episode);
        let view = OttView { status_working: false, page_id: "channels:fixture".into(), revision: 1, title: String::new(),
            classic_preferences: Default::default(), volume_available: false, category_label: None, description: String::new(), status: String::new(),
            selected_row: Some(row.id.clone()), rows: vec![row], search: None, catalog_progress: None,
            preview_context: None, random_context: None, order_context: None };
        let index = HashMap::from([(1, 0)]);
        let cards = Cards { landscape, snapshot: &view, keys: &[1], index: &index };
        let style = style(landscape);
        let scale = if focused { style.focus_scale } else { 1.0 };
        let tile = cards::Tile { rect: plx_ui::Rect::new(100.0, 100.0, style.w, style.h).scaled(scale),
            radius: style.radius * scale, scale, focused };
        read(&cards, &tile);
    }
    fn overlay_runs(row: super::super::OttRow, focused: bool) -> Vec<plx_ui::draw_census::Run> {
        let mut runs = Vec::new();
        overlay_fixture(row, focused, |cards, tile| {
            runs = plx_ui::draw_census::capture_runs(|| CardSource::<OverlayHost>::overlay(
                cards, plx_ui::Painter::recording(), 0, tile, &plx_ui::fixture::FixtureMeasure));
        });
        runs
    }
    #[test]
    fn no_art_card_title_is_drawn_inside_focused_and_unfocused_tiles() {
        let _lock = plx_base::testlock::serial();
        for kind in [OttMediaKind::LiveChannel, OttMediaKind::Episode, OttMediaKind::LocalVideo,
            OttMediaKind::Video, OttMediaKind::Movie, OttMediaKind::Series] {
            for focused in [false, true] {
                let mut row = file();
                row.title = "37 Film".into();
                row.media.as_mut().unwrap().kind = kind;
                overlay_fixture(row, focused, |cards, tile| {
                    let runs = plx_ui::draw_census::capture_runs(|| CardSource::<OverlayHost>::overlay(
                        cards, plx_ui::Painter::recording(), 0, tile, &plx_ui::fixture::FixtureMeasure));
                    assert_eq!(runs.len(), 1, "kind={kind:?}, focused={focused}: {runs:?}");
                    assert_eq!(runs[0].text, "37 Film", "use the authoritative numbered title");
                    assert!(runs[0].at.x >= tile.rect.x && runs[0].at.x + runs[0].at.w <= tile.rect.x + tile.rect.w);
                    assert!(runs[0].at.y >= tile.rect.y && runs[0].at.y < tile.rect.y + tile.rect.h);
                    assert!(runs[0].bold);
                    assert_eq!(runs[0].size, plx_ui::theme::size::LABEL);
                    assert!(runs[0].fade.is_none() && runs[0].lead.is_none(), "static text, no focus-dependent marquee");
                });
            }
        }
    }
    #[test]
    fn no_art_card_fallback_never_overlays_declared_poster_art() {
        let _lock = plx_base::testlock::serial();
        for poster in [OttPoster::Plex { server: 1, key: "/fixture/poster".into() },
            OttPoster::Http { url: "https://example.invalid/poster.jpg".into() },
            OttPoster::Local { key: plx_machine::machine::PosterKey(1) }] {
            for focused in [false, true] {
                let mut row = file();
                row.poster = Some(poster.clone());
                assert!(overlay_runs(row, focused).is_empty(), "declared artwork keeps its existing presentation");
            }
        }
    }
    #[test]
    fn no_art_card_fallback_elides_and_preserves_favorite_progress_and_caption_policy() {
        let _lock = plx_base::testlock::serial();
        let mut row = file();
        row.title = "3450 A deliberately long movie title that cannot fit inside one narrow poster tile".into();
        row.favorite = true;
        row.media.as_mut().unwrap().progress = Some(super::super::OttMediaProgress {
            position_ms: 3_000, duration_ms: 12_000 });
        assert!(text(&label(&row, false, &CaptionPolicy { title: false, details: DetailsKind::None })).is_empty(),
            "Under posters remains a separate policy; fallback labels the empty tile itself");
        overlay_fixture(row, false, |cards, tile| {
            let runs = plx_ui::draw_census::capture_runs(|| CardSource::<OverlayHost>::overlay(
                cards, plx_ui::Painter::recording(), 0, tile, &plx_ui::fixture::FixtureMeasure));
            assert_eq!(runs.len(), 1);
            assert!(runs[0].text.starts_with("3450 "));
            assert_ne!(runs[0].text, cards.snapshot.rows[0].title);
            assert!(runs[0].at.x + runs[0].at.w <= tile.rect.x + tile.rect.w);
            assert_eq!(CardSource::<OverlayHost>::progress(cards, 0), Some(0.25));
            let icons = plx_ui::draw_census::capture_icons(|| CardSource::<OverlayHost>::overlay(
                cards, plx_ui::Painter::recording(), 0, tile, &plx_ui::fixture::FixtureMeasure));
            assert_eq!(icons.len(), 1, "no new play/watched marker");
            assert_eq!(icons[0].0, plx_ui::icons::Icon::BookmarkFill);
            assert!(icons[0].1.y < tile.rect.y + tile.rect.h * 0.3);
        });
    }

}

/// Account only the status paint, never snapshot construction or source work.
/// The stable key intentionally excludes user/provider text from diagnostics.
pub(super) fn note_status(p: plx_ui::Painter, status: &str, working: bool) {
    if working && !status.trim().is_empty() {
        plx_ui::placeholder::note(p, plx_ui::placeholder::Reason::WorkingReadout, "ott-status");
    }
}

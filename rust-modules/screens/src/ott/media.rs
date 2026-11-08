//! Typed OTT media rendered by the shared native card stack. These borrowed
//! projections never fetch, copy or re-order the runtime's catalogue.
use super::{OttAction, OttMediaKind, OttPoster, OttView};
use std::collections::{HashMap, HashSet};
use plx_machine::machine::{Cx, FocusKey, GroupId};
use plx_ui::cards::{self, CardSource, GridSpec, Kind, SectionSpec, Stack, StackPage, TileLabel};
use plx_ui::screen::{At, Dir, DrawFrame, Focusable, GroupSpec, Part, Placed, Step};
use plx_ui::widgets::Art;
use crate::registry::{AppLike, PageMemory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section { Head, Items, Shelf(u32) }

/// Only occurrence keys and row indices live here; metadata stays in OttView.
pub(super) struct Shelf {
    pub snapshot_index: usize,
    pub group: GroupId,
    pub keys: Vec<u32>,
    pub index: HashMap<u32, usize>,
    pub landscape: bool,
}

pub(super) fn occurrence_id(source: &str, category: &str, item: &str) -> String {
    format!("shelf-item:{}:{source}{}:{category}{}:{item}", source.len(), category.len(), item.len())
}

pub(super) fn group_id(source: &str, category: &str) -> String {
    format!("shelf-group:{}:{source}{}:{category}", source.len(), category.len())
}

impl super::OttScreen {
    pub(super) fn install_shelves(&mut self, snapshot: &OttView) {
        self.shelves.clear();
        self.shelf_index.clear();
        let live = (!self.classic_mode && self.card_presentation.is_some() && valid_shelves(snapshot))
            .then(|| self.shelf_key_plan(snapshot)).flatten();
        self.prune_shelf_keys(live.as_ref());
        if live.is_none() { self.card_focus = None; return; }
        let source = snapshot.shelf_context.as_deref().unwrap_or_default();
        for (snapshot_index, shelf) in snapshot.shelves.iter().enumerate() {
            // Group 0 is the flat form, 1 the header and 2 the inert top spacer.
            let group = GroupId(self.row_key(&group_id(source, &shelf.id)).0 + 3);
            let mut keys = Vec::with_capacity(shelf.rows.len());
            let mut landscape = true;
            for &row_index in &shelf.rows {
                let row = &snapshot.rows[row_index];
                let key = self.row_key(&occurrence_id(source, &shelf.id, &row.id)).0;
                self.media_keys.insert(key, row_index);
                keys.push(key);
                landscape &= row.media.as_ref().is_some_and(|media|
                    matches!(media.kind, OttMediaKind::LiveChannel | OttMediaKind::Episode));
            }
            let index = keys.iter().enumerate().map(|(index, key)| (*key, index)).collect();
            self.shelf_index.insert(group.0, self.shelves.len());
            self.shelves.push(Shelf { snapshot_index, group, keys, index, landscape });
        }
        self.card_focus = self.card_focus.filter(|key|
            self.shelves.iter().any(|shelf| shelf.index.contains_key(key)));
    }

    /// Prove the complete allocation before interning anything. The ordinary
    /// canonical rows and navigation controls are allocated after the shelves;
    /// reserve their missing keys as well so shelves cannot exhaust that space.
    fn shelf_key_plan(&self, snapshot: &OttView) -> Option<HashSet<String>> {
        let source = snapshot.shelf_context.as_deref()?;
        let mut next = self.next_key;
        let mut live = HashSet::new();
        let mut admit = |id: String, group: bool| -> Option<()> {
            let key = if let Some(&key) = self.keys.get(&id) { key } else {
                let key = next;
                next = next.checked_add(1)?;
                key
            };
            // FormTable reserves the upper key band for controls. Both the
            // occurrence key and its offset group must stay in the lower band.
            if key >= crate::registry::BAND { return None; }
            if group { key.checked_add(3).filter(|group| *group < crate::registry::BAND)?; }
            live.insert(id);
            Some(())
        };
        for shelf in &snapshot.shelves {
            admit(group_id(source, &shelf.id), true)?;
            for &index in &shelf.rows {
                admit(occurrence_id(source, &shelf.id, &snapshot.rows[index].id), false)?;
            }
        }
        let controls = super::navigation::CONTEXT_IDS.into_iter()
            .chain(["nav:browse", "nav:refresh", "nav:back"]);
        let mut remaining = 0u32;
        for id in controls {
            if !self.keys.contains_key(id) { remaining = remaining.checked_add(1)?; }
        }
        for row in &snapshot.rows {
            if !self.keys.contains_key(&format!("item:{}", row.id)) {
                remaining = remaining.checked_add(1)?;
            }
        }
        // `next` may equal the exclusive ceiling only after allocating the
        // last legal key. Keep all later canonical/control keys below it too.
        next.checked_add(remaining).filter(|next| *next <= crate::registry::BAND)?;
        Some(live)
    }

    /// Shelf vocabularies track only this publication, including a refused or
    /// empty layout. Canonical rows/control keys retain their existing lifetime.
    fn prune_shelf_keys(&mut self, live: Option<&HashSet<String>>) {
        let focus = self.card_focus;
        let mut retired_focus = false;
        let media_keys = &mut self.media_keys;
        self.keys.retain(|id, key| {
            let shelf = id.starts_with("shelf-item:") || id.starts_with("shelf-group:");
            let keep = !shelf || live.is_some_and(|live| live.contains(id));
            if !keep {
                media_keys.remove(key);
                retired_focus |= focus == Some(*key);
            }
            keep
        });
        if retired_focus { self.card_focus = None; }
    }

    /// A shelf occurrence is a focus identity, never a form or playback identity.
    pub(super) fn canonical_card_key(&self, key: u32) -> u32 {
        self.media_keys.get(&key).and_then(|index| self.snapshot.rows.get(*index))
            .and_then(|row| self.keys.get(&format!("item:{}", row.id))).copied().unwrap_or(key)
    }

    pub(super) fn card_target(&self, key: u32) -> u32 {
        if self.classic_mode || self.browse || self.shelves.is_empty() { return key; }
        if self.shelves.iter().any(|shelf| shelf.index.contains_key(&key)) { return key; }
        let Some(&row) = self.media_keys.get(&key) else { return key; };
        self.card_focus.filter(|focus| self.media_keys.get(focus) == Some(&row))
            .or_else(|| self.shelves.iter().flat_map(|shelf| &shelf.keys)
                .find(|key| self.media_keys.get(key) == Some(&row)).copied()).unwrap_or(key)
    }

    pub(super) fn shelf_page_key(&self, key: u32, direction: i32) -> Option<u32> {
        let shelf = self.shelves.iter().find(|shelf| shelf.index.contains_key(&key))?;
        let index = shelf.index[&key];
        let style = shelf_style(shelf.landscape);
        let count = (super::navigation::route().content.w / (style.w + style.gap)).floor().max(1.0) as usize;
        let target = if direction > 0 { index.saturating_add(count).min(shelf.keys.len() - 1) }
            else if direction < 0 { index.saturating_sub(count) } else { index };
        shelf.keys.get(target).copied()
    }
}

fn valid_shelves(snapshot: &OttView) -> bool {
    if snapshot.shelves.is_empty() || snapshot.shelf_context.is_none()
        || snapshot.page_id.split(':').next() != Some("channels")
        || snapshot.search.as_ref().is_some_and(|search| !search.query.is_empty()) { return false; }
    let limit = plx_data::ott::MAX_CATALOG_ENTRIES;
    if snapshot.rows.len() > limit || snapshot.shelves.len() > limit { return false; }
    let Some(occurrences) = snapshot.shelves.iter().try_fold(0usize,
        |total, shelf| total.checked_add(shelf.rows.len()).filter(|total| *total <= limit)) else { return false; };
    if occurrences < snapshot.rows.len() { return false; }
    let mut identities = HashSet::new();
    if !snapshot.rows.iter().all(|row| identities.insert(row.id.as_str())) { return false; }
    let mut categories = HashSet::new();
    let mut covered = vec![false; snapshot.rows.len()];
    for shelf in &snapshot.shelves {
        if shelf.id.is_empty() || shelf.rows.is_empty() || !categories.insert(&shelf.id) { return false; }
        let mut seen = HashSet::new();
        for &index in &shelf.rows {
            let Some(row) = covered.get_mut(index) else { return false; };
            if !seen.insert(index) { return false; }
            *row = true;
        }
    }
    covered.into_iter().all(|covered| covered)
}

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
    pub shelves: &'a [Shelf],
    pub shelf_index: &'a HashMap<u32, usize>,
}

impl Page<'_> {
    fn shelf(&self, group: u32) -> Option<&Shelf> {
        self.shelves.get(*self.shelf_index.get(&group)?)
    }
}

pub(super) struct Cards<'a> {
    landscape: bool,
    snapshot: &'a OttView,
    keys: &'a [u32],
    index: &'a HashMap<u32, usize>,
    rows: Option<&'a [usize]>,
}

impl Cards<'_> {
    fn row(&self, i: usize) -> &super::OttRow {
        &self.snapshot.rows[self.rows.map_or(i, |rows| rows[i])]
    }
}

impl<H: AppLike<Memory = PageMemory>> CardSource<H> for Cards<'_> {
    fn len(&self) -> usize { self.keys.len() }
    fn elem(&self, i: usize) -> u32 { self.keys[i] }
    fn index_of(&self, elem: &u32) -> Option<usize> { self.index.get(elem).copied() }
    fn art(&self, i: usize) -> Art<'_> {
        let landscape = matches!(self.row(i).media.as_ref().map(|media| media.kind), Some(OttMediaKind::LiveChannel | OttMediaKind::Episode));
        let res = if landscape { (640, 360) } else { (420, 630) };
        match &self.row(i).poster {
            Some(OttPoster::Plex { server, key }) => Art::Thumb { sid: *server, key, res },
            Some(OttPoster::Http { url }) => Art::Http { url, res },
            Some(OttPoster::Local { key }) => Art::Local { key },
            None => Art::Poster(None),
        }
    }
    fn label(&self, i: usize) -> TileLabel {
        label(self.row(i), self.landscape, &crate::registry::captions::policy())
    }
    fn progress(&self, i: usize) -> Option<f32> {
        let media = self.row(i).media.as_ref()?;
        let progress = media.progress?;
        if media.duration_ms.is_some_and(|duration| duration != progress.duration_ms) { return None; }
        (progress.duration_ms > 0 && progress.position_ms > 0 && progress.position_ms < progress.duration_ms)
            .then(|| progress.position_ms as f32 / progress.duration_ms as f32)
    }
    fn overlay(&self, painter: plx_ui::Painter, i: usize, tile: &cards::Tile,
        measure: &dyn plx_machine::machine::Measure) {
        let row = self.row(i);
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
        if self.shelves.is_empty() {
            out.push(SectionSpec::new(Section::Items, Kind::Grid {
                spec: GridSpec::resting(frame.y).columns(columns(landscape), style(landscape), frame.x),
            }, GroupId(0)));
        } else {
            for shelf in self.shelves {
                out.push(SectionSpec::new(Section::Shelf(shelf.group.0), Kind::Shelf {
                    style: shelf_style(shelf.landscape), heading: HEADING_HEIGHT,
                }, shelf.group).seated(plx_ui::screen::Seat::RememberedFirst)
                    .edges([plx_ui::screen::EdgeRule::Geometric, plx_ui::screen::EdgeRule::Geometric,
                        plx_ui::screen::EdgeRule::Stop, plx_ui::screen::EdgeRule::Stop]));
            }
        }
    }
    fn fallback(&self, _cx: &Cx<'_, H>, out: &mut Vec<Section>) {
        if self.shelves.is_empty() { out.push(Section::Items); }
        else { out.extend(self.shelves.iter().map(|shelf| Section::Shelf(shelf.group.0))); }
    }
    fn cards<'a>(&'a self, _cx: &'a Cx<'_, H>, section: Section) -> Option<Cards<'a>> {
        if self.presentation.is_none() { return None; }
        match section {
            Section::Items if self.shelves.is_empty() => Some(Cards {
                landscape: self.presentation == Some(true), snapshot: self.snapshot, keys: self.keys,
                index: self.index, rows: None,
            }),
            Section::Shelf(group) => {
                let shelf = self.shelf(group)?;
                Some(Cards { landscape: shelf.landscape, snapshot: self.snapshot,
                    keys: &shelf.keys, index: &shelf.index,
                    rows: Some(&self.snapshot.shelves[shelf.snapshot_index].rows) })
            }
            _ => None,
        }
    }
    fn card_has_menu(&self, _cx: &Cx<'_, H>, _section: Section, _elem: &u32) -> bool { false }
    fn caption_policy(&self, section: Section) -> Option<plx_ui::caption_policy::CaptionPolicy> {
        let landscape = match section {
            Section::Shelf(group) => self.shelf(group)?.landscape,
            _ => self.presentation?,
        };
        (!landscape).then(crate::registry::captions::policy)
    }
    fn draw_heading(&self, section: Section, frame: &mut DrawFrame<'_, '_, H>, rect: plx_ui::Rect, lift: f32) {
        let Section::Shelf(group) = section else { return; };
        let Some(shelf) = self.shelf(group) else { return; };
        let content = super::navigation::route().content;
        cards::draw_heading(frame.painter.alpha(frame.page_alpha),
            &self.snapshot.shelves[shelf.snapshot_index].title, "", content.x,
            rect.y + plx_ui::theme::space::MD - lift, content.w, frame.measure);
    }
    fn reveal_margin(&self, _section: Section) -> f32 { super::navigation::route().content.y }
    fn shelf_foot(&self, _cx: &Cx<'_, H>, _section: Section) -> f32 { plx_ui::consts::UNDER_LABEL_AIR }
}

const HEADING_HEIGHT: f32 = plx_ui::theme::size::HEADLINE as f32 + 2.0 * plx_ui::theme::space::MD;

fn shelf_style(landscape: bool) -> &'static cards::RowStyle {
    static PORTRAIT: std::sync::OnceLock<cards::RowStyle> = std::sync::OnceLock::new();
    static LANDSCAPE: std::sync::OnceLock<cards::RowStyle> = std::sync::OnceLock::new();
    let slot = if landscape { &LANDSCAPE } else { &PORTRAIT };
    slot.get_or_init(|| {
        let content = super::navigation::route().content;
        cards::RowStyle { margin_x: content.x, viewport_width: Some(content.w), ..style(landscape) }
    })
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
                duration_ms: Some(12_000), progress: None, synopsis: None }),
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
            selected_row: Some(row.id.clone()), shelves: Vec::new(), shelf_context: None, rows: vec![row], search: None, catalog_progress: None,
            preview_context: None, random_context: None, order_context: None };
        let index = HashMap::from([(1, 0)]);
        let cards = Cards { landscape, snapshot: &view, keys: &[1], index: &index, rows: None };
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

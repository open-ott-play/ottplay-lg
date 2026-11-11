//! ui/table.rs — a reusable, animated list/table widget (Apple-TV "settings" look).
//!
//! A single selectable list built from `Section`s of `Row`s. The focused row is drawn as a
//! light rounded "pill" that SLIDES between rows (a spring on its y), rows scroll with a spring
//! when they overflow the frame, and everything is drawn through a `Painter` so the owner can
//! fade/translate the whole thing in (the open animation). It knows nothing about playback or
//! metadata sources: the caller builds rows (including optional progress intervals), supplies
//! accepted UTC, drives selection, and reads `sel` back — so the
//! same widget serves the in-player track menu and the Settings, Privacy and Legal surfaces.
#![allow(dead_code)]
use crate::fit;
use crate::hstack::{Align, Flow, HStack, Item, Span};
use crate::vstack::{Anchor, VStack};
use crate::label::{HAlign, Label, VAlign};
use crate::theme;
use crate::{Painter, Rect, Spring};
use std::ffi::CString;

thread_local! {
    /// The focused menu row's marquee clock ([`crate::marquee`]). One list holds focus at a time;
    /// a shelf's focused tile title keeps its own clock (`card_row`), so a popover over a shelf
    /// never restarts either.
    static ROW_MARQUEE: crate::marquee::Clock = const { crate::marquee::Clock::new() };
}

/// small trailing chip on a row (audio-description, forced, SDH, …)
pub enum Badge {
    Ad,
    Forced,
    Sdh,
    Cc,
    Text(String),
}

/// Artwork shown in the leading column of a Classic catalogue row. The row owns
/// only a source identity; normal artwork caches and fetch policy resolve it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowArt {
    Local(plx_machine::machine::PosterKey),
    Http(String),
    Plex { server: u16, key: String },
}
impl Badge {
    pub fn text(&self) -> &str {
        match self {
            Badge::Ad => plx_platform::i18n::msg::widgets_badge_ad(),
            Badge::Forced => plx_platform::i18n::msg::widgets_badge_forced(),
            Badge::Sdh => plx_platform::i18n::msg::widgets_badge_sdh(),
            Badge::Cc => plx_platform::i18n::msg::widgets_badge_cc(),
            Badge::Text(s) => s.as_str(),
        }
    }
}

/// Who wrote a text slot: the app's own catalog, or a server/user that can send anything (a machine
/// name, a plex.tv handle, a library someone else named). App text never elides at shipped sizes in
/// any shipped language; server text may overflow, so [`app_fit_failures`](TableView::app_fit_failures)
/// ignores it. Every slot defaults to `App`; a caller opts a slot into `Server`, so a forgotten
/// mark reports an overflow rather than hiding it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    #[default]
    App,
    Server,
}

pub struct Row {
    pub label: String,
    /// Display ordinal only; never a row key, selection index or playback identity.
    catalogue_number: Option<String>,
    progress_interval: Option<(i64, i64)>,
    // Derived presentation only; no row identity or additional text storage.
    secondary_label_start: Option<usize>,
    pub label_origin: Origin,
    pub detail: String, // optional sub-line ("" = none)
    pub detail_origin: Origin,
    detail_marker: Option<[f32; 4]>,
    pub badges: Vec<Badge>,
    pub checked: bool, // shows the leading checkmark (the ACTIVE item)
    /// A **switch** rather than a choice: `Some(on)` states itself as the word `On`/`Off` at the
    /// row's TRAILING edge — never as a mark in the leading column. A mark says where you are, a
    /// word says what is set, and no row is allowed to say both; the on/off PAIR of marks this once
    /// drew (a ring, ticked when on) is gone from the design system, assets and all.
    pub toggle: Option<bool>,
    /// Any other trailing read-out, in the same slot and the same voice as [`Row::toggle`]'s
    /// `On`/`Off` — a row whose value is a word rather than a state ("English", "Title"). Wins over
    /// `toggle` if both are somehow set.
    pub value: Option<String>,
    pub value_origin: Origin,
    /// Quieten the read-out a further step (the value is context rather than the point).
    pub value_dim: bool,
    /// THE trailing accessory icon slot (an SVG asset, never a font glyph): the drill-in
    /// chevron (via the [`Row::chevron`] sugar) or e.g. the sort menu's direction chevron.
    pub ticon: Option<crate::icons::Icon>,
    /// THE leading accessory icon — the SAME column the [`Row::checked`] checkmark occupies, for
    /// lists whose rows are ACTIONS rather than a picker's options (the item context menu's
    /// `[icon] [label]` rows). `checked` wins the slot when both are set: a picker's active mark is
    /// state, an action glyph is only decoration.
    pub licon: Option<crate::icons::Icon>,
    /// Small provider artwork in a numbered Classic catalogue row.
    pub catalogue_art: Option<RowArt>,
    /// A semantic fallback mark when a catalogue row has no artwork.
    pub catalogue_icon: Option<crate::icons::Icon>,
    /// Aligned capability strip; false reserves the slot without claiming archive.
    catalogue_archive: Option<bool>,
    pub dim: bool, // render dimmer (unavailable / de-emphasised)
    /// A **non-selectable hairline** that groups the rows above it from the rows below (the item
    /// menu's navigation-vs-state divider). It occupies a global row index like any other row, but
    /// [`TableView::move_sel`] steps OVER it, [`TableView::hit_row`] refuses it, and
    /// [`TableView::set_sections`] never lands the selection on one — so a caller can never focus
    /// a line that does nothing. (A second `Section` does the same job for a group boundary: every
    /// section after the first is preceded by a gap and a hairline, headed or not, so a
    /// `Row::separator` beside a section boundary would draw two rules.)
    pub sep: bool,
    /// The row's action is **destructive** — it ends or removes something (Sign out, Remove from
    /// Deck). Semantics only, never drawn: it exists so a menu never OPENS with its focus on one
    /// ([`TableView::opening_row`]). A stray OK on a freshly opened menu must be harmless; the row
    /// stays one press away, it is simply never where focus starts.
    pub destructive: bool,
    /// How many CAPTION lines a [`Row::note`] wraps to in the table's text column (`1` until
    /// [`TableView::fit_notes`] has measured it). A cache of a pure function of (text, width,
    /// measure), so it lives in a `Cell` and draw may refresh it through `&self`.
    pub note_lines: std::cell::Cell<u8>,
    note_leading: std::cell::Cell<f32>,
}
impl Row {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            catalogue_number: None,
            progress_interval: None,
            secondary_label_start: None,
            label_origin: Origin::default(),
            detail: String::new(),
            detail_origin: Origin::default(),
            detail_marker: None,
            badges: Vec::new(),
            checked: false,
            toggle: None,
            value: None,
            value_origin: Origin::default(),
            value_dim: false,
            ticon: None,
            licon: None,
            catalogue_art: None,
            catalogue_icon: None,
            catalogue_archive: None,
            dim: false,
            sep: false,
            destructive: false,
            note_lines: std::cell::Cell::new(1),
            note_leading: std::cell::Cell::new(NOTE_LEADING),
        }
    }
    /// An optional ASCII-decimal display ordinal, drawn only by an explicitly
    /// opted-in Classic catalogue. Empty or non-digit input clears the number.
    pub fn catalogue_number(mut self, value: impl Into<String>) -> Self {
        let value = value.into();
        self.catalogue_number = (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())).then_some(value);
        self
    }

    /// Optional secondary suffix of a single-line Classic catalogue label.
    /// Byte boundaries are checked again on read because labels may be replaced.
    pub fn set_secondary_label_start(&mut self, start: Option<usize>) {
        self.secondary_label_start = start.filter(|at| *at < self.label.len() && self.label.is_char_boundary(*at));
    }
    pub fn secondary_label_start(&self) -> Option<usize> {
        self.secondary_label_start.filter(|at| *at < self.label.len() && self.label.is_char_boundary(*at))
    }

    /// An accepted interval, drawn only by an explicitly opted-in Classic
    /// catalogue. This never selects a programme or reads a wall clock.
    pub fn progress_interval(mut self, start_s: i64, end_s: i64) -> Self {
        self.set_progress_interval(Some((start_s, end_s)));
        self
    }
    pub fn progress_bounds(&self) -> Option<(i64, i64)> { self.progress_interval }
    /// Supports a targeted programme rollover/clear without replacing the row's
    /// identity or presentation. Compare bounds before requesting `row_mut`.
    pub fn set_progress_interval(&mut self, interval: Option<(i64, i64)>) -> bool {
        let interval = interval.filter(|(start, end)| end > start);
        let changed = self.progress_interval != interval;
        self.progress_interval = interval;
        changed
    }
    fn progress_fraction(&self, utc_s: Option<i64>) -> f32 {
        let Some((start, end)) = self.progress_interval else { return 0.0; };
        let Some(now) = utc_s.filter(|now| *now >= start && *now < end) else { return 0.0; };
        // Widen before subtracting: valid intervals may span both i64 extremes.
        ((i128::from(now) - i128::from(start)) as f64
            / (i128::from(end) - i128::from(start)) as f64).clamp(0.0, 1.0) as f32
    }

    /// A [`Row::note`]: `sep` with text (a bare `sep` is the hairline).
    pub fn is_note(&self) -> bool {
        self.sep && !self.label.is_empty()
    }
    /// The grouping hairline — a row that draws a rule and cannot be focused.
    pub fn separator() -> Self {
        Self {
            sep: true,
            ..Self::new("")
        }
    }
    /// **A non-selectable INFORMATIONAL line** — the same "cannot be focused" contract as
    /// [`Row::separator`] (skipped by every selection/hit walk that checks [`Self::sep`]), but it
    /// carries a label and draws quiet CAPTION text instead of a hairline. For a reason attached to
    /// the rows above it (e.g. "Not available with subtitles on."), left-aligned with their labels
    /// and WRAPPED to as many lines as the column needs ([`TableView::fit_notes`] sets the row's
    /// height) — never a hairline's own job of dividing two groups, and never a focusable row a
    /// stray OK could land on.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            sep: true,
            dim: true,
            ..Self::new(text)
        }
    }
    pub fn checked(mut self, v: bool) -> Self {
        self.checked = v;
        self
    }
    /// Mark [`Self::label`] as server/user text (a machine name, a library someone else named) —
    /// see [`Origin`].
    pub fn server_label(mut self) -> Self {
        self.label_origin = Origin::Server;
        self
    }
    /// Mark [`Self::detail`] as server/user text — see [`Row::server_label`].
    pub fn server_detail(mut self) -> Self {
        self.detail_origin = Origin::Server;
        self
    }
    /// Mark [`Self::value`] as server/user text — see [`Row::server_label`].
    pub fn server_value(mut self) -> Self {
        self.value_origin = Origin::Server;
        self
    }
    /// Mark this row a SWITCH at the given state — see [`Row::toggle`].
    pub fn toggle(mut self, on: bool) -> Self {
        self.toggle = Some(on);
        self
    }
    /// Give this row a trailing read-out — see [`Row::value`].
    pub fn value(mut self, v: impl Into<String>) -> Self {
        self.value = Some(v.into());
        self
    }
    /// Quieten the read-out a step — see [`Row::value_dim`].
    pub fn value_dim(mut self, v: bool) -> Self {
        self.value_dim = v;
        self
    }
    /// The word this row's trailing slot shows, if any: an explicit [`Row::value`], else a switch's
    /// `On`/`Off`. ONE resolver, so the two can never both be drawn.
    fn readout(&self) -> Option<&str> {
        match (&self.value, self.toggle) {
            (Some(v), _) => Some(v.as_str()),
            (None, Some(on)) => Some(if on { plx_platform::i18n::msg::widgets_toggle_on() } else { plx_platform::i18n::msg::widgets_toggle_off() }),
            (None, None) => None,
        }
    }
    /// [`Origin`] of [`Self::readout`]: a switch's `On`/`Off` word is always app text.
    #[cfg(any(test, feature = "test-support"))]
    fn readout_origin(&self) -> Origin {
        if self.value.is_some() { self.value_origin } else { Origin::App }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    /// Small status dot before the detail, with width reserved by the same layout.
    pub fn detail_marker(mut self, color: [f32; 4]) -> Self {
        self.detail_marker = Some(color);
        self
    }
    fn detail_marker_width(&self) -> f32 {
        if self.detail_marker.is_some() && !self.detail.is_empty() { 16.0 } else { 0.0 }
    }
    pub fn badge(mut self, b: Badge) -> Self {
        self.badges.push(b);
        self
    }
    /// sugar: the "›" drill-in affordance is just the Chevron icon in the trailing slot
    pub fn chevron(mut self, v: bool) -> Self {
        if v {
            self.ticon = Some(crate::icons::Icon::Chevron);
        }
        self
    }
    pub fn ticon(mut self, i: crate::icons::Icon) -> Self {
        self.ticon = Some(i);
        self
    }
    pub fn licon(mut self, i: crate::icons::Icon) -> Self {
        self.licon = Some(i);
        self
    }
    pub fn catalogue_art(mut self, art: RowArt) -> Self {
        self.catalogue_art = Some(art);
        self
    }
    pub fn catalogue_archive(mut self, available: bool) -> Self {
        self.catalogue_archive = Some(available);
        self
    }
    pub fn catalogue_archive_available(&self) -> Option<bool> { self.catalogue_archive }
    pub fn catalogue_icon(mut self, icon: crate::icons::Icon) -> Self {
        self.catalogue_icon = Some(icon);
        self
    }
    pub fn dim(mut self, v: bool) -> Self {
        self.dim = v;
        self
    }
    /// Mark the row's action destructive — see [`Row::destructive`].
    pub fn destructive(mut self, v: bool) -> Self {
        self.destructive = v;
        self
    }
    /// `tall` is the TABLE's two-line measure — see [`TableView::tall_rows`]. A row cannot answer
    /// this alone: 92 and 98 are both correct, and which one applies is a property of the LIST.
    fn height_in(&self, tall: f32, plain: f32) -> f32 {
        if self.sep && self.label.is_empty() {
            SEP_H // the hairline
        } else if self.is_note() {
            // a `note`: its wrapped CAPTION lines plus air, never shorter than a plain row
            (f32::from(self.note_lines.get()) * self.note_leading.get() + 2.0 * NOTE_PAD).max(plain)
        } else if self.detail.is_empty() {
            plain // a plain row with no detail line
        } else {
            tall
        }
    }
}

pub struct Section {
    pub header: String, // "" = no header row
    pub header_origin: Origin,
    /// Right-aligned accessory on the HEADER line — a second fact about the group, one rung down
    /// and in the header's own dim ink ("Dolby Atmos"; the Sources list's owner handle beside a
    /// machine name). It is the last run on that line and is **elided** to the width
    /// [`TableView::header_columns`] resolves for it (its natural width capped at [`ACCESSORY_W`],
    /// and what the header leaves of the span), so a long plex.tv handle truncates on a character
    /// instead of colliding with the header or widening the panel — the rows below it are never
    /// touched.
    pub accessory: String,
    pub accessory_origin: Origin,
    /// The app-owned LEADING part of a [`Origin::Server`] accessory (the Sources list's
    /// `Not reachable ·` before a handle), which must fit its resolved column even though the
    /// whole run may not. Empty when the accessory is wholly one origin. Only the fit report reads it.
    pub accessory_app_prefix: String,
    /// Dim the WHOLE group — header, accessory and every row — at one alpha.
    ///
    /// Deliberately not [`Row::dim`] applied row by row: a row's dim is an ink role, and the state
    /// this expresses (an unreachable server, still granted and still pinned) is a fact about the
    /// GROUP, whose header would otherwise stay the brightest thing in the panel. One alpha over
    /// the lot is also what keeps a dimmed row's own read-out ("On") legible as itself: nothing was
    /// unpinned, so nothing may read as off.
    pub dim: bool,
    pub rows: Vec<Row>,
}
impl Section {
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            header_origin: Origin::default(),
            accessory: String::new(),
            accessory_origin: Origin::default(),
            accessory_app_prefix: String::new(),
            dim: false,
            rows: Vec::new(),
        }
    }
    pub fn accessory(mut self, a: impl Into<String>) -> Self {
        self.accessory = a.into();
        self
    }
    /// Mark [`Self::header`] as server/user text (a machine name) — see [`Origin`].
    pub fn server_header(mut self) -> Self {
        self.header_origin = Origin::Server;
        self
    }
    /// Mark [`Self::accessory`] as server/user text (a plex.tv handle) — see [`Origin`].
    pub fn server_accessory(mut self) -> Self {
        self.accessory_origin = Origin::Server;
        self
    }
    /// Declare the app-owned leading part of a [`Self::server_accessory`], text included in
    /// [`Self::accessory`] — see [`Section::accessory_app_prefix`].
    pub fn accessory_app_prefix(mut self, p: impl Into<String>) -> Self {
        self.accessory_app_prefix = p.into();
        self
    }
    /// Dim the whole group at [`GROUP_DIM_A`] — see [`Section::dim`].
    pub fn dim(mut self, v: bool) -> Self {
        self.dim = v;
        self
    }
    pub fn row(mut self, r: Row) -> Self {
        self.rows.push(r);
        self
    }
}

/// One row by GLOBAL index across `sections` — [`TableView::row_mut`]'s lookup for a list that has
/// not been handed to a [`TableView`] yet (`track_menu`'s style-lock pass edits its sections first).
pub fn row_mut_in(sections: &mut [Section], gi: usize) -> Option<&mut Row> {
    sections.iter_mut().flat_map(|s| s.rows.iter_mut()).nth(gi)
}

/// A [`Row::separator`]'s row height. The hairline sits on its centre line, so this IS the gap
/// between the two groups it divides — a gap between stacked blocks comes from a `space` rung, so
/// it is the rung, not a hand-tuned number.
const SEP_H: f32 = theme::space::MD;
/// The list's own vertical padding — air above the first row, air below the last.
/// [`TableView::update`] subtracts it from the frame height it is given, so a caller never derives
/// this number itself. It USED to be something every popover screen subtracted by hand
/// (`panel_h - 40.0`), which is exactly the footgun this constant's own doc used to warn about and
/// which half the callers fell into anyway — passing the raw frame height straight through, so the
/// scroll clamp thought the viewport was `PAD_V` taller than it is and stopped short of the true
/// bottom. Settings ▸ Privacy & data's *Delete all local data* row (last in its list) was the
/// reported case; `legal.rs`, `onboard.rs` and both of `consent.rs`'s tables had the identical bug.
pub const PAD_V: f32 = TOP_PAD + BOT_PAD;
/// A plain row (label only) — mockup rowBase padding 13 + 34px label.
///
/// `pub` for the same caller shape [`CONTENT_X`] is: a block that draws ROWS of its own on the
/// app's ground rather than mounting a [`TableView`] (`screens::search::render`'s `recents` — its rows are the
/// user's own words and have to stay editable in place). It re-derived this and the four constants
/// below from the mockup, so a row-height change here silently misaligned that block while both
/// modules' own tests stayed green.
pub const ROW_H: f32 = 60.0;
const ROW_H_TALL: f32 = 92.0; // a row that carries a detail sub-line (title HEADLINE + detail CAPTION)
/// **A CATALOG list's two-line row: 98, against a settings table's 92** — see
/// [`TableView::tall_rows`].
///
/// It arrived as the height of a row carrying a leading 54x81 POSTER, a slot this widget briefly
/// grew for `ui::filmography` and no longer has: the canvas retired the chip the next day ("a
/// poster at that size is a grey rectangle 222 times over") in favour of one large preview beside
/// the list. The chip went with it rather than being kept as a variant nobody draws — it is not an
/// answer waiting for its next caller, it is one that was measured and rejected. The taller ROW
/// survives it, because that was never about the poster: it is what a `BODY` title over a
/// `CAPTION` sub-line wants when the list IS the screen.
pub const ROW_H_ART: f32 = 98.0;
const ROW_SUB_GAP: f32 = 15.0; // title baseline → detail cap-top, in a two-line row
/// Panel header ("AUDIO"/"SUBTITLES", a server over its libraries). 40px in BOTH size classes and
/// whatever the header's own size — the band is fixed so a size change cannot reflow a panel.
/// Tight under the caps on purpose: the header belongs to the rows below it, so the band under its
/// caps ([`HDR_INK_PAD`]) is shorter than the air above them (half [`DIV_H`] + the cap inset).
/// `pub` for the [`ROW_H`] caller shape.
pub const HDR_H: f32 = 40.0;
/// Measure a [`Section::accessory`] is elided to. It is the LAST run on the header line, so it is
/// the one that gives way: a 34-character plex.tv handle truncates on a character and the library
/// names underneath keep their full width.
const ACCESSORY_W: f32 = 320.0;
/// The alpha a [`Section::dim`]med group is drawn at — the same weight an unreachable tab pill
/// takes, applied once over header and rows together.
const GROUP_DIM_A: f32 = 0.52;
/// A note's line pitch and its air above/below the block: one line is exactly [`ROW_H`].
const NOTE_LEADING: f32 = 32.0;
const NOTE_PAD: f32 = 14.0;
const DIV_H: f32 = 24.0; // gap + hairline between sections
/// Extra air between a page title band and the hairline under it (owner, 2026-10-01: "a bigger gap
/// under the title"): the title sits on its own, then the divider, then the rows.
const TITLE_GAP: f32 = 12.0;
/// The list's own air above its first row. `pub` because a panel that stacks something ABOVE the
/// list (the Sources panel's level band) has to subtract it to put the SEAM on the space scale —
/// otherwise the two paddings add and the gap lands between rungs.
pub const TOP_PAD: f32 = 20.0;
pub const BOT_PAD: f32 = 20.0;
/// Distance from a table frame's top edge to the cap-top of its first section label.
///
/// Route screens use this to align that label with the narrative title in the neighbouring
/// column.  Keeping the relationship here means a future table-padding retune cannot silently
/// break every two-column screen.
pub const FIRST_HEADER_CAP_OFFSET: f32 = TOP_PAD + HEADER_CAP_INSET;
const HEADER_CAP_INSET: f32 = 8.0;
/// Pill (row) inset from the panel's left/right. `pub` for the [`ROW_H`] caller shape.
pub const SIDE: f32 = 12.0;
const CONTENT_PAD: f32 = 20.0; // text/check padding inside the pill (mockup rowBase 13px 20px)
/// Where a row's own content starts, measured from the panel's left edge. Exposed so a panel that
/// draws chrome ABOVE the list (the Sources panel's level pills) can start it on the same line the
/// rows do, instead of re-deriving two private constants and drifting from them.
pub const CONTENT_X: f32 = SIDE + CONTENT_PAD;

/// Narrowest a popover menu panel gets (only to avoid a degenerate sliver around one short row).
pub const MENU_MIN_W: f32 = 300.0;
/// Widest any popover menu panel may be — the one shared cap ([`TableView::menu_panel_width`]).
/// Content wider than this ellipsizes on screen and FAILS the per-menu localization tests, so a
/// translation that needs more room is caught in `cargo test`, not on a customer's TV.
pub const MENU_MAX_W: f32 = 650.0;
const CHECK_W: f32 = 32.0; // leading check column
const GAP: f32 = 16.0; // check→label gap
/// The leading column every row's label stands past: the check column and its gap to the label.
const LABEL_INSET: f32 = CHECK_W + GAP;
/// Focused-row pill corner radius. `pub` for the [`ROW_H`] caller shape.
pub const PILL_RAD: f32 = 18.0;
/// Pill inset from the row's top/bottom. `pub` for the [`ROW_H`] caller shape.
pub const PILL_INSET: f32 = 3.0;
const PANEL_BG: [f32; 4] = theme::SURFACE_PANEL; // opaque panel colour — fade masks + badge knockout
/// Air between two chips of one right-aligned badge run (a subtitle row's `FORCED` + `SDH`).
const BADGE_GAP: f32 = 10.0;
const ACCESSORY_GAP: f32 = 14.0;
/// The air a HUGGED panel keeps between a row's label and its trailing read-out — the spacing
/// scale's label→value rung ([`theme::space::MD`], 24px). [`TableView::measured_width`] budgets it,
/// so at its own width every row shows at least this much space. It is the panel-sizing target,
/// not the layout's floor: a crowded row (a table wider content than its frame) may still squeeze
/// the pair down to [`ACCESSORY_GAP`] ([`TableView::row_columns_under`]) before the value elides.
const ROW_VALUE_GAP: f32 = theme::space::MD;
/// The empty band under a row's lowest ink, which every row kind leaves: a plain row's label is
/// centred in `ROW_H` with 13px under it, and a two-line row's centred pair leaves ~12.
const ROW_INK_PAD: f32 = 12.0;
/// How far below the frame's top a row's (or header's) ink is fully opaque: the band over which the
/// top edge dissolves it ([`TableView::top_edge_alpha`]). It is exactly where the FIRST thing in a
/// table rests — a section label's caps sit [`FIRST_HEADER_CAP_OFFSET`] under the frame's top and a
/// headerless table's first row ink [`TOP_PAD`] + [`ROW_INK_PAD`] — so a table at rest is never faded.
const TOP_FADE_H: f32 = FIRST_HEADER_CAP_OFFSET;
/// A section header's ink ends at its CAPTION caps below `HEADER_CAP_INSET`.
const HDR_INK_PAD: f32 = HDR_H - HEADER_CAP_INSET - theme::size::CAPTION as f32;

/// The bottom-edge fade shared by rows and headers ([`TableView::bottom_edge_alpha`]): opaque
/// while `ink_bot` has `pad + BOT_PAD` of viewport below it, transparent once the edge reaches it.
fn edge_alpha(ink_bot: f32, vis_bot: f32, pad: f32) -> f32 {
    ((vis_bot - ink_bot) / (pad + BOT_PAD)).clamp(0.0, 1.0)
}

/// A row's two text columns as [`TableView::row_columns`] resolves them: the primary label's (and
/// its sub-line's) width, and the trailing value's, which the value is elided to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowColumns {
    pub label_w: f32,
    pub value_w: f32,
}

/// Which text slot a [`FitIssue`] names.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitRole {
    Label,
    Detail,
    Value,
    Header,
    Accessory,
    Note,
    /// The page title band ([`TableView::set_title`]).
    Title,
}

/// One text slot [`TableView::fit_report`] found would end in an ellipsis at its resolved column.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub struct FitIssue {
    pub role: FitRole,
    pub origin: Origin,
    pub text: String,
    pub natural: f32,
    pub budget: f32,
}
#[cfg(any(test, feature = "test-support"))]
impl FitIssue {
    fn describe(&self) -> String {
        format!("{:?} {:?} is {:.0}px in a {:.0}px column", self.role, self.text, self.natural, self.budget)
    }
}

/// Slack, in px, when comparing a resolved column with a run's natural width: `fit::two_runs`
/// derives the secondary as `span - primary - gap`, which float rounding can leave a hair under the
/// width it was given.
#[cfg(any(test, feature = "test-support"))]
const FIT_EPS: f32 = 0.01;

/// Fail a text-fit test with every finding [`TableView::app_fit_failures`] collected.
#[cfg(any(test, feature = "test-support"))]
pub fn assert_no_fit_failures(out: &[String]) {
    assert!(out.is_empty(), "text the television would end in an ellipsis:\n  {}", out.join("\n  "));
}
const ACCESSORY_ICON_W: f32 = 26.0;

/// The horizontal space a lone trailing icon takes from the content edge: its INK width (per
/// [`crate::icons::ink_x`]) plus [`ACCESSORY_GAP`]. The tests' yardstick for what [`trailing_items`]
/// stands, written out independently of the stack.
#[cfg(test)]
fn ticon_slot_w(icon: crate::icons::Icon) -> f32 {
    let (l, r) = crate::icons::ink_x(icon);
    (r - l) * ACCESSORY_ICON_W + ACCESSORY_GAP
}

/// **A row's trailing run, as ONE stack**: the read-out, then the badge run, then the trailing icon,
/// [`ACCESSORY_GAP`] apart and anchored by the icon's INK at the content edge ([`Item::glyph`]: the
/// icon is drawn so its ink, not its 26px box, ends on the edge a flush read-out ends on, whatever
/// the glyph's own side bearing). `value_w` is the read-out's natural width, `None` for a row
/// without one. The width the label may not use ([`trailing_reserved`]), where each run stands
/// ([`trailing_stack`] at `text_right`) and the tests' read-out edge all read these items, so
/// they cannot disagree.
fn trailing_items(row: &Row, value_w: Option<f32>, measure: &dyn plx_machine::machine::Measure,
    progress_w: Option<f32>) -> [Item; 4] {
    [
        value_w.map_or(Item::absent(), Item::new),
        if row.badges.is_empty() { Item::absent() } else { Item::new(badge_run_w(&row.badges, measure)) },
        row.ticon.map_or(Item::absent(), |icon| Item::glyph(icon, ACCESSORY_ICON_W)),
        progress_w.map_or(Item::absent(), Item::new),
    ]
}

/// [`trailing_items`] anchored with its trailing edge at `right` (the content edge).
fn trailing_stack(items: &[Item; 4], right: f32) -> HStack<'_> {
    HStack::new(items, ACCESSORY_GAP, Align::Trailing(right))
}

/// What a row reserves for its trailing run: the stack's width plus the [`ACCESSORY_GAP`] between
/// the run and the label before it (0 for a row with no trailing run at all).
fn trailing_reserved(items: &[Item; 4]) -> f32 {
    let width = trailing_stack(items, 0.0).width();
    if items.iter().any(|item| *item != Item::absent()) { width + ACCESSORY_GAP } else { 0.0 }
}

fn progress_frame(at: Span, row_y: f32, row_h: f32) -> Rect {
    let height = (row_h / 3.5).floor().max(1.0).min(row_h.max(0.0));
    Rect::new(at.x, row_y + (row_h - height) * 0.5, at.w, height)
}

fn paint_progress(p: Painter, bar: Rect, fraction: f32, ink: [f32; 4]) {
    if bar.w <= 0.0 || bar.h <= 0.0 { return; }
    let split = bar.x + bar.w * fraction.clamp(0.0, 1.0);
    // Preserve the caller's scissor, including a partially visible list row.
    // Disjoint clips avoid double alpha at the antialiased rounded boundary.
    if split > bar.x {
        let _clip = crate::screen::ClipScope::open_in(p, Rect::new(bar.x, bar.y, split - bar.x, bar.h));
        p.rrect(bar, theme::space::XS.min(bar.h * 0.5), theme::space::XS.min(bar.h * 0.5), ink);
    }
    if split < bar.x + bar.w {
        let _clip = crate::screen::ClipScope::open_in(p, Rect::new(split, bar.y, bar.x + bar.w - split, bar.h));
        p.rrect(bar, theme::space::XS.min(bar.h * 0.5), theme::space::XS.min(bar.h * 0.5), crate::palette::color(crate::palette::Role::Surface, theme::CLASSIC_PROGRESS_TRACK));
    }
}

/// The badges of a row's trailing run, [`BADGE_GAP`] apart from `x` on: each with the span it
/// draws. The ONE place the run is laid out — its width ([`badge_run_w`]) and where each chip is
/// painted are both read off it, so they cannot disagree.
fn badge_spans<'a>(
    badges: &'a [Badge],
    x: f32,
    measure: &'a dyn plx_machine::machine::Measure,
) -> impl Iterator<Item = (&'a Badge, Span)> + 'a {
    let mut flow = Flow::new(x, BADGE_GAP);
    badges.iter().filter_map(move |b| {
        flow.push(Item::new(crate::widgets::badge_w(b.text(), None, measure))).map(|span| (b, span))
    })
}

/// The badge run's width: the last chip's trailing edge when it starts at 0 (0 for no badges).
fn badge_run_w(badges: &[Badge], measure: &dyn plx_machine::machine::Measure) -> f32 {
    badge_spans(badges, 0.0, measure).last().map_or(0.0, |(_, s)| s.x + s.w)
}

/// Where the trailing icon's box starts, as an offset from the content edge (<= 0): the box is
/// placed so the glyph's ink right edge lands ON the content edge. A test's yardstick for the
/// box [`trailing_stack`] stands.
#[cfg(test)]
fn ticon_box_dx(icon: crate::icons::Icon) -> f32 {
    -crate::icons::ink_x(icon).1 * ACCESSORY_ICON_W
}
/// The trailing read-out's WEIGHT: `size::LABEL` **bold**, which is what the `PlxNative Design
/// System`'s `TableView` authors it as (`var(--font-weight-bold) var(--size-label)`) and what its
/// prose says in words. The product drew it regular until 2026-08-21 — a rung below the row's
/// HEADLINE label in size, a step behind it in ink, AND a weight lighter, which is three
/// de-emphases stacked on the one run that answers the row's question. The read-out is not a
/// caption of the label; it is the value, and it has to hold its own beside the badge run next to
/// it. The step back is the INK's alone.
///
/// ONE constant, because the three calls at the draw site have to agree: the cap band the run is
/// centred on, the advance width it adds to the trailing stack ([`trailing_items`]), and the paint.
/// Bold glyphs are WIDER than their regular twins, so measuring on one flag and painting on the
/// other under-counts [`trailing_reserved`] — and that is the LABEL's elision budget (`text_w` below), so the label would be elided
/// as though the read-out were narrower than it is and run right into it. The BADGE run is not at
/// risk: it is placed outside the read-out, while the complete trailing-width calculation
/// reserves both runs before measuring the label.
const VALUE_BOLD: std::os::raw::c_int = 1;

/// The next layout stamp: unique across tables, so a cache keyed on one cannot be fooled by two
/// tables that happen to have mutated the same number of times.
fn next_rev() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Derived from the same measurement capability as the painted text. Minimum
/// touch targets and the default layout stay unchanged when text is reduced.
#[derive(Clone, Copy, PartialEq)]
struct TableMetrics {
    plain: f32,
    tall: f32,
    art: f32,
    header: f32,
    page_rows: usize,
}
impl TableMetrics {
    const DEFAULT: Self = Self { plain: ROW_H, tall: ROW_H_TALL, art: ROW_H_ART, header: HDR_H, page_rows: 0 };
}

/// Table contents with a derived row-shape certificate. Mutable access invalidates
/// the certificate even when a caller edits a row directly rather than via row_mut.
/// Neither the certificate nor the revision is canonical state.
pub struct TableSections {
    inner: Vec<Section>,
    revision: u32,
    plain_rows: std::cell::Cell<Option<bool>>,
}
impl TableSections {
    const fn new() -> Self {
        Self { inner: Vec::new(), revision: 0, plain_rows: std::cell::Cell::new(None) }
    }
    fn invalidate(&mut self) {
        self.plain_rows.set(None);
        self.revision = next_rev();
    }
    pub fn into_vec(self) -> Vec<Section> { self.inner }
}
impl Default for TableSections {
    fn default() -> Self { Self::new() }
}
impl From<Vec<Section>> for TableSections {
    fn from(inner: Vec<Section>) -> Self {
        Self { inner, revision: next_rev(), plain_rows: std::cell::Cell::new(None) }
    }
}
impl FromIterator<Section> for TableSections {
    fn from_iter<T: IntoIterator<Item = Section>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}
impl std::ops::Deref for TableSections {
    type Target = Vec<Section>;
    fn deref(&self) -> &Self::Target { &self.inner }
}
impl std::ops::DerefMut for TableSections {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.invalidate();
        &mut self.inner
    }
}
impl IntoIterator for TableSections {
    type Item = Section;
    type IntoIter = std::vec::IntoIter<Section>;
    fn into_iter(self) -> Self::IntoIter { self.inner.into_iter() }
}
impl<'a> IntoIterator for &'a TableSections {
    type Item = &'a Section;
    type IntoIter = std::slice::Iter<'a, Section>;
    fn into_iter(self) -> Self::IntoIter { self.inner.iter() }
}
impl<'a> IntoIterator for &'a mut TableSections {
    type Item = &'a mut Section;
    type IntoIter = std::slice::IterMut<'a, Section>;
    fn into_iter(self) -> Self::IntoIter {
        self.invalidate();
        self.inner.iter_mut()
    }
}

pub struct TableView {
    pub sections: TableSections,
    pub sel: i32, // global index across all sections' rows (headers are not selectable)
    /// Does the LIST hold focus? `false` draws no selection pill at all — the "nothing selected"
    /// mode this widget did not have.
    ///
    /// It exists for a panel with a control OUTSIDE the list (the Sources panel's Browse / On Home
    /// segments): while focus is up there, a table that always paints its selection puts a second
    /// accent capsule on screen, and the two are indistinguishable. `sel` is REMEMBERED across the
    /// trip — you come back to the row you left, so this suppresses the pill rather than clearing
    /// the selection.
    ///
    /// Defaults to `true`, so every panel whose list is the only focusable thing is unchanged.
    pub list_focused: bool,
    /// compact size class: BODY regular row LABELS instead of the default HEADLINE bold (HEADLINE-bold
    /// rows overwhelmed the small account popover, a panel of one-word actions). Set it on every
    /// ACTION menu — the item context menu, account, more, and the library sort/filter/genre menus
    /// all do; only a PICKER of title+detail rows stays on the default.
    ///
    /// It no longer affects HEADERS: those are CAPS at CAPTION in both classes, because the caps
    /// are what make a header a label and a size that varied could tie with its own rows.
    pub compact: bool,
    /// Presentation only; one measured geometry still owns paint, hit, focus and scroll.
    classic: std::cell::Cell<bool>,
    classic_catalogue: std::cell::Cell<bool>,
    classic_choices: std::cell::Cell<bool>,
    classic_menu: std::cell::Cell<bool>,
    classic_clock: std::cell::Cell<Option<i64>>,
    classic_page: std::cell::Cell<Option<(usize, f32)>>,
    /// see [`TableView::tall_rows`]
    tall: bool,
    /// Semantic ink for section labels. Ambient routes can raise this role for contrast without
    /// replacing the shared table header renderer or changing row/detail hierarchy.
    pub header_ink: [f32; 4],
    /// The floor [`Self::menu_panel_width`] clamps to; [`MENU_MIN_W`] unless a panel family sets a
    /// wider one (the in-player popovers: [`theme::layout::PLAYER_MENU_MIN_W`]).
    pub min_panel_w: f32,
    // the highlight pill's top and bottom edges spring INDEPENDENTLY (content coords), so moving
    // to a taller/shorter row morphs the pill smoothly instead of snapping its height.
    hl_top: Spring,
    hl_bot: Spring,
    scroll: Spring,
    /// A drill-in page's title band, see [`TableView::set_title`].
    title: Option<String>,
    /// Re-stamped by every mutation that can change the table's measured size (new sections, a new
    /// title, a row edited in place), so a panel that caches its layout target can tell whether
    /// the table it measured is still the table it holds ([`Self::layout_rev`]). The stamp comes
    /// from one process-wide counter ([`next_rev`]), not a per-table one: a panel swaps whole
    /// tables in and out ([`Self::blank_like`]), and two tables each on their second mutation must
    /// not look like the same layout.
    rev: u32,
    metrics: std::cell::Cell<TableMetrics>,
    measure_rev: std::cell::Cell<u32>,
    // The contents may be moved from an older table. An exact tuple, not max(revs),
    // keeps layout invalidation correct when that happens more than once.
    layout_stamp: std::cell::Cell<Option<((u32, u32, u32), u32)>>,
    // Only the digit count is cached: font and viewport measurements stay live.
    // set_sections/row_mut invalidate through rev, as with the other layout caches.
    catalogue_digits: std::cell::Cell<Option<((u32, u32), usize)>>,
    /// Observational work counter for host regression tests; not shipped or canonical state.
    #[cfg(any(test, feature = "test-support"))]
    row_visits: std::cell::Cell<usize>,
}
/// [`TableView::walk`]'s event index for the hairline that divides a section from the one above
/// it (`-1` is a header, `>= 0` a row). Emitted for EVERY section after the first, headed or not.
const WALK_DIVIDER: i32 = -2;
/// [`TableView::walk`]'s event index for the page title band ([`TableView::set_title`]); it is
/// followed by a [`WALK_DIVIDER`] event, so the title reads as the page's own heading.
const WALK_TITLE: i32 = -3;
/// The glyph that opens a page title: the drill-in's "back" mark, the mirror of a row's chevron.
const TITLE_BACK_GLYPH: &str = "\u{2039}";
impl TableView {
    pub const MOTION_SHAPE: &'static str = "TableViewMotion{sel:i32,list_focused:bool,compact:bool,classic:bool,classic_catalogue:bool,classic_choices:bool,classic_menu:bool,classic_clock:Option<i64>,classic_page:Option<{rows:u32,height:f32}>,tall:bool,header_ink:[f32;4],hl_top:Spring{pos:f32,vel:f32},hl_bot:Spring{pos:f32,vel:f32},scroll:Spring{pos:f32,vel:f32}}";

    /// The owner records its row data separately. These fields determine layout, the selected
    /// face and subsequent motion; no text/texture cache or renderer pointer is traversed.
    pub fn write_motion(&self, c: &mut plx_machine::machine::Canon) {
        let Self { sections: _, sel, list_focused, compact, classic, classic_catalogue, classic_choices, classic_menu, classic_clock, classic_page, tall, header_ink, hl_top, hl_bot, scroll, title: _, min_panel_w: _, rev: _, metrics: _, measure_rev: _, layout_stamp: _, catalogue_digits: _,
            #[cfg(any(test, feature = "test-support"))] row_visits: _,
        } = self;
        c.u32(*sel as u32).bool(*list_focused).bool(*compact).bool(classic.get()).bool(classic_catalogue.get()).bool(classic_choices.get()).bool(classic_menu.get());
        c.option(classic_clock.get(), |c, utc| { c.u64(utc as u64); });
        c.option(classic_page.get(), |c, (rows, height)| { c.u32(rows as u32).f32(height); });
        c.bool(*tall);
        for component in header_ink { c.f32(*component); }
        for spring in [hl_top, hl_bot, scroll] { c.f32(spring.pos).f32(spring.vel); }
    }

    /// **Two-line rows at the CATALOG measure (98) rather than the settings one (92).**
    ///
    /// Opt-in per table, because both are right: a settings row is a line of chrome in a panel, and
    /// a row in a list that IS the screen — `ui::filmography`'s credits — carries a `BODY` title
    /// over a `CAPTION` sub-line and wants the air. The canvas states 98 for that list and 92 is
    /// what every other table here has always drawn.
    pub fn tall_rows(&mut self, v: bool) {
        self.tall = v;
    }
    fn tall_row_h(&self) -> f32 { if self.tall { self.metrics.get().art } else { self.metrics.get().tall } }
    fn plain_row_h(&self) -> f32 { self.metrics.get().plain }
    fn header_h(&self) -> f32 { self.metrics.get().header }

    /// Prepare shared geometry before measuring, focusing, scrolling or painting.
    /// This cache has no source or selection state. Changing typography invalidates
    /// intrinsic-height caches just like a changed row would.
    pub fn set_classic(&self, classic: bool) {
        if self.classic.replace(classic) != classic { self.measure_rev.set(next_rev()); }
    }

    pub fn set_classic_catalogue(&self, enabled: bool) { self.classic_catalogue.set(enabled); }
    pub fn is_classic_catalogue(&self) -> bool { self.classic.get() && self.classic_catalogue.get() }

    /// Reuse Classic's dense, single-line list geometry for settings/choice forms.
    /// This is presentation only: it does not admit catalogue paging/volume commands.
    /// Callers retain bindings and project explanatory text outside the row.
    pub fn set_classic_choices(&self, enabled: bool) {
        if self.classic_choices.replace(enabled) != enabled { self.measure_rev.set(next_rev()); }
    }
    /// A full-page Classic menu projects its title/section descriptions outside
    /// the list. Preserve all rows/bindings/data; only omit their duplicate chrome.
    /// This flag has no effect unless Classic is explicitly active.
    pub fn set_classic_menu(&self, enabled: bool) {
        if self.classic_menu.replace(enabled) != enabled { self.measure_rev.set(next_rev()); }
    }
    pub fn is_classic_menu(&self) -> bool { self.classic.get() && self.classic_menu.get() }
    fn presented_row_height(&self, row: &Row) -> f32 {
        if self.is_classic_menu() && !row.sep { self.plain_row_h() }
        else { row.height_in(self.tall_row_h(), self.plain_row_h()) }
    }

    /// Paint-only accepted UTC. No per-row traversal, layout/selection change,
    /// timer, or synthetic advance when the caller has no valid sample.
    pub fn set_classic_clock(&self, utc_s: Option<i64>) -> bool {
        self.classic_clock.replace(utc_s) != utc_s
    }
    pub fn classic_clock(&self) -> Option<i64> { self.classic_clock.get() }
    pub fn is_classic_choices(&self) -> bool { self.classic.get() && (self.classic_choices.get() || self.classic_menu.get()) }
    pub fn is_classic_list(&self) -> bool { self.is_classic_catalogue() || self.is_classic_choices() }

    /// Opt-in for a headerless, single-line Classic body. The requested count is
    /// an upper limit: enlarged or unusually tall text may fit fewer rows.
    /// One geometry owns paging, scrolling, painting and hit testing.
    pub fn set_classic_page_size(&self, preferred: Option<usize>, frame_height: f32) {
        let page = preferred.filter(|rows| *rows > 0)
            .filter(|_| frame_height.is_finite() && frame_height > PAD_V)
            .map(|rows| (rows, frame_height));
        if self.classic_page.replace(page) != page { self.measure_rev.set(next_rev()); }
    }
    pub fn classic_page_rows(&self) -> Option<usize> {
        let rows = self.metrics.get().page_rows;
        (self.is_classic_list() && self.classic_page.get().is_some() && rows > 0).then_some(rows)
    }


    /// Shared Classic list policy: row steps wrap, page steps clamp. One walk
    /// preserves separator semantics and does not allocate a catalogue-sized map.
    pub fn catalogue_next(&self, selected: i32, delta: i32, page_height: Option<f32>) -> Option<i32> {
        let origin = self.row_top(selected);
        let target = origin + delta as f32 * page_height.map_or(0.0, |h|
            self.classic_page_rows().map_or((h - self.vertical_padding()).max(self.plain_row_h()),
                |rows| rows as f32 * self.plain_row_h()));
        let mut first = None; let mut last = None; let mut before = None; let mut after = None;
        let mut nearest: Option<(f32, i32)> = None;
        self.walk(|top, index, _, row| {
            let Some(row) = row else { return; };
            if row.sep { return; }
            first.get_or_insert(index); last = Some(index);
            if index < selected { before = Some(index); }
            if index > selected && after.is_none() { after = Some(index); }
            let distance = (top - target).abs();
            if nearest.is_none_or(|(best, _)| distance < best) { nearest = Some((distance, index)); }
        });
        if page_height.is_some() { nearest.map(|(_, index)| index) }
        else if delta < 0 { before.or(last) } else { after.or(first) }
    }

    pub fn is_classic(&self) -> bool { self.classic.get() }

    // Classic catalogue and choice lists share the web player's edge-to-edge
    // viewport. Actual editors retain the form padding and pointer target floor.
    fn vertical_padding(&self) -> f32 {
        if self.is_classic_list() { 0.0 } else { PAD_V }
    }
    fn top_padding(&self) -> f32 {
        if self.is_classic_list() { 0.0 } else { TOP_PAD }
    }
    fn side_padding(&self) -> f32 {
        if self.is_classic_list() { 0.0 } else { SIDE }
    }
    fn pill_inset(&self) -> f32 {
        if self.is_classic_list() { 0.0 } else { PILL_INSET }
    }
    fn content_clip(&self, frame: Rect) -> Rect {
        // Integer rows can leave a few pixels below the last item. That space
        // must not reveal or activate a partial item from the next page.
        let h = self.classic_page_rows()
            .map_or(frame.h, |rows| frame.h.min(rows as f32 * self.plain_row_h()));
        Rect::new(frame.x, frame.y, frame.w, h)
    }

    pub fn fit_layout(&self, measure: &dyn plx_machine::machine::Measure) {
        let scale = measure.font_scale();
        let label = measure.cap_h(self.label_style().0);
        let value = measure.cap_h(self.value_style().0);
        let caption = measure.cap_h(theme::size::CAPTION);
        let pair = label + ROW_SUB_GAP + caption + 2.0 * ROW_INK_PAD;
        // Forms keep the measured cap band and 44px Classic pointer target.
        // The opt-in catalogue page below owns its denser row geometry.
        let plain = if self.classic.get() { 44.0 } else { ROW_H };
        let mut metrics = TableMetrics {
            plain: plain.max(scale.points(plain)).max(label.max(value) + 2.0 * ROW_INK_PAD),
            tall: ROW_H_TALL.max(scale.points(ROW_H_TALL)).max(pair),
            art: ROW_H_ART.max(scale.points(ROW_H_ART)).max(pair),
            header: HDR_H.max(scale.points(HDR_H)).max(HEADER_CAP_INSET + caption + HDR_INK_PAD),
            page_rows: 0,
        };
        if self.is_classic_list() { if let Some((wanted, height)) = self.classic_page.get() {
            let available = height.max(0.0);
            let wanted = (wanted as f32 / scale.points(1.0).max(1.0)).floor().max(1.0) as usize;
            // Match listFitPageSize/listRowHeight: enlarged text reduces the
            // visible count; the old 44px form floor must not reduce it again.
            // An unusually short viewport still reserves the measured ink.
            let fit = (available / label.max(value).max(1.0)).floor().max(1.0) as usize;
            metrics.page_rows = wanted.min(fit);
            metrics.plain = (available / metrics.page_rows as f32).floor().max(1.0);
        } }
        if self.metrics.replace(metrics) != metrics {
            self.measure_rev.set(next_rev());
        }
    }
    pub const fn new() -> Self {
        Self {
            sections: TableSections::new(),
            sel: 0,
            list_focused: true,
            compact: false,
            classic: std::cell::Cell::new(false),
            classic_catalogue: std::cell::Cell::new(false),
            classic_choices: std::cell::Cell::new(false),
            classic_menu: std::cell::Cell::new(false),
            classic_clock: std::cell::Cell::new(None),
            classic_page: std::cell::Cell::new(None),
            tall: false,
            header_ink: theme::TEXT_TERTIARY,
            min_panel_w: MENU_MIN_W,
            hl_top: Spring::at(0.0),
            hl_bot: Spring::at(0.0),
            scroll: Spring::at(0.0),
            title: None,
            rev: 0,
            metrics: std::cell::Cell::new(TableMetrics::DEFAULT),
            measure_rev: std::cell::Cell::new(0),
            layout_stamp: std::cell::Cell::new(None),
            catalogue_digits: std::cell::Cell::new(None),
            #[cfg(any(test, feature = "test-support"))]
            row_visits: std::cell::Cell::new(0),
        }
    }

    /// This table drawn WITHOUT the focus pill (and without the focused-row ink flip): the page
    /// that is leaving a transition, where only the arriving page owns the selection.
    pub fn unfocused(mut self) -> Self {
        self.list_focused = false;
        self
    }

    /// **An empty table dressed like this one**: the same size class, header ink, row height and
    /// panel-width floor, with no rows, no title and every spring at rest. A panel that moves its
    /// outgoing page aside for a transition (`ui::panel_motion`) swaps this in for the page it
    /// builds next, so the next page needs no re-configuration and the old one is kept whole, to
    /// draw once more, without a `Clone` of every row.
    pub fn blank_like(&self) -> Self {
        let mut t = Self::new();
        t.list_focused = self.list_focused;
        t.compact = self.compact;
        t.classic.set(self.classic.get());
        t.classic_catalogue.set(self.classic_catalogue.get());
        t.classic_choices.set(self.classic_choices.get());
        t.classic_menu.set(self.classic_menu.get());
        t.classic_clock.set(self.classic_clock.get());
        t.classic_page.set(self.classic_page.get());
        t.tall = self.tall;
        t.header_ink = self.header_ink;
        t.min_panel_w = self.min_panel_w;
        t.metrics.set(self.metrics.get());
        t
    }

    /// A counter that moves whenever the table's measured size could have: see the field.
    pub fn layout_rev(&self) -> u32 {
        let key = (self.rev, self.sections.revision, self.measure_rev.get());
        if let Some((old, stamp)) = self.layout_stamp.get() {
            if old == key { return stamp; }
        }
        let stamp = next_rev();
        self.layout_stamp.set(Some((key, stamp)));
        stamp
    }

    fn content_rev(&self) -> (u32, u32) { (self.rev, self.sections.revision) }

    /// The row under the pointer in a `frame`-anchored draw (screen coords), or None — popover
    /// click support (hover→focus, click→commit) shares the draw's own layout walk.
    /// **Where row `i` is on screen** — the exact inverse of [`Self::hit_row`], walked the same
    /// way so the two can never disagree about a row's band.
    ///
    /// It exists for `ui::popover::Opener`: a context menu anchors beside the element it was opened
    /// from, and a caller that measured that band itself would be a second layout of this widget.
    /// Answers `None` for a header, a hairline, or a row scrolled out of the frame — all three are
    /// cases where there is nothing on screen to anchor to.
    pub fn row_rect(&self, frame: Rect, i: i32) -> Option<Rect> {
        let clip = self.content_clip(frame);
        self.row_frame(frame, i)
            .filter(|r| r.y + r.h > clip.y && r.y < clip.y + clip.h)
    }

    /// Row `i`'s frame under the live scroll WHETHER OR NOT it is inside the viewport — the one
    /// walk [`row_rect`](Self::row_rect), [`hit_row`](Self::hit_row) and `ui::geom::Table::place`
    /// share (spec §7.1). `None` for a header index, a separator, or an index out of range.
    pub fn row_frame(&self, frame: Rect, i: i32) -> Option<Rect> {
        if let Some((rows, height)) = self.uniform_classic_rows() {
            let row = rows.get(usize::try_from(i).ok()?)?;
            return Some(self.frame_at(frame, i as f32 * height, row));
        }
        let mut out = None;
        self.walk(|cy, gi, _, row| {
            if gi == i {
                if let Some(row) = row.filter(|row| !row.sep) {
                    out = Some(self.frame_at(frame, cy, row));
                }
            }
        });
        out
    }

    fn frame_at(&self, frame: Rect, content_y: f32, row: &Row) -> Rect {
        Rect::new(frame.x + self.side_padding(), frame.y + self.top_padding() + content_y - self.scroll.pos,
            frame.w - 2.0 * self.side_padding(), self.presented_row_height(row))
    }

    /// Drain actual row visits so host tests can bound work through draw and focus,
    /// rather than timing a machine or testing only a detached layout helper.
    #[cfg(any(test, feature = "test-support"))]
    pub fn take_row_visits(&self) -> usize { self.row_visits.replace(0) }

    /// Visit selectable row frames in one layout walk. This is the bulk form of
    /// `row_frame`, sharing scaled row/header/note geometry with hit testing and
    /// drawing; callers must not rescan the table once per row.
    pub fn for_each_row_frame(&self, frame: Rect, mut visit: impl FnMut(i32, Rect)) {
        self.walk(|cy, gi, _, row| {
            if let Some(row) = row.filter(|row| !row.sep) {
                visit(gi, self.frame_at(frame, cy, row));
            }
        });
    }

    /// Visit only rows intersecting the interactive viewport, without allocating
    /// a catalog-sized hit map. Partially visible rows retain their full geometry;
    /// the caller clips their hit region to the same viewport.
    pub fn for_each_visible_row(&self, frame: Rect, mut visit: impl FnMut(i32, Rect)) {
        let clip = self.content_clip(frame);
        self.walk_window(frame.y + self.top_padding(), self.scroll.pos, clip.y, clip.y + clip.h, |cy, gi, _, row| {
            let Some(row) = row.filter(|row| !row.sep) else { return; };
            let rect = self.frame_at(frame, cy, row);
            if rect.y + rect.h > clip.y && rect.y < clip.y + clip.h {
                visit(gi, rect);
            }
        });
    }

    /// The next SELECTABLE row from `i` in direction `delta` (−1/+1; 0 answers `i` itself if
    /// selectable), stepping over separators as `move_sel` does; `None` at the ends.
    pub fn next_selectable(&self, i: i32, delta: i32) -> Option<i32> {
        let n = self.n_rows();
        if n == 0 || i < 0 || i >= n {
            return None;
        }
        if delta == 0 {
            return (!self.rows_at(i).sep).then_some(i);
        }
        let mut j = i + delta;
        while j >= 0 && j < n {
            if !self.rows_at(j).sep {
                return Some(j);
            }
            j += delta;
        }
        None
    }

    /// The nearest selectable row to `i` (what `set_sections` and `move_sel` settle on).
    pub fn settle(&self, i: i32) -> i32 {
        self.settle_sel(i)
    }
    pub fn hit_row(&self, frame: Rect, mx: f32, my: f32) -> Option<i32> {
        let clip = self.content_clip(frame);
        if !clip.contains(mx, my)
            || (self.classic_page_rows().is_some() && my >= clip.y + clip.h) { return None; }
        let mut hit = None;
        self.walk_window(frame.y + self.top_padding(), self.scroll.pos, my, my, |cy, gi, _, row| {
            let Some(row) = row.filter(|row| !row.sep) else { return; };
            let rect = self.frame_at(frame, cy, row);
            if my >= rect.y && my <= rect.y + rect.h { hit = Some(gi); }
        });
        hit
    }

    /// **A drill-in page's title band**, drawn as "‹ TITLE" in the section-header caps caption at
    /// the very top of the content and followed by the same boundary gap a section divider has
    /// ([`DIV_H`] around a hairline), so it reads as the page's own heading rather than as a
    /// section caption glued to the first row. `None` (the default) draws nothing. The band is
    /// part of [`Self::measured_height`], [`Self::measured_width`] and [`Self::fit_report`]; it is
    /// not a hit target here (the owner that pops on it registers its own stop). Design record:
    /// `docs/player-submenus.md`.
    ///
    /// The band moves every row down (or up) by [`HDR_H`] + [`TITLE_GAP`] + [`DIV_H`], so when it appears or goes
    /// the pill springs are re-jumped to the selected row's new place: installing the title after
    /// the sections must not leave the pill a band off until the spring catches up. The scroll is
    /// left alone (a [`Self::restore_sections`] puts its own back after).
    pub fn set_title(&mut self, title: Option<String>) {
        self.rev = next_rev();
        let moved = self.title.is_some() != title.is_some();
        self.title = title;
        if moved && self.n_rows() > 0 {
            let top = self.row_top(self.sel);
            self.hl_top.jump(top + self.pill_inset());
            self.hl_bot.jump(top + self.row_height(self.sel) - self.pill_inset());
        }
    }

    /// The title band's rect in screen space for a table drawn into `frame`: under the current
    /// scroll, exactly where [`Self::draw`] puts it, and clipped to `frame` as the draw is. The
    /// strip spans the pill's width ([`SIDE`] inset) across [`HDR_H`], for a pointer-only "back"
    /// target. `None` without a title, or once it has scrolled fully out of the viewport. (A
    /// section-0 dim never touches it: the title is the page's, not the section's.)
    pub fn title_rect(&self, frame: Rect) -> Option<Rect> {
        if self.is_classic_menu() { return None; }
        self.title.as_ref()?;
        let band = Rect::new(frame.x + SIDE, frame.y + TOP_PAD - self.scroll.pos, frame.w - 2.0 * SIDE, self.header_h());
        let seen = band.intersect(frame);
        (seen.w > 0.0 && seen.h > 0.0).then_some(seen)
    }

    /// The title set by [`Self::set_title`].
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The title band's own run, upper-cased, and the x offset it starts at from the content's
    /// left edge: past the back glyph and the check-column→label [`GAP`].
    fn title_run(&self, measure: &dyn plx_machine::machine::Measure) -> Option<(String, f32)> {
        if self.is_classic_menu() { return None; }
        let title = self.title.as_ref()?;
        let glyph = measure.width_str(TITLE_BACK_GLYPH, theme::size::CAPTION, false);
        Some((title.to_uppercase(), glyph + GAP))
    }

    /// Replace the contents and reinstate a SAVED view: the selection (`None`: the
    /// [`Self::opening_row`]) snaps with no pill slide and the scroll is put back at `scroll` (read
    /// it with [`Self::scroll_pos`]; clamped by the next [`Self::update`]). The pop half of a
    /// drill-in: the page below comes back exactly where it was left.
    pub fn restore_sections(&mut self, sections: Vec<Section>, sel: Option<i32>, scroll: f32) {
        self.set_sections_or_open(sections, sel, false);
        self.scroll.jump(scroll);
    }

    /// [`Self::set_sections`] with an optional selection: `None` lands on [`Self::opening_row`]
    /// of the NEW sections. Unlike [`Self::open_sections`] it takes `slide`, and with `slide ==
    /// true` it keeps the scroll, which is what a same-page refresh wants.
    pub fn set_sections_or_open(&mut self, sections: Vec<Section>, sel: Option<i32>, slide: bool) {
        self.sections = sections.into();
        let sel = sel.unwrap_or_else(|| self.opening_row());
        let sections = std::mem::take(&mut self.sections).into_vec();
        self.set_sections(sections, sel, slide);
    }

    /// replace the contents and re-anchor selection. `slide=false` snaps the pill to the new
    /// selection (use when the whole list changed, e.g. Audio↔Subtitles); `true` lets it glide.
    pub fn set_sections(&mut self, sections: Vec<Section>, sel: i32, slide: bool) {
        self.rev = next_rev();
        self.sections = sections.into();
        let n = self.n_rows();
        self.sel = if n == 0 { 0 } else { self.settle_sel(sel) };
        if !slide {
            let top = self.row_top(self.sel);
            self.hl_top.jump(top + self.pill_inset());
            self.hl_bot
                .jump(top + self.row_height(self.sel) - self.pill_inset());
            self.scroll.jump(0.0);
        }
    }

    /// **Open** a menu on `sections`: [`Self::set_sections`] with the selection on
    /// [`Self::opening_row`] and the pill snapped there. Every menu that has no prior selection to
    /// restore opens through this, so none can open with its focus on a destructive action.
    pub fn open_sections(&mut self, sections: Vec<Section>) {
        self.sections = sections.into();
        let sel = self.opening_row();
        let sections = std::mem::take(&mut self.sections).into_vec();
        self.set_sections(sections, sel, false);
    }

    /// Where a menu's focus STARTS: the first selectable row that is not [`Row::destructive`]; the
    /// first selectable row when every row is destructive (the menu still has to focus something,
    /// and then the one action on offer is the one asked for); `0` for an empty table.
    pub fn opening_row(&self) -> i32 {
        let n = self.n_rows();
        let selectable = || (0..n).filter(|&i| !self.rows_at(i).sep);
        selectable()
            .find(|&i| !self.rows_at(i).destructive)
            .or_else(|| selectable().next())
            .unwrap_or(0)
    }

    pub fn n_rows(&self) -> i32 {
        self.sections.iter().map(|s| s.rows.len()).sum::<usize>() as i32
    }

    /// The LAST **selectable** row's global index, or `None` for a table with none.
    ///
    /// Not `n_rows() - 1`: [`Row::separator`] rows occupy an index but cannot be landed on, so on
    /// a list that ends in one the last index is a row the selection can never reach. The route
    /// family's "DOWN off the last row enters the action band" rule
    /// ([`crate::route_screen`]'s rule 2) is graded against this, so the band stays reachable
    /// whatever the list ends with.
    pub fn last_row(&self) -> Option<i32> {
        (0..self.n_rows()).rev().find(|&i| !self.rows_at(i).sep)
    }

    /// Is the selection on the last selectable row? — rule 2's own predicate.
    pub fn at_last_row(&self) -> bool {
        self.last_row() == Some(self.sel)
    }

    /// Does the row at `i` OPEN something — i.e. does it wear the drill-in chevron?
    ///
    /// The route family's rule 8 (RIGHT enters nested content) is a statement about the chevron a
    /// row already draws, so it is read off that rather than kept as a second per-screen list that
    /// can drift from what is painted. A toggle row, which changes a value in place, answers
    /// `false`, and RIGHT does nothing on it.
    pub fn row_opens(&self, i: i32) -> bool {
        i >= 0
            && i < self.n_rows()
            && matches!(self.rows_at(i).ticon, Some(crate::icons::Icon::Chevron))
    }

    /// the full drawn height of the content (headers + rows + top/bottom padding) — the owner
    /// sizes its panel to this (clamped) so the panel hugs the list, tvOS-style.
    pub fn measured_height(&self) -> f32 {
        if self.n_rows() == 0 {
            return 120.0;
        }
        self.content_h() + self.vertical_padding()
    }

    /// Intrinsic panel width: the width at which EVERY header, label, sub-line and trailing value
    /// resolves to its full natural width under [`Self::row_columns_under`] /
    /// [`Self::header_columns`] with the device's [`fit::HEADROOM`] to spare. It is the INVERSE of
    /// those two layouts and is built from the same pieces (`row_runs`, [`trailing_reserved`],
    /// [`ROW_VALUE_GAP`], [`ACCESSORY_GAP`], the hug margin), so a panel of exactly this width can
    /// never elide a run the layout would have kept. Action menus size their panel with it.
    pub fn measured_width(&self, measure: &dyn plx_machine::machine::Measure) -> f32 {
        self.fit_layout(measure);
        let h = fit::HEADROOM;
        let mut width: f32 = 0.0;
        if let Some((title, x0)) = self.title_run(measure) {
            width = 2.0 * CONTENT_X + x0 + measure.width_str(&title, theme::size::CAPTION, false) / h;
        }
        for section in &self.sections {
            if !section.header.is_empty() && !self.is_classic_menu() {
                let header = measure.width_str(&section.header.to_uppercase(), theme::size::CAPTION, false);
                let band = if section.accessory.is_empty() {
                    header / h
                } else {
                    let accessory = measure
                        .width_str(&section.accessory, theme::size::MICRO, false)
                        .min(ACCESSORY_W)
                        .max(Self::accessory_floor(section, measure));
                    (header + accessory) / h + ACCESSORY_GAP
                };
                width = width.max(2.0 * CONTENT_X + band);
            }
            // Notes wrap at whitespace, but an individual word cannot be elided. Let the
            // same indivisible run graded by fit_report establish the minimum panel width.
            for row in section.rows.iter().filter(|r| r.is_note()) {
                let widest = Self::widest_note_word(&row.label, measure).0;
                width = width.max(2.0 * CONTENT_X + LABEL_INSET + widest / h);
            }
            for row in section.rows.iter().filter(|row| !row.sep) {
                let (label, value) = self.row_runs(row, measure);
                let detail = if self.is_classic_menu() { 0.0 } else { row.detail_marker_width() + measure.width_str(&row.detail, theme::size::CAPTION, false) };
                let primary = (label / h).max(detail / h);
                let text = match value {
                    Some(value) => primary + ROW_VALUE_GAP + value / h,
                    None => primary,
                };
                let fixed = trailing_reserved(&trailing_items(row, None, measure, self.progress_width(row, 0.0)));
                width = width.max(self.intrinsic_row_width(row, fixed + text, measure));
            }
        }
        width.ceil()
    }

    /// The shared popover-menu width rule: the table's own [`Self::measured_width`] (longest
    /// header / label / detail / trailing value plus `2 * CONTENT_X` of padding and the check
    /// column) clamped to [[`Self::min_panel_w`] (default [`MENU_MIN_W`]), [`MENU_MAX_W`]]. Every TableView-in-popover menu sizes
    /// its panel with this; none carries a width constant of its own.
    ///
    /// [`MENU_MAX_W`] is a localization CATCH, not a silent ellipsis: each menu's fit test grades
    /// its shipped languages at the cap, and [`Self::menu_cap_failure`] flags any draft whose
    /// `measured_width` exceeds it.
    pub fn menu_panel_width(&self, measure: &dyn plx_machine::machine::Measure) -> f32 {
        self.measured_width(measure).clamp(self.min_panel_w.min(MENU_MAX_W), MENU_MAX_W)
    }

    /// [`Self::menu_panel_width`] for a popover that hangs from a header control: never narrower
    /// than `pill`, the control's focused pill ([`crate::widgets::focus_pill`] of its plate),
    /// so a panel never reads as a smaller thing than the control that opened it, and the Library's
    /// Sort & Filter and the Watchlist's size by the same rule. The control's width wins over
    /// content only up to [`MENU_MAX_W`].
    pub fn menu_panel_width_from(&self, measure: &dyn plx_machine::machine::Measure, pill: Rect) -> f32 {
        self.menu_panel_width_to(measure, pill, MENU_MAX_W)
    }

    /// [`Self::menu_panel_width_from`] with the ceiling the caller names instead of
    /// [`MENU_MAX_W`]: a list of USER-NAMED rows (the Library's "+N" libraries) is as wide as its
    /// widest row up to the safe width, so a name is read whole with room to spare and only
    /// marquees beyond it (the board: names are never faded or cut).
    pub fn menu_panel_width_to(&self, measure: &dyn plx_machine::machine::Measure, pill: Rect, ceiling: f32) -> f32 {
        self.measured_width(measure)
            .clamp(self.min_panel_w.min(ceiling), ceiling)
            .max(pill.w.min(ceiling))
    }

    /// Test-side half of the cap: `Err` names the offending width when this table would need a
    /// panel wider than [`MENU_MAX_W`] (`what` = language tag + menu/row context).
    #[cfg(any(test, feature = "test-support"))]
    pub fn menu_cap_failure(&self, measure: &dyn plx_machine::machine::Measure, what: &str) -> Option<String> {
        let w = self.measured_width(measure);
        (w > MENU_MAX_W).then(|| format!("{what}: measured_width {w} > MENU_MAX_W {MENU_MAX_W}"))
    }

    /// Every text slot (label, detail sub-line, trailing value, section header, section accessory)
    /// that a `frame_w`-wide table would end in an ellipsis, with `headroom` of its resolved column
    /// to spare, each tagged with its [`Origin`]. Measure with the device's own advances
    /// (`fontcov::advances::ShippedMeasure`).
    ///
    /// Label and Detail are checked against their column shrunk by `headroom`. Value, Header and
    /// Accessory are the secondary run of a `two_runs` pair, which resolves a fitting secondary to
    /// exactly its natural width, so shrinking the column would fail every fitting run. Those roles
    /// instead re-resolve the pair with EVERY run's natural width divided by `headroom` (glyphs
    /// running wider than the measure predicts, the primary still claiming its share first) and
    /// report the secondary only if its own inflated width no longer fits.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fit_report(&self, frame_w: f32, measure: &dyn plx_machine::machine::Measure, headroom: f32) -> Vec<FitIssue> {
        let (size, bold) = self.label_style();
        let mut out = Vec::new();
        self.fit_notes(frame_w, measure);
        if let Some((title, x0)) = self.title_run(measure) {
            let natural = measure.width_str(&title, theme::size::CAPTION, false);
            let budget = (frame_w - 2.0 * CONTENT_X - x0).max(0.0);
            if budget + FIT_EPS < natural / headroom {
                out.push(FitIssue { role: FitRole::Title, origin: Origin::App, text: title, natural, budget });
            }
        }
        for section in &self.sections {
            if !section.header.is_empty() && !self.is_classic_menu() {
                let header_text = section.header.to_uppercase();
                let header_nat = measure.width_str(&header_text, theme::size::CAPTION, false);
                let (header_w, accessory_w) = Self::header_columns(section, frame_w, measure, headroom);
                if header_w + FIT_EPS < header_nat / headroom {
                    out.push(FitIssue { role: FitRole::Header, origin: section.header_origin, text: header_text, natural: header_nat, budget: header_w });
                }
                if !section.accessory.is_empty() {
                    let accessory_nat = measure.width_str(&section.accessory, theme::size::MICRO, false).min(ACCESSORY_W);
                    if accessory_w + FIT_EPS < accessory_nat / headroom {
                        out.push(FitIssue { role: FitRole::Accessory, origin: section.accessory_origin, text: section.accessory.clone(), natural: accessory_nat, budget: accessory_w });
                    }
                    if !section.accessory_app_prefix.is_empty() {
                        let prefix_nat = measure.width_str(&section.accessory_app_prefix, theme::size::MICRO, false);
                        if accessory_w + FIT_EPS < prefix_nat / headroom {
                            out.push(FitIssue { role: FitRole::Accessory, origin: Origin::App, text: section.accessory_app_prefix.clone(), natural: prefix_nat, budget: accessory_w });
                        }
                    }
                }
            }
            for row in section.rows.iter().filter(|r| r.is_note()) {
                // a note WRAPS, so it fits unless one unbreakable word is wider than its column
                let budget = Self::note_column_w(frame_w) * headroom;
                let widest = Self::widest_note_word(&row.label, measure);
                if widest.0 > budget {
                    out.push(FitIssue { role: FitRole::Note, origin: row.label_origin, text: widest.1.to_string(), natural: widest.0, budget });
                }
            }
            for row in section.rows.iter().filter(|r| !r.sep) {
                let label_budget = self.row_columns(row, frame_w, measure).label_w * headroom;
                let lw = measure.width_str(&row.label, size, bold);
                if lw > label_budget {
                    out.push(FitIssue { role: FitRole::Label, origin: row.label_origin, text: row.label.clone(), natural: lw, budget: label_budget });
                }
                let dw = row.detail_marker_width() + measure.width_str(&row.detail, theme::size::CAPTION, false);
                if !self.is_classic_menu() && dw > label_budget {
                    out.push(FitIssue { role: FitRole::Detail, origin: row.detail_origin, text: row.detail.clone(), natural: dw, budget: label_budget });
                }
                if let Some(value) = row.readout() {
                    let value_nat = measure.width_str(value, self.value_style().0, self.value_style().1);
                    let value_w = self.row_columns_under(row, frame_w, measure, headroom).value_w;
                    if value_w + FIT_EPS < value_nat / headroom {
                        out.push(FitIssue { role: FitRole::Value, origin: row.readout_origin(), text: value.to_string(), natural: value_nat, budget: value_w });
                    }
                }
            }
        }
        out
    }

    /// [`Self::fit_report`] at the device's advances and headroom, reduced to the [`Origin::App`]
    /// findings as `"{tag}: {finding}"` lines. A server-owned overflow is not this app's text to
    /// fix; a caller that wants it too reads `fit_report` directly.
    #[cfg(any(test, feature = "test-support"))]
    pub fn app_fit_failures(&self, frame_w: f32, tag: &str) -> Vec<String> {
        use plx_base::fontcov::advances::ShippedMeasure;
        use crate::fit::HEADROOM;
        self.fit_report(frame_w, &ShippedMeasure, HEADROOM)
            .iter()
            .filter(|i| i.origin == Origin::App)
            .map(|i| format!("{tag}: {}", i.describe()))
            .collect()
    }

    /// [`Self::app_fit_failures`] graded at the width this table's popover ACTUALLY gets
    /// ([`Self::menu_panel_width`] at the device's advances), not at the [`MENU_MAX_W`] ceiling: a
    /// panel that hugs its content must not elide any of it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn app_fit_failures_hugged(&self, tag: &str) -> Vec<String> {
        let w = self.menu_panel_width(&plx_base::fontcov::advances::ShippedMeasure);
        self.app_fit_failures(w, &format!("{tag} @hugged {w}"))
    }

    /// Where a row's read-out ends (its right edge) in a `frame_w`-wide table — the x offset from
    /// the panel's left, the same [`trailing_stack`] edge the draw uses.
    #[cfg(any(test, feature = "test-support"))]
    fn value_right_edge(&self, row: &Row, frame_w: f32) -> f32 {
        let items = trailing_items(row, None, &plx_base::fontcov::advances::ShippedMeasure, self.progress_width(row, frame_w));
        frame_w - SIDE - CONTENT_PAD - trailing_reserved(&items)
    }

    fn widest_note_word<'a>(text: &'a str, measure: &dyn plx_machine::machine::Measure) -> (f32, &'a str) {
        text.split(|c: char| c.is_whitespace() && !matches!(c, '\u{a0}' | '\u{202f}'))
            .filter(|word| !word.is_empty())
            .map(|word| (measure.width_str(word, theme::size::CAPTION, false), word))
            .fold((0.0, ""), |a, b| if b.0 > a.0 { b } else { a })
    }

    /// The text column a [`Row::note`] wraps in: the ROW-LABEL column (past the check column), so
    /// the note reads as attached to the rows above it rather than as a second header.
    fn note_column_w(frame_w: f32) -> f32 {
        (frame_w - 2.0 * CONTENT_X - LABEL_INSET).max(0.0)
    }

    /// **Measure every [`Row::note`] against a `frame_w`-wide table**, storing the wrapped line
    /// count [`Row::height_in`] reads back. `Row::height_in` has no measure, so this is the one
    /// place a note's height is decided; [`Self::draw`] runs it before it walks, and an owner that
    /// sizes its panel from [`Self::measured_height`] (the track menu) runs it first. Idempotent.
    pub fn fit_notes(&self, frame_w: f32, measure: &dyn plx_machine::machine::Measure) {
        self.fit_layout(measure);
        if self.uniform_classic_rows().is_some() { return; }
        let w = Self::note_column_w(frame_w);
        for row in self.sections.iter().flat_map(|s| s.rows.iter()) {
            if !row.is_note() { continue; }
            let lines = crate::text_view::TextView::new(&row.label, theme::size::CAPTION, crate::palette::color(crate::palette::Role::Muted, theme::TEXT_TERTIARY))
                .with_measure(measure)
                .line_count(w);
            row.note_lines.set(lines.clamp(1, u8::MAX as usize) as u8);
            row.note_leading.set(measure.font_scale().points(NOTE_LEADING));
        }
    }

    fn label_style(&self) -> (std::os::raw::c_int, bool) {
        if self.is_classic_list() {
            if let Some((requested, height)) = self.classic_page.get() {
                return (crate::classic::catalogue_text_size(height, requested), false);
            }
        }
        if self.compact || self.classic.get() { (theme::size::BODY, false) } else { (theme::size::HEADLINE, true) }
    }

    fn value_style(&self) -> (std::os::raw::c_int, bool) {
        if self.is_classic_list() { self.label_style() } else { (theme::size::LABEL, VALUE_BOLD != 0) }
    }

    fn catalogue_max_digits(&self) -> usize {
        if !self.is_classic_catalogue() { return 0; }
        let content_rev = self.content_rev();
        if let Some((rev, digits)) = self.catalogue_digits.get() {
            if rev == content_rev { return digits; }
        }
        let digits = self.sections.iter().flat_map(|section| &section.rows)
            .filter(|row| {
                #[cfg(any(test, feature = "test-support"))]
                self.row_visits.set(self.row_visits.get() + 1);
                !row.sep
            })
            .map(|row| {
                row.catalogue_number.as_ref().map_or(0, String::len)
            }).max().unwrap_or(0);
        self.catalogue_digits.set(Some((content_rev, digits)));
        digits
    }

    fn row_content_padding(&self) -> f32 {
        if self.is_classic_list() { 10.0 } else { CONTENT_X }
    }

    fn catalogue_number_width(&self, frame_w: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
        let digits = self.catalogue_max_digits();
        if digits == 0 { return 0.0; }
        // The Classic list occupies 720/1280 of the viewport, on either side.
        // Thus frame_w/720 is the web player's horizontal viewport scale.
        measure.width_str("9", self.label_style().0, false) * digits as f32 + 6.0 * frame_w / 720.0
    }

    fn catalogue_art_width(&self, row: &Row) -> f32 {
        if row.catalogue_art.is_some() || row.catalogue_icon.is_some() {
            (self.plain_row_h()-2.0).max(0.0)*1.5+6.0
        } else { 0.0 }
    }
    fn catalogue_archive_width(row: &Row, frame_w: f32) -> f32 {
        if row.catalogue_archive.is_some() { 9.0*frame_w/720.0 } else { 0.0 }
    }
    fn row_label_inset(&self, row: &Row, frame_w: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
        if self.is_classic_catalogue() {
            self.catalogue_number_width(frame_w, measure)
                + measure.width_str("\u{a0}", self.label_style().0, false)
                + Self::catalogue_archive_width(row,frame_w) + self.catalogue_art_width(row)
        } else if self.is_classic_choices() && !row.checked && row.licon.is_none() {
            measure.width_str("\u{a0}", self.label_style().0, false)
        } else { LABEL_INSET }
    }

    fn progress_width(&self, row: &Row, frame_w: f32) -> Option<f32> {
        // The Classic list is 720/1280 of the viewport; F reserves 40 * wk.
        (self.is_classic_catalogue() && !row.sep && row.progress_interval.is_some())
            .then_some((40.0 * frame_w / 720.0).max(0.0))
    }

    fn intrinsic_row_width(&self, row: &Row, content: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
        if !self.is_classic_catalogue() {
            return 2.0 * self.row_content_padding() + self.row_label_inset(row, 0.0, measure) + content;
        }
        let digits = self.catalogue_max_digits();
        let fixed = 20.0 + content + self.catalogue_art_width(row) + measure.width_str("\u{a0}", self.label_style().0, false)
            + measure.width_str("9", self.label_style().0, false) * digits as f32;
        // Solve the same proportional columns that row_columns reserves:
        // number padding (6 * wk) and optional progress (40 * wk).
        let proportional = (if digits > 0 { 6.0 / 720.0 } else { 0.0 })
            + (if row.catalogue_archive.is_some() { 9.0 / 720.0 } else { 0.0 })
            + (if self.progress_width(row, 0.0).is_some() { 40.0 / 720.0 } else { 0.0 });
        fixed / (1.0 - proportional)
    }

    /// The same label budget used by rendering and intrinsic-width checks.
    pub fn label_width(&self, row: &Row, frame_w: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
        self.row_columns(row, frame_w, measure).label_w
    }

    /// A row's label and trailing-value columns, resolved by priority — content hugging and
    /// compression resistance, measured rather than assumed from English lengths.
    ///
    /// When both runs fit, the value takes its natural width and the label (with its sub-line)
    /// every pixel left. When they do not, the LABEL is the primary read: it keeps its natural
    /// width up to [`fit::ROW_PRIMARY_SHARE`] of the row's text span and the value gives way first,
    /// elided to what is left (see [`fit::two_runs`]).
    pub fn row_columns(&self, row: &Row, frame_w: f32, measure: &dyn plx_machine::machine::Measure) -> RowColumns {
        self.row_columns_under(row, frame_w, measure, 1.0)
    }

    /// [`Self::row_columns`] with both natural widths divided by `headroom` — `1.0` renders, and
    /// [`Self::fit_report`] passes the device's headroom to model glyphs wider than the measure.
    /// The label is hugged with [`fit::HUG_MARGIN`] before dividing.
    fn row_columns_under(&self, row: &Row, frame_w: f32, measure: &dyn plx_machine::machine::Measure, headroom: f32) -> RowColumns {
        let fixed = trailing_reserved(&trailing_items(row, None, measure, self.progress_width(row, frame_w)));
        let span = (frame_w - 2.0 * self.row_content_padding() - self.row_label_inset(row, frame_w, measure) - fixed).max(0.0);
        let (label, value) = self.row_runs(row, measure);
        let Some(value) = value else {
            return RowColumns { label_w: span, value_w: 0.0 };
        };
        let (label_w, value_w) = fit::two_runs(span, ACCESSORY_GAP, fit::Pair {
            primary_nat: label / headroom,
            secondary_nat: value / headroom,
            primary_share: fit::ROW_PRIMARY_SHARE,
        });
        RowColumns { label_w, value_w }
    }

    /// A row's two runs at their natural widths, exactly as [`Self::row_columns_under`] feeds them
    /// to [`fit::two_runs`] at headroom `1.0` and [`Self::measured_width`] sums them: the label
    /// hugged with [`fit::HUG_MARGIN`] (whole pixels), and the trailing value if it has one.
    fn row_runs(&self, row: &Row, measure: &dyn plx_machine::machine::Measure) -> (f32, Option<f32>) {
        let (size, bold) = self.label_style();
        (
            (measure.width_str(&row.label, size, bold) * fit::HUG_MARGIN).ceil(),
            row.readout().map(|v| measure.width_str(v, self.value_style().0, self.value_style().1)),
        )
    }

    /// The width a server-header section guarantees its app-owned accessory text (hugged like a
    /// row label), `0` when the header is not a server one yielding to app text. Shared by
    /// [`Self::header_columns`] and [`Self::measured_width`].
    fn accessory_floor(section: &Section, measure: &dyn plx_machine::machine::Measure) -> f32 {
        let app_text = if !section.accessory_app_prefix.is_empty() {
            Some(section.accessory_app_prefix.as_str())
        } else if section.accessory_origin == Origin::App {
            Some(section.accessory.as_str())
        } else {
            None
        };
        match app_text.filter(|_| section.header_origin == Origin::Server) {
            Some(t) => (measure.width_str(t, theme::size::MICRO, false) * fit::HUG_MARGIN).ceil(),
            None => 0.0,
        }
    }

    /// A [`Section::header`]'s (uppercased, as drawn) and [`Section::accessory`]'s columns, the
    /// header band's counterpart of [`Self::row_columns_under`]: the header is the primary, the
    /// accessory (pre-capped at [`ACCESSORY_W`]) the secondary, so both elide on their own side of
    /// the gap and never overlap. Without an accessory the header keeps the whole span.
    ///
    /// **A SERVER header yields to app-owned accessory text.** When the header is
    /// [`Origin::Server`] and the accessory is app text (or leads with an
    /// [`Section::accessory_app_prefix`]), the roles swap: the accessory is the primary, guaranteed
    /// at least that app text's width (hugged, like a row label), and the server header elides
    /// first. The accessory then elides only its server tail.
    fn header_columns(section: &Section, frame_w: f32, measure: &dyn plx_machine::machine::Measure, headroom: f32) -> (f32, f32) {
        let span = (frame_w - 2.0 * CONTENT_X).max(0.0);
        if section.accessory.is_empty() {
            return (span, 0.0);
        }
        let header_nat = measure.width_str(&section.header.to_uppercase(), theme::size::CAPTION, false) / headroom;
        let accessory_nat = measure.width_str(&section.accessory, theme::size::MICRO, false).min(ACCESSORY_W) / headroom;
        let floor = Self::accessory_floor(section, measure) / headroom;
        if floor == 0.0 {
            return fit::two_runs(span, ACCESSORY_GAP, fit::Pair {
                primary_nat: header_nat,
                secondary_nat: accessory_nat,
                primary_share: fit::ROW_PRIMARY_SHARE,
            });
        }
        let share = floor.max(span * (1.0 - fit::ROW_PRIMARY_SHARE)) / span.max(1.0);
        let (accessory_w, header_w) = fit::two_runs(span, ACCESSORY_GAP, fit::Pair {
            primary_nat: accessory_nat.max(floor),
            secondary_nat: header_nat,
            primary_share: share.min(1.0),
        });
        (header_w, accessory_w)
    }

    /// How opaque a row spanning `y..y + h` is drawn at a PANEL's bottom edge `vis_bot` ([`Self::draw`];
    /// a page's rows are never faded there, see [`Self::draw_page`]).
    ///
    /// The draw hard-clips to its frame, and at the TOP the rows dissolve before it
    /// ([`Self::top_edge_alpha`]). At the BOTTOM of a panel it cut the next row mid-glyph wherever the
    /// frame happened to end — a popover at its height cap. So a row fades as
    /// the edge climbs from the list's own bottom air ([`BOT_PAD`]) up through its empty lower
    /// band ([`ROW_INK_PAD`]), and is fully transparent by the time the edge reaches its ink: the
    /// scissor only ever cuts padding or nothing visible. A row resting with the list's bottom
    /// air below it — where the last row settles — is whole.
    pub fn bottom_edge_alpha(&self, y: f32, h: f32, vis_bot: f32) -> f32 {
        edge_alpha(y + h - ROW_INK_PAD, vis_bot, ROW_INK_PAD)
    }

    /// How much of the ink band of the row spanning content `cy..cy + h` the focus pill covers
    /// RIGHT NOW, 0..1 — the ONE rule a row's ink is derived from, so the ink can never disagree
    /// with the fill sliding under it (the same reason `widgets::cap_cover` exists for a tab strip).
    ///
    /// The ink band is the row less its empty [`ROW_INK_PAD`] above and below, and less the
    /// [`PILL_INSET`] the pill itself keeps off the row's edge (a two-line row's title caps start
    /// ~15px under its top, measured on a sim capture), so the label only goes fully dark once the
    /// pill's edge has passed its caps and goes back to light as the pill leaves —
    /// reading the SELECTION instead (as this once did) flipped the next row's label to the pill's
    /// near-black ink on the frame focus moved, a row before the pill got there: dark text on the
    /// dark ground. Linear in coverage, like the strip's; shape it HERE if a device capture ever
    /// asks, not with a second spring (the ink would start leading the fill). A table that does not
    /// hold focus draws no pill and so covers nothing. At rest on a row the answer is exactly 1.
    fn pill_cover(&self, cy: f32, h: f32) -> f32 {
        if !self.list_focused {
            return 0.0;
        }
        let inset = if self.is_classic_list() { 0.0 } else { ROW_INK_PAD + PILL_INSET };
        let (top, bot) = (cy + inset, cy + h - inset);
        if bot <= top {
            return 0.0;
        }
        ((self.hl_bot.pos.min(bot) - self.hl_top.pos.max(top)) / (bot - top)).clamp(0.0, 1.0)
    }

    /// How opaque ink whose top is at `ink_top` is drawn under the viewport's top edge `vis_top`:
    /// whole once it is [`TOP_FADE_H`] below it, gone as it reaches it. The top edge DISSOLVES a row
    /// rather than cutting it (rows scroll up under the crumb band); the scissor still clips the
    /// frame, but by then the ink it would cut is transparent.
    fn top_edge_alpha(ink_top: f32, vis_top: f32) -> f32 {
        ((ink_top - vis_top) / TOP_FADE_H).clamp(0.0, 1.0)
    }

    /// The nearest **selectable** row to `i`: `i` itself when it is one, else the first non-separator
    /// after it, else the last one before it — so selection cannot come to rest on a grouping
    /// hairline, whoever set it. The one exception is a list that is ALL separators, which has no
    /// selectable row to offer and gets `i` back; that is degenerate rather than defended against
    /// (nothing can commit — `on_ok` reads no action off a hairline — and nothing panics).
    fn settle_sel(&self, i: i32) -> i32 {
        let n = self.n_rows();
        if n == 0 {
            return 0;
        }
        let i = i.clamp(0, n - 1);
        if !self.rows_at(i).sep {
            return i;
        }
        (i + 1..n)
            .find(|&j| !self.rows_at(j).sep)
            .or_else(|| (0..i).rev().find(|&j| !self.rows_at(j).sep))
            .unwrap_or(i)
    }

    /// Move the selection `delta` **selectable** rows: a separator is skipped rather than landed on,
    /// so one DOWN across the item menu's divider lands on the first state action, not on the rule.
    /// A step that would run off either end is dropped (the selection stays put), matching the old
    /// clamp.
    pub fn move_sel(&mut self, delta: i32) {
        let n = self.n_rows();
        if n == 0 {
            return;
        }
        let step = if delta < 0 { -1 } else { 1 };
        let mut cur = self.settle_sel(self.sel);
        for _ in 0..delta.abs() {
            let mut j = cur + step;
            while j >= 0 && j < n && self.rows_at(j).sep {
                j += step;
            }
            if j < 0 || j >= n {
                break;
            }
            cur = j;
        }
        self.sel = cur;
    }

    /// Certify the narrow layout whose old walk is exactly `index * plain_height`.
    /// Shape is checked once per mutable contents access; mode and measured height
    /// stay live. Unknown, decorated and variable-height lists use the full walk.
    fn uniform_classic_rows(&self) -> Option<(&[Row], f32)> {
        self.classic_page_rows()?;
        if self.title.is_some() || self.sections.len() != 1 { return None; }
        let section = &self.sections[0];
        if !section.header.is_empty() || section.rows.is_empty() { return None; }
        let height = self.plain_row_h();
        // Every addition in the old f32 walk is exact in this integer range.
        // Beyond it (or at fractional heights), retain that walk's rounding.
        if !height.is_finite() || height <= 0.0 || height.fract() != 0.0
            || section.rows.len() as f64 * f64::from(height) > 16_777_216.0 {
            return None;
        }
        let plain = self.sections.plain_rows.get().unwrap_or_else(|| {
            let plain = section.rows.iter().all(|row| {
                !row.sep && row.detail.is_empty()
            });
            self.sections.plain_rows.set(Some(plain));
            plain
        });
        plain.then_some((section.rows.as_slice(), height))
    }

    /// Preserve the draw's inclusive edge test while visiting only its interval.
    /// Bounds use the SAME f32 screen-space expression as the full walk, so an
    /// animated fractional scroll and exact edge ties do not change membership.
    /// Binary search performs only scalar geometry; no offscreen row is read.
    fn walk_window(&self, top0: f32, scroll: f32, top: f32, bottom: f32,
        mut f: impl FnMut(f32, i32, usize, Option<&Row>)) {
        if let Some((rows, height)) = self.uniform_classic_rows().filter(|_| {
            top0.is_finite() && scroll.is_finite() && top.is_finite()
                && bottom.is_finite() && top <= bottom
        }) {
            fn partition(len: usize, mut before: impl FnMut(usize) -> bool) -> usize {
                let (mut lo, mut hi) = (0, len);
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if before(mid) { lo = mid + 1; } else { hi = mid; }
                }
                lo
            }
            let y = |i: usize| top0 + i as f32 * height - scroll;
            let first = partition(rows.len(), |i| y(i) + height < top);
            let end = partition(rows.len(), |i| y(i) <= bottom);
            for (offset, row) in rows[first..end].iter().enumerate() {
                let i = first + offset;
                f(i as f32 * height, i as i32, 0, Some(row));
            }
        } else {
            self.walk(f);
        }
    }

    /// walk the visual layout top-to-bottom, invoking `f(content_y, global_row_index_or_-1, sec)`
    /// once per header (gi = -1) and once per row (gi >= 0). Row references keep
    /// callers from performing another section lookup for each layout event.
    fn walk(&self, mut f: impl FnMut(f32, i32, usize, Option<&Row>)) {
        let mut y = 0.0f32;
        let mut gi = 0i32;
        if self.title.is_some() && !self.is_classic_menu() {
            f(y, WALK_TITLE, 0, None);
            y += self.header_h() + TITLE_GAP + DIV_H;
            f(y, WALK_DIVIDER, 0, None);
        }
        for (si, sec) in self.sections.iter().enumerate() {
            // an EMPTY section (a `Form` whose items all fell through) has nothing to divide from
            // the one above: no hairline, no gap, and it must not lengthen the content
            if si > 0 && !sec.rows.is_empty() && !self.is_classic_menu() {
                y += DIV_H;
                f(y, WALK_DIVIDER, si, None);
            }
            if !sec.header.is_empty() && !self.is_classic_menu() {
                f(y, -1, si, None);
                y += self.header_h();
            }
            for row in &sec.rows {
                #[cfg(any(test, feature = "test-support"))]
                self.row_visits.set(self.row_visits.get() + 1);
                f(y, gi, si, Some(row));
                y += self.presented_row_height(row);
                gi += 1;
            }
        }
    }

    fn row_top(&self, target: i32) -> f32 {
        if let Some((rows, height)) = self.uniform_classic_rows() {
            return if target >= 0 && (target as usize) < rows.len() {
                target as f32 * height
            } else { 0.0 };
        }
        let mut out = 0.0;
        self.walk(|y, gi, _, _| {
            if gi == target {
                out = y;
            }
        });
        out
    }
    fn row_height(&self, target: i32) -> f32 {
        if let Some((_, height)) = self.uniform_classic_rows() { return height; }
        let mut n = 0i32;
        for sec in &self.sections {
            for row in &sec.rows {
                if n == target {
                    return self.presented_row_height(row);
                }
                n += 1;
            }
        }
        self.plain_row_h()
    }
    fn content_h(&self) -> f32 {
        if let Some((rows, height)) = self.uniform_classic_rows() {
            return rows.len() as f32 * height;
        }
        // the LAST ROW's bottom, not the last walk event's: a trailing header or divider event
        // (an empty section) is not content
        let mut h = 0.0;
        self.walk(|y, gi, _, _| {
            if gi >= 0 {
                h = y;
            }
        });
        h + self.row_height(self.n_rows() - 1)
    }

    /// `frame_h` is the SAME rect height passed to [`Self::draw`]/[`Self::hit_row`] — this
    /// function applies its own vertical padding (zero for a Classic catalogue),
    /// so a caller must not subtract it a second time.
    pub fn update(&mut self, dt: f32, frame_h: f32) {
        if self.n_rows() == 0 {
            return;
        }
        let visible_h = (frame_h - self.vertical_padding()).max(0.0);
        let top = self.row_top(self.sel);
        let rh = self.row_height(self.sel);
        // Classic follows the web player's discrete row/page changes. Advancing
        // two highlight springs and a separate scroll spring showed half a row
        // before the final selection, especially at the TV's lower frame rate.
        // Modern retains its independent spring-driven highlight treatment.
        if self.is_classic() {
            self.hl_top.jump(top + self.pill_inset());
            self.hl_bot.jump(top + rh - self.pill_inset());
        } else {
            self.hl_top.step(top + self.pill_inset(), 360.0, dt);
            self.hl_bot.step(top + rh - self.pill_inset(), 360.0, dt);
        }
        if let Some(rows) = self.classic_page_rows() {
            // A page is the same row interval used by catalogue_next. The last
            // partial page retains its origin rather than revealing prior rows.
            let first = (self.sel.max(0) as usize / rows) * rows;
            self.scroll.jump(first as f32 * self.plain_row_h());
            return;
        }
        // keep the selection (plus a row of context) inside the viewport
        let content_h = self.content_h();
        let max_scroll = (content_h - visible_h).max(0.0);
        let mut sc = self.scroll.pos;
        let context = ((visible_h - rh) * 0.5).clamp(0.0, rh);
        if top - context < sc {
            sc = top - context;
        }
        // A non-focusable explanation after the final control must still be readable.
        // Reveal that tail when it fits beside the selected row; ordinary rows retain the
        // same one-row context and no note can push the selected control out of view.
        let mut has_note = false;
        let only_note_tail = self.sections.iter().flat_map(|section| &section.rows)
            .skip(self.sel.max(0) as usize + 1)
            .all(|row| { has_note |= row.is_note(); row.sep });
        let below = if only_note_tail && has_note {
            context.max((content_h - top - rh).min((visible_h - rh).max(0.0)))
        } else {
            context
        };
        if top + rh + below > sc + visible_h {
            sc = top + rh + below - visible_h;
        }
        sc = sc.clamp(0.0, max_scroll);
        if self.is_classic() { self.scroll.jump(sc); }
        else { self.scroll.step(sc, 300.0, dt); }
    }

    /// The highlight springs as `(top position, top velocity, bottom position, bottom velocity)`.
    /// Test-only: screens need to prove they actually advance the shared table motion without
    /// exposing its implementation as product API.
    #[cfg(any(test, feature = "test-support"))]
    pub fn highlight_motion(&self) -> (f32, f32, f32, f32) {
        (
            self.hl_top.pos,
            self.hl_top.vel,
            self.hl_bot.pos,
            self.hl_bot.vel,
        )
    }

    /// The scroll spring's live position, in content coordinates.
    ///
    /// It was `#[cfg(test)]` — the tests are still its main reader, for the reason below — until a
    /// caller needed to draw a SCROLL RAIL beside the list (`ui::filmography`). A rail is the one
    /// thing outside this widget that has to know where the scroll actually is; everything else it
    /// exposes is about rows.
    ///
    /// For tests: this is what a regression on the `update(dt, frame_h)` contract shows up as
    /// first — the pill motion test cannot see it, since a 2-row list never scrolls.
    pub fn scroll_pos(&self) -> f32 {
        self.scroll.pos
    }

    /// Draw as a PANEL: a popover or menu whose `frame` is a real edge, so rows are scissored to it
    /// and fade out before the scissor can reach their ink ([`Self::bottom_edge_alpha`]).
    pub fn draw(&self, p: Painter, frame: Rect, measure: &dyn plx_machine::machine::Measure) {
        self.draw_in(p, frame, false, measure)
    }

    /// Draw as a PAGE's content column (`TablePart`, the Filmography page): `frame` ends at the
    /// safe area, but the page does not — the screen's bottom edge is the real one. Rows scrolling down simply run off it, uncut and
    /// unfaded (the fade at the safe-area line hid the rows a page scrolls to, with the margin
    /// below it empty). The TOP still dissolves ([`Self::top_edge_alpha`]), and layout, scrolling
    /// and hit rects are the same as [`Self::draw`]'s — only what is DRAWN below `frame` differs.
    pub fn draw_page(&self, p: Painter, frame: Rect, measure: &dyn plx_machine::machine::Measure) {
        self.draw_in(p, frame, true, measure)
    }

    fn draw_in(&self, p: Painter, frame: Rect, page: bool, measure: &dyn plx_machine::machine::Measure) {
        let classic = self.classic.get();
        let white = if classic { crate::palette::color(crate::palette::Role::Text, theme::CLASSIC_TEXT) } else { crate::palette::color(crate::palette::Role::Text, theme::TEXT_PRIMARY) };
        let dimc = if classic { crate::palette::color(crate::palette::Role::Muted, theme::CLASSIC_SECONDARY) } else { crate::palette::color(crate::palette::Role::Muted, theme::TEXT_TERTIARY) };
        let ink = if classic { crate::palette::color(crate::palette::Role::SelectionText, theme::CLASSIC_SELECTION_INK) } else { crate::palette::color(crate::palette::Role::SelectionText, crate::ACCENT_INK) };
        let selection = if classic { crate::palette::color(crate::palette::Role::Selection, theme::CLASSIC_SELECTION) } else { crate::palette::color(crate::palette::Role::Selection, crate::ACCENT) };
        let background = if classic { crate::palette::color(crate::palette::Role::Background, theme::CLASSIC_BACKGROUND) } else { crate::palette::color(crate::palette::Role::Surface, PANEL_BG) };
        let border = if classic { crate::palette::color(crate::palette::Role::Border, theme::CLASSIC_BORDER) } else { crate::palette::color(crate::palette::Role::Border, theme::HAIRLINE) };
        let header_ink = if classic { crate::palette::color(crate::palette::Role::Muted, theme::CLASSIC_SECONDARY) } else { crate::palette::color(crate::palette::Role::Muted, self.header_ink) };
        if self.n_rows() == 0 {
            Label::new(
                plx_platform::i18n::msg::widgets_tracks_empty_c().as_ptr(),
                theme::size::BODY,
                dimc,
            )
            .h(HAlign::Center)
            .draw(p, frame);
            return;
        }
        self.fit_notes(frame.w, measure);
        // Hard-clip everything below to the panel frame: the list overflows, so a partial edge row is
        // cut cleanly at the frame instead of poking over the video / control buttons. Rows are
        // faded as WHOLE rows by their ink position (never a per-pixel mask), so a tall two-line
        // row stays uniform however close it is to an edge. A page's clip runs on to the bottom of
        // the screen instead ([`Self::draw_page`]). A `ClipScope`, so it intersects with an
        // enclosing panel clip and restores it when this fn returns.
        let clip = if page && !self.classic.get() {
            Rect::new(frame.x, frame.y, frame.w, (crate::consts::SCR_H - frame.y).max(frame.h))
        } else {
            self.content_clip(frame)
        };
        let _clip = crate::screen::ClipScope::open_in(p, clip);
        let top0 = frame.y + self.top_padding();
        let scroll = self.scroll.pos;
        let vis_top = frame.y;
        let vis_bot = clip.y + clip.h;
        // the bottom edge fades a panel's rows out before its scissor; a page has no such edge
        let bottom_alpha = |ink_bot: f32, pad: f32| if page { 1.0 } else { edge_alpha(ink_bot, vis_bot, pad) };

        let content_x = frame.x + self.row_content_padding();
        let text_right = frame.x + frame.w - self.row_content_padding();

        // ---- sliding pill (under the rows) — warm off-white; top/bottom edges morph independently ----
        // It follows its GROUP's dim: the focused row of an unreachable server must not be the one
        // bright thing in a panel that is telling you the server is unreachable.
        let py0 = top0 + self.hl_top.pos - scroll;
        let py1 = top0 + self.hl_bot.pos - scroll;
        let pill = Rect::new(
            frame.x + self.side_padding(),
            py0,
            frame.w - 2.0 * self.side_padding(),
            (py1 - py0).max(1.0),
        );
        if self.list_focused && pill.y + pill.h > vis_top && pill.y < vis_bot {
            // fades with its row's ink under the top edge (focus keeps a row of context above it,
            // so this only shows while the scroll is still catching up)
            let fade = if self.is_classic_list() { 1.0 }
                else { Self::top_edge_alpha(py0 - PILL_INSET + ROW_INK_PAD, vis_top) };
            self.group_painter(p.alpha(fade), self.section_of_row(self.sel)).rrect(
                pill,
                if self.classic.get() { theme::space::XS } else { PILL_RAD },
                if self.classic.get() { theme::space::XS } else { PILL_RAD },
                selection,
            );
        }

        // ---- headers + rows ----
        self.walk_window(top0, scroll, vis_top, vis_bot, |cy, gi, si, row| {
            // ONE alpha over the whole group (see `Section::dim`) — pushed here, at the top of the
            // walk, so header, accessory, rows, marks and read-outs can never dim out of step.
            let sy = top0 + cy - scroll;
            let p = self.event_painter(p, gi, si);
            if gi == WALK_DIVIDER {
                // the hairline between this section and the one above, headed or not, centred in
                // the `DIV_H` gap; it fades with the group like everything else in it
                let y = sy - DIV_H * 0.5;
                if y + 2.0 > vis_top && y < vis_bot {
                    // dissolves with the section it heads, a hair before its caps
                    p.alpha(Self::top_edge_alpha(sy, vis_top)).rect(
                        Rect::new(content_x, y, frame.w - 2.0 * (SIDE + CONTENT_PAD), 2.0),
                        0.0,
                        border,
                        border,
                        0.0,
                    );
                }
                return;
            }
            if gi == WALK_TITLE {
                if sy + self.header_h() > vis_top && sy < vis_bot {
                    let p = p.alpha(
                        bottom_alpha(sy + self.header_h() - HDR_INK_PAD, HDR_INK_PAD)
                            * Self::top_edge_alpha(sy + HEADER_CAP_INSET, vis_top),
                    );
                    let hsz = theme::size::CAPTION;
                    let (cap_top, baseline) = crate::typography::text_cap_band(hsz, 0);
                    let band =Rect::new(content_x, sy + HEADER_CAP_INSET, (text_right - content_x).max(0.0), baseline - cap_top);
                    if let Some((title, x0)) = self.title_run(measure) {
                        let glyph = CString::new(TITLE_BACK_GLYPH).unwrap_or_default();
                        Label::new(glyph.as_ptr(), hsz, header_ink).v(VAlign::CapTop).draw(p, band);
                        let text_band = Rect::new(band.x + x0, band.y, (band.w - x0).max(0.0), band.h);
                        let text = plx_gfx::text::elide_by(&title, text_band.w, false, |t| measure.width_str(t, hsz, false));
                        if let Ok(cs) = CString::new(text) {
                            Label::new(cs.as_ptr(), hsz, header_ink).v(VAlign::CapTop).draw(p, text_band);
                        }
                    }
                }
                return;
            }
            if gi == -1 {
                // panel/section header; scissor-clipped to `frame`
                if sy + self.header_h() > vis_top && sy < vis_bot {
                    // dissolves under the top edge, and a panel's fades out at the bottom edge before
                    // its caps are cut (`bottom_edge_alpha`)
                    let p = p.alpha(
                        bottom_alpha(sy + self.header_h() - HDR_INK_PAD, HDR_INK_PAD)
                            * Self::top_edge_alpha(sy + HEADER_CAP_INSET, vis_top),
                    );
                    let sec = &self.sections[si];
                    // **CAPS at CAPTION, one size in BOTH size classes.** The caps are what make a
                    // header read as a label rather than as a row, which is why the size stops
                    // varying: at HEADLINE — what the non-compact class used to draw — a header
                    // ties with or outweighs the rows it heads, and on the Sources panel (a
                    // two-line picker, so non-compact) that put the machine names on screen bigger
                    // than the libraries they name. Design system, `TableView.prompt.md`.
                    //
                    // `to_uppercase`, not `to_ascii_uppercase`: a header is a machine name or a
                    // library name and can be any script — the share measured here is Cyrillic.
                    let hsz = theme::size::CAPTION;
                    let cap_y = sy + HEADER_CAP_INSET;
                    let (cap_top, baseline) = crate::typography::text_cap_band(hsz, 0);
                    let header_band = Rect::new(
                        content_x,
                        cap_y,
                        (text_right - content_x).max(0.0),
                        baseline - cap_top,
                    );
                    let (header_w, accessory_w) = Self::header_columns(sec, frame.w, measure, 1.0);
                    let header_text = plx_gfx::text::elide_by(&sec.header.to_uppercase(), header_w, false, |t| {
                        measure.width_str(t, hsz, false)
                    });
                    if let Ok(cs) = CString::new(header_text) {
                        Label::new(cs.as_ptr(), hsz, header_ink)
                            .v(VAlign::CapTop)
                            .draw(p, header_band);
                    }
                    // trailing accessory, one rung down and on the header's own baseline. Elided,
                    // because it is the run that gives way (see `Section::accessory`).
                    if !sec.accessory.is_empty() {
                        // MICRO, a rung below the header: "the header names the group, the
                        // accessory only qualifies it, so it never ties with the header"
                        // (`TableView.prompt.md`). At CAPTION the two were the same size and a
                        // plex.tv handle read as loud as the machine it hangs off.
                        let asz = theme::size::MICRO;
                        let a = plx_gfx::text::elide_by(&sec.accessory, accessory_w, false, |t| {
                            measure.width_str(t, asz, false)
                        });
                        if let Ok(ac) = CString::new(a) {
                            Label::new(ac.as_ptr(), asz, header_ink)
                                .h(HAlign::Right)
                                .v(VAlign::Baseline)
                                .draw(p, header_band);
                        }
                    }
                }
                return;
            }
            let Some(row) = row else { return; };
            let h = self.presented_row_height(row);
            if sy + h < vis_top || sy > vis_bot {
                return; // fully scrolled out; a partial edge row is drawn and scissor-clipped to `frame`
            }
            // …dissolved under the TOP edge, and (a panel's rows only) faded out at the BOTTOM edge
            // before the scissor can reach its ink
            let edge = if self.is_classic_list() { 1.0 }
                else { bottom_alpha(sy + h - ROW_INK_PAD, ROW_INK_PAD) * Self::top_edge_alpha(sy + ROW_INK_PAD, vis_top) };
            if edge <= 0.0 {
                return;
            }
            let p = p.alpha(edge);
            if row.sep && row.label.is_empty() {
                // grouping hairline, on the row's centre line and inset to the label column so it
                // reads as a divider between groups rather than a full-bleed panel rule
                p.rect(
                    Rect::new(
                        content_x,
                        sy + h * 0.5 - 1.0,
                        frame.w - 2.0 * (SIDE + CONTENT_PAD),
                        2.0,
                    ),
                    0.0,
                    border,
                    border,
                    0.0,
                );
                return;
            }
            if row.sep {
                // a `note` row: quiet CAPTION text WRAPPED in the row-label column, never a mark or
                // a pill — it cannot be focused (skipped by every `sep` check above), so it never
                // draws over the sliding highlight either. Its height is `Row::height_in`'s.
                let nsz = theme::size::CAPTION;
                let x = content_x + LABEL_INSET;
                let view = crate::text_view::TextView::new(&row.label, nsz, dimc)
                    .with_measure(measure)
                    .leading(NOTE_LEADING);
                let w = (text_right - x).max(0.0);
                let lines = view.line_count(w);
                // centre the INK (first cap-top to last baseline), as the one-line note was
                let ink_h = (lines.saturating_sub(1)) as f32 * measure.font_scale().points(NOTE_LEADING) + measure.cap_h(nsz);
                view.draw(p, Rect::new(x, sy + (h - ink_h) * 0.5, w, ink_h));
                return;
            }
            // **The INK follows the PILL, and `list_focused` gates both.** `ink` is near-black and
            // is only legible ON the accent pill, so a row wears it exactly as far as the pill
            // currently covers its ink band (`pill_cover`) — not from the frame focus moves to it,
            // when the pill is still a row away and the label read black on the dark ground. A
            // table that holds no focus draws no pill and covers nothing; suppressing the pill
            // while still flipping the ink drew black-on-panel rows, which is what the owner saw
            // the moment focus moved to the Sources panel's level pills. `focused` stays the
            // SELECTION (the marquee runs on the selected row).
            let focused = gi == self.sel && self.list_focused;
            let cover = self.pill_cover(cy, h);
            // `cross` at the ends is exact, so a row at rest on either side is the colour it always was
            let on_pill = |rest: [f32; 4], over: [f32; 4]| match cover {
                c if c <= 0.0 => rest,
                c if c >= 1.0 => over,
                c => theme::cross(rest, over, c),
            };
            let base = on_pill(if row.dim { dimc } else { white }, ink);
            let row_bg = on_pill(background, selection);
            let cyc = sy + h * 0.5; // row vertical center

            // Leading column (SVG): the PICKER's tick, or an ACTION's glyph — one or the other, and
            // never a switch. A mark here says WHERE YOU ARE; what a row is SET to is a word at the
            // trailing edge (below), and no row is allowed to say both.
            let label_x = content_x + self.row_label_inset(row, frame.w, measure);
            if self.is_classic_catalogue() {
                if let Some(number) = &row.catalogue_number {
                    if let Ok(number) = CString::new(number.as_str()) {
                        let width = self.catalogue_number_width(frame.w, measure);
                        let _clip = crate::screen::ClipScope::open_in(p, Rect::new(content_x, sy, width, h));
                        Label::new(number.as_ptr(), self.label_style().0, base)
                            .h(HAlign::Right).draw(p, Rect::new(content_x, sy, width, h));
                    }
                }
                let archive_width=Self::catalogue_archive_width(row,frame.w);
                if row.catalogue_archive==Some(true) {
                    let bar=archive_width/3.0;
                    let x=content_x+self.catalogue_number_width(frame.w,measure)+bar;
                    let height=(h-2.0*bar).max(0.0);
                    let color=on_pill(theme::CLASSIC_ARCHIVE_AVAILABLE,ink);
                    p.rect(Rect::new(x,cyc-height*0.5,bar,height),bar*0.5,color,color,0.0);
                }
                if row.catalogue_art.is_some() || row.catalogue_icon.is_some() {
                    let number_width = self.catalogue_number_width(frame.w, measure);
                    let icon_x = content_x + number_width + archive_width + measure.width_str("\u{a0}", self.label_style().0, false);
                    let art_h=(h-2.0).max(0.0);
                    let frame = Rect::new(icon_x, cyc - art_h*0.5, art_h*1.5, art_h);
                    if let Some(source) = row.catalogue_art.as_ref() {
                        let art = match source {
                            RowArt::Local(key) => crate::widgets::Art::Local { key },
                            RowArt::Http(url) => crate::widgets::Art::Http { url, res: (96, 64) },
                            RowArt::Plex { server, key } => crate::widgets::Art::Thumb { sid: *server, key, res: (96, 64) },
                        };
                        crate::widgets::card_contained(p, frame, art, 3.0);
                    } else if let Some(icon) = row.catalogue_icon {
                        crate::icons::draw(p, icon, Rect::new(icon_x+(frame.w-art_h)*0.5,cyc-art_h*0.5,art_h,art_h), base);
                    }
                }
            }
            let lead = if self.is_classic_catalogue() {
                None
            } else if row.checked {
                Some(crate::icons::Icon::Check)
            } else {
                row.licon
            };
            if let Some(li) = lead {
                let cs = 26.0f32;
                let cr = Rect::new(content_x + (CHECK_W - cs) * 0.5, cyc - cs * 0.5, cs, cs);
                crate::icons::draw(p, li, cr, base);
            }
            // The trailing run — read-out, badges, icon, optional progress — is ONE stack ([`trailing_items`]),
            // anchored at `text_right`; each part below is drawn where it stands.
            // The read-out is flush right at its item's trailing edge (below), so its width moves
            // nothing the draw reads: a zero-wide item places the same edge, and costs no measure.
            let trailing_run = trailing_items(row, row.readout().map(|_| 0.0), measure, self.progress_width(row, frame.w));
            let trailing_at = |i: usize| trailing_stack(&trailing_run, text_right).placed().find(|s| s.i == i);
            if let Some(at) = trailing_at(3) {
                let bar = progress_frame(at, sy, h);
                paint_progress(p, bar, row.progress_fraction(self.classic_clock.get()),
                    on_pill(crate::palette::color(crate::palette::Role::Accent, theme::CLASSIC_PROGRESS), crate::palette::color(crate::palette::Role::SelectionText, theme::CLASSIC_SECONDARY_ON_SELECTION)));
            }
            // trailing accessory (SVG), its BOX at the stack's `box_x` so its ink ends on the edge
            if let (Some(ti), Some(at)) = (row.ticon, trailing_at(2)) {
                let cs = ACCESSORY_ICON_W;
                crate::icons::draw(p, ti, Rect::new(at.box_x, cyc - cs * 0.5, cs, cs), base);
            }
            // PLACE 4 — the badge run, RIGHT-ALIGNED at the trailing edge and the outermost of the
            // three trailing runs (the design system's cell is a flex row whose label block takes
            // all the slack, so `badge` — written last — is flush right, with the read-out beside
            // it). It was drawn INLINE, starting at the label's own drawn right edge, which gave a
            // chip the design pins flush a per-ROW x: a column of resolution classes, or of
            // FORCED/SDH/codec tags, landed wherever each label happened to end, so the panel read
            // as ragged text rather than as a column you can compare straight down.
            //
            // Centred on the ROW BOX (`cyc`), not on the title's cap band: the chip is a sibling of
            // the whole label block, so on a two-line row it sits level with the pair rather than
            // ~16px high, level with the first line.
            if !row.badges.is_empty() {
                // one measuring pass: laid out from 0, the last chip's trailing edge IS the run's
                // width, and each chip is then drawn at `start + its offset`
                let laid: Vec<(&Badge, Span)> = badge_spans(&row.badges, 0.0, measure).collect();
                let start = trailing_at(1).map_or(text_right, |s| s.x);
                for (b, span) in laid {
                    // the shared chip leaf in this row's contextual colours: the row's own ink for
                    // the label, the design's keyline for the ring (the pill's near-black ink over
                    // a focused row, where a 55%-white stroke would vanish), the row's ground
                    // knocked out of the interior
                    let sty = crate::widgets::BadgeStyle::Outlined {
                        col: base,
                        border: on_pill(if classic { border } else { crate::palette::color(crate::palette::Role::Border, theme::OVERLAY_BORDER) }, ink),
                        bg: row_bg,
                    };
                    crate::widgets::badge(p, start + span.x, cyc, b.text(), None, sty, measure);
                }
            }
            // Trailing VALUE — the read-out that says what this row is set to ("On"/"Off" for a
            // switch, or any word). One step behind the label in ink, so the label is what you read
            // and the value is what you check; over the focused row's near-white pill that step is
            // an alpha of the pill's own ink rather than a grey, which would go muddy on it. That
            // step is the ink's ALONE — the run itself is bold (see [`VALUE_BOLD`], which all three
            // calls below take so the measure and the paint can never be two different faces).
            if let Some(v) = row.readout() {
                let ink = if classic {
                    let dim = row.dim || row.value_dim;
                    on_pill(if dim { theme::with_a(dimc, 0.6) } else { dimc },
                        if dim { theme::with_a(ink, 0.6) } else { ink })
                } else { on_pill(row_value_ink(row, false), row_value_ink(row, true)) };
                // the value gives way before the label (`row_columns`): elided to its column
                let (vsz, value_bold) = self.value_style();
                let value_bold = i32::from(value_bold);
                let value_w = self.row_columns(row, frame.w, measure).value_w;
                let v = plx_gfx::text::elide_by(v, value_w, false, |t| {
                    measure.width_str(t, vsz, value_bold != 0)
                });
                if let Ok(vc) = std::ffi::CString::new(v) {
                    let vy = crate::typography::text_vcenter_y(vsz, value_bold, cyc);
                    p.text(
                        vc.as_ptr(),
                        trailing_at(0).map_or(text_right, |s| s.x + s.w),
                        vy,
                        vsz,
                        ink,
                        2,
                        value_bold,
                    );
                }
            }
            // Single-line rows centre their label on the row by cap band. Two-line rows stack a
            // title over a detail sub-line: lay the pair out off both cap bands and centre it in the
            // tall row, with an explicit gap between the title baseline and the detail cap-top
            // (layout ≠ paint — the old fixed offsets left the two lines cramped after centring).
            let two_line = !self.is_classic_menu() && !row.detail.is_empty();
            // Paint the same size and weight that row fitting and column measurement use.
            let (tsz, title_bold) = self.label_style();
            let tbold = i32::from(title_bold);
            let (title_y, detail_y) = if two_line {
                let (t_top, t_base) = crate::typography::text_cap_band(tsz, tbold);
                let (d_top, d_base) = crate::typography::text_cap_band(theme::size::CAPTION, 0);
                // The pair's cap bands, one stack: title cap-top, then the detail cap-top one
                // ROW_SUB_GAP below the title's baseline, the pair centred in the row.
                let items = [Item::new(t_base - t_top), Item::new(d_base - d_top)];
                let pair = VStack::new(&items, ROW_SUB_GAP, Anchor::Centre(sy + h * 0.5));
                let (Some((title_cap, _)), Some((detail_cap, _))) = (pair.span(0), pair.span(1)) else {
                    unreachable!("the title and the detail line are always present")
                };
                (title_cap - t_top, detail_cap - d_top)
            } else {
                (0.0, 0.0) // unused single-line; Label centres the label
            };
            // PLACE 2 — single-line labels share the measured style of two-line titles.
            let (lsz, lbold) = (tsz, tbold);
            let text_w = self.label_width(row, frame.w, measure);
            let secondary = on_pill(dimc, crate::palette::color(crate::palette::Role::SelectionText, theme::CLASSIC_SECONDARY_ON_SELECTION));
            let split = (self.is_classic_catalogue() && !two_line
                && !plx_gfx::text::needs_bidi_context(&row.label))
                .then(|| row.secondary_label_start()).flatten();
            // Preserve the original whole-run path for any bidi context.
            // The suffix changes tone only: same face, advances, baseline and marquee.
            let paint_label = |text: &str, x: f32, y: f32| {
                if let Some(at) = split.filter(|at| *at < text.len() && text.is_char_boundary(*at)) {
                    let (primary, tail) = text.split_at(at);
                    if let (Ok(primary_c), Ok(tail_c)) = (CString::new(primary), CString::new(tail)) {
                        p.text(primary_c.as_ptr(), x, y, lsz, base, 0, lbold);
                        p.text(tail_c.as_ptr(), x + measure.width_str(primary, lsz, lbold != 0), y,
                            lsz, secondary, 0, lbold);
                    }
                } else if let Ok(text) = CString::new(text) {
                    p.text(text.as_ptr(), x, y, lsz, base, 0, lbold);
                }
            };
            // The FOCUSED row's label never elides: one that overflows its column scrolls through
            // it instead ([`crate::marquee::ROW`] — a rest, a glide, a loop), so a long server name
            // such as a subtitle release can be read in full. Every other row elides as before.
            let (msz, mbold) = if two_line { (tsz, tbold) } else { (lsz, lbold) };
            let full_w = if focused { measure.width_str(&row.label, msz, mbold != 0) } else { 0.0 };
            if focused && full_w > text_w {
                let m = crate::marquee::ROW;
                let t_ms = m.phase(ROW_MARQUEE.with(|c| c.read(&row.label)), full_w, text_w);
                m.report(t_ms, full_w, text_w);
                let off = m.x(t_ms, full_w, text_w);
                let y = if two_line {
                    title_y
                } else {
                    let (cap_top, baseline) = crate::typography::text_cap_band(msz, mbold);
                    cyc - (cap_top + baseline) * 0.5 // `Label`'s own Middle placement
                };
                {
                    // scissored to the label's column on the row's band; nests inside the table's
                    // own frame clip and restores it on drop
                    let _clip = crate::screen::ClipScope::open_in(p, Rect::new(label_x, sy, text_w, h));
                    // the shared placement rule: an Arabic or Hebrew label glides mirrored
                    let rtl = plx_gfx::text::is_rtl_paragraph(&row.label);
                    let (run, follower) = crate::marquee::placements(full_w, text_w, off, rtl);
                    paint_label(&row.label, label_x + run, y);
                    paint_label(&row.label, label_x + follower, y);
                }
            } else {
                if focused {
                    // a fitting focused label releases the clock: the next overflowing one starts
                    // from its rest beat rather than mid-glide
                    ROW_MARQUEE.with(|c| c.release());
                }
                let lbl = plx_gfx::text::elide_by(&row.label, text_w, false, |t| {
                    measure.width_str(t, lsz, lbold != 0)
                });
                if let Ok(cs) = CString::new(lbl.as_str()) {
                    if two_line {
                        p.text(cs.as_ptr(), label_x, title_y, tsz, base, 0, tbold);
                    } else if split.is_some() {
                        let (cap_top, baseline) = crate::typography::text_cap_band(lsz, lbold);
                        paint_label(&lbl, label_x, cyc - (cap_top + baseline) * 0.5);
                    } else {
                        let mut lab = Label::new(cs.as_ptr(), lsz, base);
                        if lbold == 1 {
                            lab = lab.bold();
                        }
                        lab.draw(p, Rect::new(label_x, sy, 0.0, h));
                    }
                }
            }
            // detail sub-line, elided (long Cyrillic descriptors would run off the edge) — to the
            // SAME right edge as the label above it, because the design system puts the pair in one
            // `flex:1` box. It used to elide against the bare row width, which was harmless only
            // while the badges sat on the title's line: right-aligned and row-centred, a chip now
            // occupies the sub-line's band too, and an unbounded sub-line would run under it.
            if !self.is_classic_menu() && !row.detail.is_empty() {
                let sub = on_pill(dimc, if classic { theme::with_a(ink, 0.6) } else { theme::scrim_black(0.6) });
                let marker_w = row.detail_marker_width().min(text_w.max(0.0));
                if let Some(color) = row.detail_marker.filter(|_| marker_w >= 8.0) {
                    let (top, bottom) = crate::typography::text_cap_band(theme::size::CAPTION, 0);
                    p.rect(Rect::new(label_x, detail_y + (top + bottom) * 0.5 - 4.0, 8.0, 8.0),
                        4.0, color, color, 0.0);
                }
                let detail = plx_gfx::text::elide_by(&row.detail, (text_w - marker_w).max(0.0), false, |t| {
                    measure.width_str(t, theme::size::CAPTION, false)
                });
                if let Ok(cd) = CString::new(detail) {
                    p.text(
                        cd.as_ptr(),
                        label_x + marker_w,
                        detail_y,
                        theme::size::CAPTION,
                        sub,
                        0,
                        0,
                    );
                }
            }
        });
    }

    /// The painter for one walk event: its section's ([`Self::group_painter`]), except the page
    /// title band and ITS hairline (the walk's first two events, the only divider tagged section 0
    /// since a section never divides from nothing above it), which belong to the page and so must
    /// never be dimmed by a dimmed first section.
    fn event_painter(&self, p: Painter, gi: i32, si: usize) -> Painter {
        if gi == WALK_TITLE || (gi == WALK_DIVIDER && si == 0) {
            p
        } else {
            self.group_painter(p, si)
        }
    }

    /// `p`, dimmed if section `si` is — the ONE place [`Section::dim`] becomes an alpha.
    fn group_painter(&self, p: Painter, si: usize) -> Painter {
        if self.sections.get(si).is_some_and(|s| s.dim) {
            p.alpha(GROUP_DIM_A)
        } else {
            p
        }
    }

    /// Which section global row `gi` belongs to (0 when it belongs to none — an empty table).
    fn section_of_row(&self, gi: i32) -> usize {
        let mut n = 0i32;
        for (si, sec) in self.sections.iter().enumerate() {
            n += sec.rows.len() as i32;
            if gi < n {
                return si;
            }
        }
        0
    }

    /// **One row, for an in-place edit** — a read-out that changes while the list does not (the
    /// Subtitles panel's Color value), so a press re-writes that row alone instead of rebuilding
    /// every section. `None` past the end. The caller must not change the row's height class.
    pub fn row_mut(&mut self, gi: i32) -> Option<&mut Row> {
        self.rev = next_rev();
        usize::try_from(gi).ok().and_then(|gi| row_mut_in(&mut self.sections, gi))
    }

    fn rows_at(&self, gi: i32) -> &Row {
        if let Some((rows, _)) = self.uniform_classic_rows() {
            if let Some(row) = usize::try_from(gi).ok().and_then(|i| rows.get(i)) {
                return row;
            }
        }
        let mut n = 0i32;
        for sec in &self.sections {
            for row in &sec.rows {
                #[cfg(any(test, feature = "test-support"))]
                self.row_visits.set(self.row_visits.get() + 1);
                if n == gi {
                    return row;
                }
                n += 1;
            }
        }
        // unreachable for a valid gi (draw early-returns when there are no rows)
        self.sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .next()
            .unwrap()
    }
}

/// The ink of a row's trailing read-out ([`Row::readout`]): one step behind the row's label, so it
/// follows the row's [`Row::dim`] as well as its own [`Row::value_dim`]. A dim row's label is
/// already [`theme::TEXT_TERTIARY`], so its read-out takes [`theme::ROW_VALUE_INK_DIM`] below
/// that; drawn at the live row's ink, a step that cannot be taken (the Timing section's Earlier at
/// its floor) still announced itself at the trailing edge. Over the focused pill a dim row's
/// read-out takes the quiet rung.
fn row_value_ink(row: &Row, focused: bool) -> [f32; 4] {
    match (focused, row.dim, row.value_dim) {
        (true, false, false) => crate::palette::color(crate::palette::Role::SelectionText, theme::ROW_VALUE_INK_ON),
        (true, _, _) => crate::palette::color(crate::palette::Role::SelectionText, theme::ROW_VALUE_INK_ON_DIM),
        (false, true, _) => crate::palette::color(crate::palette::Role::Muted, theme::ROW_VALUE_INK_DIM),
        (false, false, false) => crate::palette::color(crate::palette::Role::Muted, theme::TEXT_SECONDARY),
        (false, false, true) => crate::palette::color(crate::palette::Role::Muted, theme::TEXT_TERTIARY),
    }
}

#[cfg(test)]
mod tests {
    /// **A dimmed row's read-out dims with it.** The Timing section's Earlier row at the
    /// selected kind's floor is `dim`, and its "−0.1 s" read-out was drawn at the SAME ink as the
    /// live Later row's — so the step that could not be taken still announced itself at the
    /// trailing edge. The read-out stays one step behind its label: a dim label is
    /// [`theme::TEXT_TERTIARY`], so a dim row's value sits below that.
    #[test]
    fn a_dim_rows_readout_follows_the_row_dim() {
        let live = Row::new("Later").value("+0.1 s").value_dim(true);
        let dimmed = Row::new("Earlier").value("\u{2212}0.1 s").value_dim(true).dim(true);
        assert_ne!(
            row_value_ink(&dimmed, false),
            row_value_ink(&live, false),
            "a dim row's read-out must not keep the live row's ink",
        );
        assert!(
            row_value_ink(&dimmed, false)[3] < theme::TEXT_TERTIARY[3],
            "a dim row's read-out sits a step behind its dim label",
        );
        // an undimmed, unquietened read-out is unchanged
        let plain = Row::new("Audio").value("English");
        assert_eq!(row_value_ink(&plain, false), theme::TEXT_SECONDARY);
        assert_eq!(row_value_ink(&plain, true), theme::ROW_VALUE_INK_ON);
        // over the focused pill a dim row's read-out takes the quiet rung
        let dim_plain = Row::new("Audio").value("English").dim(true);
        assert_eq!(row_value_ink(&dim_plain, true), theme::ROW_VALUE_INK_ON_DIM);
    }

    #[test]
    fn table_motion_canonical_state_covers_hidden_spring_velocity_and_layout_flags() {
        fn hash(table: &super::TableView) -> u64 {
            let mut c = plx_machine::machine::Canon::new();
            table.write_motion(&mut c);
            c.finish()
        }
        let baseline = hash(&super::TableView::new());
        for field in 0..7 {
            let mut table = super::TableView::new();
            match field {
                0 => table.hl_top.vel = 1.0,
                1 => table.hl_bot.vel = 1.0,
                2 => table.scroll.vel = 1.0,
                3 => table.sel = 7,
                4 => table.list_focused = !table.list_focused,
                5 => table.compact = !table.compact,
                6 => table.tall = !table.tall,
                _ => unreachable!(),
            }
            assert_ne!(hash(&table), baseline, "field {field}");
            assert_eq!(hash(&table), hash(&table), "reading state must not advance animation");
        }
    }

    use super::*;

    #[test]
    fn classic_row_and_page_motion_settle_in_one_update_without_fractional_frames() {
        let mut table = TableView::new();
        let section = (0..41).fold(Section::new(""), |section, n| section.row(Row::new(format!("row {n}"))));
        table.set_sections(vec![section], 0, false);
        table.set_classic(true);
        table.set_classic_catalogue(true);
        let frame = Rect::new(0.0, 0.0, 800.0, 880.0);
        table.set_classic_page_size(Some(10), frame.h);
        table.fit_layout(&crate::fixture::FixtureMeasure);
        let rows = table.classic_page_rows().unwrap();
        for selected in [1, rows as i32 - 1, rows as i32, 40, 0] {
            table.sel = selected;
            table.update(1.0 / 30.0, frame.h);
            let first = selected.max(0) as usize / rows * rows;
            assert_eq!(table.scroll.pos, first as f32 * table.plain_row_h());
            assert_eq!(table.scroll.vel, 0.0);
            assert_eq!(table.hl_top.pos, table.row_top(selected) + table.pill_inset());
            assert_eq!(table.hl_bot.pos, table.row_top(selected) + table.row_height(selected) - table.pill_inset());
            assert_eq!((table.hl_top.vel, table.hl_bot.vel), (0.0, 0.0));
            let row = table.row_rect(frame, selected).unwrap();
            assert_eq!(table.hit_row(frame, row.cx(), row.cy()), Some(selected), "paint and hit placement must already use the final page");
            let before = (table.scroll.pos, table.hl_top.pos, table.hl_bot.pos);
            table.update(0.0, frame.h);
            assert_eq!(before, (table.scroll.pos, table.hl_top.pos, table.hl_bot.pos));
        }
    }

    #[test]
    fn classic_choices_snap_but_modern_rows_keep_their_spring_motion() {
        for classic in [false, true] {
            let mut table = TableView::new();
            let section = (0..20).fold(Section::new(""), |section, n| section.row(Row::new(format!("row {n}"))));
            table.set_sections(vec![section], 0, false);
            table.set_classic(classic);
            table.set_classic_choices(classic);
            table.fit_layout(&crate::fixture::FixtureMeasure);
            table.sel = 10;
            table.update(1.0 / 60.0, 300.0);
            let wanted = table.row_top(10) + table.pill_inset();
            if classic {
                assert_eq!(table.hl_top.pos, wanted);
                assert_eq!(table.scroll.vel, 0.0);
                let before = table.scroll.pos;
                table.update(1.0 / 60.0, 300.0);
                assert_eq!(table.scroll.pos, before, "a Classic menu must not finish a second half-step");
            } else {
                assert!(table.hl_top.pos < wanted);
                assert!(table.hl_top.vel > 0.0, "Modern keeps its original animated treatment");
            }
        }
    }

    #[test]
    fn large_table_actual_paint_visits_each_row_once_and_culls_offscreen_ink() {
        let _serial = plx_base::testlock::serial();
        const N: i32 = 10_049;
        let mut section = Section::new("");
        for i in 0..N { section = section.row(Row::new(format!("row {i}"))); }
        let mut table = TableView::new().unfocused();
        table.set_sections(vec![section], 0, false);
        let frame = Rect::new(0.0, 0.0, 800.0, 600.0);
        for start in [0, N / 2, N - 1] {
            table.scroll.pos = table.row_top(start);
            table.take_row_visits();
            let text = crate::draw_census::capture_text(|| table.draw_page(
                crate::Painter::recording(), frame, &crate::fixture::FixtureMeasure));
            // One walk supplies the row reference. Restoring rows_at(gi) in draw
            // adds N*(N+1)/2 visits and fails this actual paint-path assertion.
            assert_eq!(table.take_row_visits(), N as usize);
            assert!(text.iter().any(|(text, _)| text == &format!("row {start}")));
            let visible_bound = (crate::consts::SCR_H / ROW_H).ceil() as usize + 2;
            assert!(text.len() <= visible_bound, "offscreen rows must not be painted");
            for (label, rect) in text {
                let i: i32 = label.strip_prefix("row ").unwrap().parse().unwrap();
                assert!(i >= start);
                assert!(rect.y < crate::consts::SCR_H + ROW_H);
            }
        }
    }

    #[test]
    fn panel_width_reserves_a_notes_longest_unbroken_word_at_large_text() {
        use plx_machine::machine::{FontScale, Measure, TypographyMeasure};
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(200).unwrap());
        let mut plain = TableView::new();
        plain.set_sections(vec![Section::new("").row(Row::new("Item"))], 0, false);
        assert_eq!(plain.measured_width(&measure), 247.0, "the no-note layout stays unchanged");
        for glue in ['\u{a0}', '\u{202f}'] {
            let word = format!("indivisible{glue}word");
            let note = format!("Short {word} followed by several short words that should wrap");
            let mut table = TableView::new();
            table.set_sections(vec![Section::new("").row(Row::note(&note))], 0, false);
            let width = table.measured_width(&measure);
            let natural = measure.width_str(&word, theme::size::CAPTION, false);
            assert!(width >= 2.0 * CONTENT_X + LABEL_INSET + natural / fit::HEADROOM);
            assert!(width < measure.width_str(&note, theme::size::CAPTION, false),
                "a wrapped paragraph must not demand a single-line panel");
            assert!(table.fit_report(width, &measure, fit::HEADROOM).is_empty());
        }
    }

    #[test]
    fn final_control_reveals_its_non_focusable_note_without_losing_its_hit_target() {
        let mut table = TableView::new();
        table.set_sections(vec![Section::new("")
            .row(Row::new("First")).row(Row::new("Middle")).row(Row::new("Last"))
            .row(Row::note("An explanation on four lines"))], 2, false);
        let note = &table.sections[0].rows[3];
        note.note_lines.set(4);
        let frame = Rect::new(50.0, 80.0, 440.0, 300.0);
        for _ in 0..180 { table.update(1.0 / 60.0, frame.h); }
        let control = table.row_frame(frame, 2).unwrap();
        assert!(control.y >= frame.y + TOP_PAD - 0.1);
        let note_top = frame.y + TOP_PAD + table.row_top(3) - table.scroll_pos();
        let note_end = note_top + table.row_height(3);
        assert!(note_top >= control.y + control.h - 0.1);
        assert!(note_end <= frame.y + frame.h - BOT_PAD + 0.1);
        assert_eq!(table.hit_row(frame, control.x + control.w / 2.0, control.y + control.h / 2.0), Some(2));
    }

    #[test]
    fn large_table_bulk_frames_keep_visible_work_bounded_and_match_hit_geometry() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        const N: i32 = 10_049;
        let mut section = Section::new("Catalog");
        for i in 0..N {
            section = section.row(if i % 7 == 0 { Row::new("Video").detail("File") } else { Row::new("Video") });
        }
        let mut table = TableView::new();
        table.set_title(Some("Local videos".into()));
        table.set_sections(vec![section, Section::new("Actions")
            .row(Row::separator()).row(Row::note("An explanatory note that spans multiple lines"))
            .row(Row::new("Save")).row(Row::new("Cancel"))], 0, false);
        let frame = Rect::new(50.0, 80.0, 700.0, 380.0);
        for percent in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(percent).unwrap());
            table.fit_notes(frame.w, &measure);
            for target in [0, N / 2, N + 3] {
                // Start, middle and last action; no clock or animated settling is needed.
                table.scroll.pos = table.row_top(target);
                let mut traversed = 0;
                table.for_each_row_frame(frame, |_, _| traversed += 1);
                assert_eq!(traversed, N + 2, "each real row is emitted once, never separators/notes");
                let mut visible = Vec::new();
                table.for_each_visible_row(frame, |i, r| visible.push((i, r)));
                assert!(!visible.is_empty());
                assert!(visible.len() <= 8, "hit registration must depend on the viewport, not 10,049 entries");
                for (i, r) in visible {
                    let one = table.row_frame(frame, i).unwrap();
                    assert_eq!([r.x, r.y, r.w, r.h], [one.x, one.y, one.w, one.h]);
                    let top = r.y.max(frame.y);
                    let bottom = (r.y + r.h).min(frame.y + frame.h);
                    assert_eq!(table.hit_row(frame, r.x + r.w * 0.5, (top + bottom) * 0.5), Some(i));
                }
            }
        }
    }

    #[test]
    fn font_scale_keeps_rows_headers_and_hit_regions_in_one_layout() {
        use plx_machine::machine::{FontScale, Measure, TypographyMeasure};
        for percent in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(percent).expect("supported UI scale"));
            let mut table = TableView::new();
            table.set_title(Some("Profile setup".into()));
            table.set_sections(vec![Section::new("Connection")
                .row(Row::new("Profile name").detail("Set"))
                .row(Row::new("Source address").detail("Not set"))
                .row(Row::new("Save"))
                .row(Row::new("Cancel"))], 0, false);
            table.fit_notes(900.0, &measure);
            let factor = (f32::from(percent) / 100.0).max(1.0);
            assert_eq!(table.plain_row_h(), ROW_H * factor);
            assert_eq!(table.tall_row_h(), ROW_H_TALL * factor);
            assert_eq!(table.header_h(), HDR_H * factor);
            let frame = Rect::new(0.0, 0.0, 900.0, 1000.0);
            let mut prior_bottom = table.title_rect(frame).unwrap().y + table.header_h();
            for i in 0..table.n_rows() {
                let row = table.row_frame(frame, i).unwrap();
                assert!(row.y >= prior_bottom);
                assert_eq!(table.hit_row(frame, row.x + row.w * 0.5, row.y + row.h * 0.5), Some(i));
                let label = measure.cap_h(theme::size::HEADLINE);
                let ink = if i < 2 { label + ROW_SUB_GAP + measure.cap_h(theme::size::CAPTION) } else { label };
                assert!(ink + 2.0 * ROW_INK_PAD <= row.h, "scale {percent}, row {i}");
                prior_bottom = row.y + row.h;
            }
            assert_eq!(table.next_selectable(2, 1), Some(3));
            assert_eq!(table.next_selectable(3, -1), Some(2));
            assert!(measure.cap_h(theme::size::CAPTION) + HEADER_CAP_INSET + HDR_INK_PAD <= table.header_h());
        }
    }

    #[test]
    fn font_scale_scroll_keeps_selected_fields_save_and_cancel_visible() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(200).expect("supported UI scale"));
        let mut table = TableView::new();
        table.set_title(Some("Profile setup".into()));
        table.set_sections(vec![Section::new("Connection")
            .row(Row::new("Profile name").detail("Set"))
            .row(Row::new("Source address").detail("Not set"))
            .row(Row::new("Save"))
            .row(Row::new("Cancel"))], 0, false);
        table.fit_notes(800.0, &measure);
        let frame = Rect::new(0.0, 0.0, 800.0, 280.0);
        for i in [1, 2, 3, 2, 0] {
            table.sel = i;
            for _ in 0..180 { table.update(1.0 / 60.0, frame.h); }
            let row = table.row_frame(frame, i).unwrap();
            assert!(row.y >= frame.y + TOP_PAD - 0.1, "row {i} top {}", row.y);
            assert!(row.y + row.h <= frame.y + frame.h - BOT_PAD + 0.1,
                "row {i} bottom {}", row.y + row.h);
            assert_eq!(table.hit_row(frame, row.x + row.w * 0.5, row.y + row.h * 0.5), Some(i));
        }
    }

    #[test]
    fn font_scale_invalidates_height_cache_and_can_return_to_default() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        let mut table = TableView::new();
        table.set_sections(vec![Section::new("Connection").row(Row::new("Save"))], 0, false);
        table.fit_notes(800.0, &measure);
        let height = table.measured_height();
        let revision = table.layout_rev();
        measure.set_font_scale(FontScale::from_percent(200).expect("supported UI scale"));
        table.fit_notes(800.0, &measure);
        assert!(table.measured_height() > height);
        assert_ne!(table.layout_rev(), revision);
        let revision = table.layout_rev();
        table.fit_notes(800.0, &measure);
        assert_eq!(table.layout_rev(), revision);
        measure.set_font_scale(FontScale::DEFAULT);
        table.fit_notes(800.0, &measure);
        assert_eq!(table.measured_height(), height);
        table.tall_rows(true);
        assert_eq!(table.tall_row_h(), ROW_H_ART);
    }

    /// ONE trailing read-out per row, resolved in ONE place. The rule the design system states and
    /// this widget enforces is that **a mark says where you are and a word says what is set, and no
    /// row is allowed to say both** — so a switch's state is the WORD `On`/`Off` at the trailing
    /// edge (there is no ring/ticked-ring pair any more, assets and all), a leading tick is not a
    /// read-out at all, and a row carrying both a `value` and a `toggle` draws exactly one of them.
    ///
    /// Worth pinning even though it is four lines of `match`: `library.rs`'s Sources test spells the
    /// `On`/`Off` mapping out a second time to grade the row MODEL, and this is the assertion that
    /// the copy it grades is the copy the widget actually draws.
    #[test]
    fn a_row_states_exactly_one_trailing_read_out() {
        assert_eq!(
            Row::new("Unwatched only").toggle(true).readout(),
            Some("On")
        );
        assert_eq!(
            Row::new("Unwatched only").toggle(false).readout(),
            Some("Off")
        );
        assert_eq!(Row::new("Genre").value("All").readout(), Some("All"));
        // both set: the explicit word wins and the switch's is NOT drawn beside it
        assert_eq!(
            Row::new("Genre").value("Comedy").toggle(true).readout(),
            Some("Comedy")
        );
        // a leading mark is a different column with a different job — it states no value
        assert_eq!(Row::new("English").checked(true).readout(), None);
        assert_eq!(Row::new("Chapters").readout(), None);
    }

    /// The shared table promises a travelling focus pill. A caller that changes `sel` but forgets
    /// to call `update` gets exactly the reported failure: new-row ink with the old pill, followed
    /// by a later jump. Pin both halves of the motion contract here — moving and genuinely resting.
    /// **The last SELECTABLE row is not the last index.** `route_screen`'s rule 2 (DOWN off the
    /// last row enters the action band) is graded on this, so on a list that ends in a grouping
    /// hairline — which `move_sel` can never land on — the band would be unreachable if the
    /// predicate were `sel == n_rows() - 1`.
    #[test]
    fn the_last_selectable_row_is_never_a_grouping_hairline() {
        let mut t = TableView::new();
        t.set_sections(
            vec![Section::new("S")
                .row(Row::new("a"))
                .row(Row::new("b"))
                .row(Row::separator())],
            0,
            false,
        );
        assert_eq!(t.n_rows(), 3);
        assert_eq!(t.last_row(), Some(1), "the hairline is not a landable row");
        t.sel = 1;
        assert!(t.at_last_row());
        t.sel = 0;
        assert!(!t.at_last_row());

        let empty = TableView::new();
        assert_eq!(empty.last_row(), None);
        assert!(!empty.at_last_row());
    }

    /// **A menu never opens with its focus on a destructive action.** The opening row steps over
    /// destructive rows and separators; when every row is destructive it is the first anyway.
    #[test]
    fn a_menu_opens_on_its_first_non_destructive_row() {
        let mut t = TableView::new();
        t.open_sections(vec![Section::new("S")
            .row(Row::new("Sign out").destructive(true))
            .row(Row::separator())
            .row(Row::new("Settings"))]);
        assert_eq!(t.sel, 2);
        assert_eq!(t.opening_row(), 2);

        t.open_sections(vec![Section::new("S")
            .row(Row::separator())
            .row(Row::new("Remove").destructive(true))]);
        assert_eq!(t.sel, 1, "all destructive: the first selectable row anyway");

        t.open_sections(vec![Section::new("S").row(Row::new("a")).row(Row::new("b"))]);
        assert_eq!(t.sel, 0, "an ordinary menu still opens on its first row");

        t.open_sections(Vec::new());
        assert_eq!(t.sel, 0);
    }

    /// **A row OPENS something exactly when it wears the drill-in chevron.** `route_screen`'s
    /// rule 8 is read off the painted affordance rather than kept as a second per-screen list.
    #[test]
    fn a_row_opens_something_exactly_when_it_wears_the_drill_in_chevron() {
        let mut t = TableView::new();
        t.set_sections(
            vec![Section::new("S")
                .row(Row::new("a door").chevron(true))
                .row(Row::new("a switch").toggle(true))
                .row(Row::new("a plain row"))],
            0,
            false,
        );
        assert!(t.row_opens(0));
        assert!(!t.row_opens(1), "a switch changes a value in place");
        assert!(!t.row_opens(2));
        assert!(!t.row_opens(-1), "and an out-of-range ask answers no rather than panicking");
        assert!(!t.row_opens(99));
    }

    #[test]
    fn the_focus_pill_runs_between_rows_and_goes_quiet_at_rest() {
        let _serial = plx_base::testlock::serial();
        let mut t = TableView::new();
        t.set_sections(
            vec![Section::new("").row(Row::new("One")).row(Row::new("Two"))],
            0,
            false,
        );
        let start = t.highlight_motion();
        t.move_sel(1);
        t.update(1.0 / 60.0, t.measured_height());
        let running = t.highlight_motion();
        assert!(
            running.0 > start.0,
            "the pill did not leave its old row: {running:?}"
        );
        assert!(
            running.1.abs() > 0.0,
            "a travelling edge must carry velocity: {running:?}"
        );

        for _ in 0..240 {
            t.update(1.0 / 60.0, t.measured_height());
        }
        let resting = t.highlight_motion();
        let target = t.row_top(1) + PILL_INSET;
        assert!(
            (resting.0 - target).abs() < 0.01,
            "pill stopped away from row 1: {resting:?}"
        );
        assert!(
            resting.1.abs() < 0.01,
            "settled pill still reports motion: {resting:?}"
        );
    }

    /// **Regression for the Settings ▸ Privacy & data report**: "Delete all local data" (the last
    /// row) never scrolled fully into view. `update`'s `frame_h` is the SAME height passed to
    /// `draw`/`hit_row` — a caller that (wrongly) subtracted [`PAD_V`] before calling `update`, or
    /// an `update` that (wrongly) failed to subtract it internally, makes the scroll clamp believe
    /// the viewport is `PAD_V` taller than the clipped frame it is actually drawn into, so it stops
    /// short of the true bottom by exactly that amount — the last row settles PARTIALLY behind the
    /// clip. This overflows a small frame on purpose and settles on the last row.
    #[test]
    fn scrolling_to_the_last_row_reveals_it_fully_above_the_bottom_pad() {
        let _serial = plx_base::testlock::serial();
        let mut t = TableView::new();
        let mut sec = Section::new("S");
        for i in 0..20 {
            sec = sec.row(Row::new(format!("row {i}")));
        }
        t.set_sections(vec![sec], 0, false);
        let content_h = t.measured_height() - PAD_V;
        let frame_h = 300.0; // far shorter than the 20-row content, so this frame must scroll
        assert!(content_h > frame_h, "test needs overflowing content");

        t.move_sel(t.n_rows() - 1);
        for _ in 0..300 {
            t.update(1.0 / 60.0, frame_h);
        }

        // The frame passed to `draw` reserves TOP_PAD above row 0 and BOT_PAD below the last row —
        // so at rest the last row's bottom (`content_h`, in content coordinates) must land exactly
        // `BOT_PAD` above the clipped frame's bottom edge, not merely somewhere inside it.
        let last_row_bottom_on_screen = TOP_PAD + content_h - t.scroll_pos();
        let want = frame_h - BOT_PAD;
        assert!(
            (last_row_bottom_on_screen - want).abs() < 0.5,
            "last row settled at {last_row_bottom_on_screen}, wanted {want} \
             (frame_h={frame_h}, content_h={content_h}, scroll={})",
            t.scroll_pos()
        );
    }

    /// Owner reports: the Settings table (issue 10) and the player's Quality popover (its last
    /// rung) ended in a row cut mid-glyph by the viewport's bottom edge. A row crossing that edge
    /// must be transparent before the edge reaches its ink, and a row resting with the list's own
    /// bottom air below it must be whole. Swept over every scroll a DOWN walk passes through, in
    /// both a Settings-sized frame and a popover-sized one, with plain and two-line rows.
    #[test]
    fn no_row_is_drawn_cut_mid_glyph_at_the_bottom_edge() {
        let _serial = plx_base::testlock::serial();
        for (frame_h, detail_every) in [(300.0f32, 2usize), (520.0, 3), (455.0, 0)] {
            let mut t = TableView::new();
            let mut sec = Section::new("S");
            for i in 0..18 {
                let mut row = Row::new(format!("row {i}")).value("value");
                if detail_every > 0 && i % detail_every == 0 { row = row.detail("detail"); }
                sec = sec.row(row);
            }
            t.set_sections(vec![sec, Section::new("T").row(Row::new("tail"))], 0, false);
            let frame = Rect::new(0.0, 100.0, 700.0, frame_h);
            let bottom = frame.y + frame.h;
            let mut crossed = 0;
            for step in 0..t.n_rows() {
                if step > 0 { t.move_sel(1); }
                for _ in 0..12 { t.update(1.0 / 60.0, frame_h); }
                for i in 0..t.n_rows() {
                    let Some(r) = t.row_frame(frame, i) else { continue };
                    let a = t.bottom_edge_alpha(r.y, r.h, bottom);
                    if r.y < bottom && r.y + r.h > bottom {
                        crossed += 1;
                        assert!(a == 0.0 || r.y + r.h - ROW_INK_PAD <= bottom,
                            "row {i} at {}..{} is drawn at {a} while the edge {bottom} cuts its ink", r.y, r.y + r.h);
                    }
                    if r.y + r.h + BOT_PAD <= bottom {
                        assert_eq!(a, 1.0, "row {i} resting above the bottom air is whole");
                    }
                }
            }
            assert!(crossed > 0, "the premise: rows do cross the bottom edge at {frame_h}");
        }
    }

    /// A long (uppercased) header and its right-aligned accessory never claim overlapping pixels,
    /// for any header/accessory pair.
    #[test]
    fn a_long_header_never_overlaps_its_accessory() {
        use crate::fixture::FixtureMeasure;
        let frame_w = 700.0;
        let long_header = "a machine name so long it would eat the whole header band by itself";
        let cases = [
            (long_header, "owner@example.com"),
            (long_header, "x"),
            ("short", "a handle also long enough to matter here"),
            ("S", ""),
        ];
        for (header, accessory) in cases {
            let sec = Section::new(header).accessory(accessory);
            let (header_w, accessory_w) = TableView::header_columns(&sec, frame_w, &FixtureMeasure, 1.0);
            let span = frame_w - 2.0 * CONTENT_X;
            assert!(header_w >= 0.0 && accessory_w >= 0.0, "{header:?}/{accessory:?}: negative column");
            if accessory.is_empty() {
                assert_eq!(accessory_w, 0.0);
                continue;
            }
            assert!(
                header_w + ACCESSORY_GAP + accessory_w <= span + 0.01,
                "{header:?}/{accessory:?}: header_w={header_w} + gap + accessory_w={accessory_w} overruns the {span}px band — they would overlap",
            );
        }
    }

    /// `row_columns`' fallback for a value nothing shortened: the value is elided to what is left
    /// beside its label, but never to nothing.
    #[test]
    fn row_columns_still_elides_an_unshortened_long_value() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        use plx_machine::machine::Measure;
        use crate::route_screen::RouteLayout;
        let frame_w = RouteLayout::screen().sectioned_table().w;
        // Too long to fit beside its label.
        const LONG_VALUE: &str = "This value is deliberately far too long to sit beside its label";
        let mut table = TableView::new();
        table.compact = false;
        let rows = [
            Row::new("Quality").value("Not set").chevron(true),
            Row::new("Direct Play").value(LONG_VALUE).chevron(true),
        ];
        table.set_sections(vec![rows.into_iter().fold(Section::new(""), Section::row)], 0, false);
        let direct_play_row = &table.sections[0].rows[1];
        let cols = table.row_columns(direct_play_row, frame_w, &M);
        let natural = M.width_str(LONG_VALUE, theme::size::LABEL, true);
        assert!(cols.value_w < natural, "the premise: the long value does not fit beside its label");
        assert!(cols.value_w > 0.0, "a value keeps a visible slot");
    }

    /// `fit_report` does not flag a short value that fits, and still flags one that does not.
    #[test]
    fn fit_report_does_not_flag_a_short_value_that_fits() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        use crate::fit::HEADROOM;
        let frame_w = 700.0;
        let mut table = TableView::new();
        table.compact = false;
        let rows = [
            Row::new("Unwatched only").value("Off"),
            Row::new("Direct Play").value(
                "This value is deliberately far too long to sit beside its label in a 700px frame",
            ),
        ];
        table.set_sections(vec![rows.into_iter().fold(Section::new(""), Section::row)], 0, false);
        let issues = table.fit_report(frame_w, &M, HEADROOM);
        let short_flagged = issues.iter().any(|i| i.role == FitRole::Value && i.text == "Off");
        assert!(!short_flagged, "a short value that fits must not be reported: {issues:?}");
        let long_flagged = issues.iter().any(|i| i.role == FitRole::Value && i.text.starts_with("This value"));
        assert!(long_flagged, "a genuinely too-long value must still be reported: {issues:?}");
    }

    /// **Every row's LAST trailing element ends on one right edge** (the row content's right edge).
    /// On a row with a trailing icon that is the icon; on a row without one it is the read-out
    /// itself, flush — no empty column is held open. And a hugged panel must show every run whole.
    #[test]
    fn a_sections_values_share_a_right_edge_and_the_hugged_panel_elides_nothing() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        use plx_machine::machine::Measure;
        let mut table = TableView::new();
        table.compact = false;
        let section = Section::new("")
            .row(Row::new("Unwatched only").value("Off"))
            .row(Row::new("Genre").value("All").chevron(true))
            .row(Row::new("Lone").chevron(true));
        table.set_sections(vec![section], 0, false);
        let content_right = 800.0 - SIDE - CONTENT_PAD;
        let (off, genre, lone) = (&table.sections[0].rows[0], &table.sections[0].rows[1], &table.sections[0].rows[2]);
        // the chevron's INK ends on the content edge (its box overhangs by the right-side bearing)
        let ink_right = |row: &Row| {
            let icon = row.ticon.unwrap();
            content_right + ticon_box_dx(icon) + crate::icons::ink_x(icon).1 * ACCESSORY_ICON_W
        };
        let last_edge = |row: &Row| if row.ticon.is_some() { ink_right(row) } else { table.value_right_edge(row, 800.0) };
        let near = |a: f32, b: f32| (a - b).abs() <= 0.5;
        assert!(
            near(table.value_right_edge(off, 800.0), ink_right(genre)),
            "'Off' ends at {} but the chevron ink at {}",
            table.value_right_edge(off, 800.0),
            ink_right(genre)
        );
        assert!(near(ink_right(genre), content_right), "chevron ink {} is not on the content edge {content_right}", ink_right(genre));
        assert!(near(last_edge(off), last_edge(lone)), "a value row and a lone-chevron row end on different edges");
        assert!(
            near(table.value_right_edge(genre, 800.0), content_right - ticon_slot_w(genre.ticon.unwrap())),
            "a value beside a chevron stops one ink-width + gap short of the edge"
        );
        let w = table.menu_panel_width(&M);
        let failures = table.app_fit_failures_hugged("filter");
        assert!(failures.is_empty(), "elision at the hugged width {w}: {failures:?}");
        let (size, bold) = table.label_style();
        for (row, icon) in [(off, 0.0), (genre, ticon_slot_w(crate::icons::Icon::Chevron))] {
            let span = w - 2.0 * CONTENT_X - CHECK_W - GAP - icon;
            let value = row.readout().unwrap();
            let gap = span - M.width_str(&row.label, size, bold) - M.width_str(value, theme::size::LABEL, VALUE_BOLD != 0);
            assert!(gap >= theme::space::MD - 0.01, "{}: label to value air {gap} < the MD rung", row.label);
        }
    }

    const ES_NOTE: &str = "No se puede cambiar mientras Realzar diálogos o Normalizar volumen esté activado.";

    fn note_table(note: &str) -> TableView {
        let mut t = TableView::new();
        t.set_sections(
            vec![Section::new("S").row(Row::new("Color").value("Blanco")).row(Row::note(note))],
            0,
            false,
        );
        t
    }

    /// **A note wraps inside the row-label column and its row grows to the wrapped lines** — the
    /// Spanish Subtitles note ran off the panel as one clipped line at a fixed `ROW_H`.
    #[test]
    fn a_long_note_wraps_and_its_row_grows_with_the_lines() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        let t = note_table(ES_NOTE);
        let before = t.measured_height();
        t.fit_notes(620.0, &M);
        let note = &t.sections[0].rows[1];
        assert!(note.note_lines.get() >= 2, "the premise: it does not fit one line of the column");
        let h = note.height_in(t.tall_row_h(), t.plain_row_h());
        assert!(h > ROW_H, "note height {h} must grow past ROW_H");
        assert!((t.measured_height() - before - (h - ROW_H)).abs() < 0.01, "layout routes through the same height");
        let short = note_table("Nope.");
        short.fit_notes(620.0, &M);
        assert_eq!(short.sections[0].rows[1].height_in(short.tall_row_h(), short.plain_row_h()), ROW_H, "one line keeps the ROW_H measure");
    }

    /// **`fit_report` judges notes**: a wrapped note fits, one unbreakable word wider than the
    /// column does not.
    #[test]
    fn fit_report_covers_note_rows() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        use crate::fit::HEADROOM;
        let ok = note_table(ES_NOTE).fit_report(620.0, &M, HEADROOM);
        assert!(ok.iter().all(|i| i.role != FitRole::Note), "a wrappable note fits: {ok:?}");
        let word = "Superextraordinariamente".repeat(4);
        let bad = note_table(&word).fit_report(620.0, &M, HEADROOM);
        assert!(bad.iter().any(|i| i.role == FitRole::Note), "an unbreakable overlong word must be reported: {bad:?}");
    }

    /// **Every section after the first is divided from the one above, headed or not.**
    #[test]
    fn a_headerless_section_after_the_first_gets_a_divider() {
        let mut t = TableView::new();
        t.set_sections(
            vec![
                Section::new("A").row(Row::new("a")),
                Section::new("").row(Row::new("b")),
                Section::new("C").row(Row::new("c")),
            ],
            0,
            false,
        );
        let (mut dividers, mut headers) = (0, 0);
        t.walk(|_, gi, _, _| match gi {
            WALK_DIVIDER => dividers += 1,
            -1 => headers += 1,
            _ => {}
        });
        assert_eq!(headers, 2);
        assert_eq!(dividers, 2, "one hairline per section boundary, headed or not");
    }

    /// **A header sits closer to its rows than to the divider above it**: the dead band under the
    /// caps is shorter than the band above them (half the divider gap plus the cap inset).
    #[test]
    fn a_header_reads_as_belonging_to_its_rows() {
        let above = DIV_H * 0.5 + HEADER_CAP_INSET;
        assert!(HDR_INK_PAD < above, "space under the caps ({HDR_INK_PAD}) must be under the space above ({above})");
        assert!(HDR_INK_PAD >= PILL_INSET * 2.0, "the focused pill of the first row must clear the caps");
    }

    /// **A title band adds the header band plus a section-boundary gap** to the content, and is
    /// part of neither the rows nor the selection.
    #[test]
    fn a_title_adds_a_header_band_and_a_divider_gap() {
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("").row(Row::new("a")).row(Row::new("b"))], 0, false);
        let bare = t.measured_height();
        t.set_title(Some("Style".into()));
        assert_eq!(t.measured_height(), bare + HDR_H + TITLE_GAP + DIV_H);
        let (mut titles, mut dividers, mut first_row_y) = (0, 0, None);
        t.walk(|y, gi, _, _| match gi {
            WALK_TITLE => titles += 1,
            WALK_DIVIDER => dividers += 1,
            0 => first_row_y = Some(y),
            _ => {}
        });
        assert_eq!((titles, dividers), (1, 1), "the title is followed by the hairline a section boundary has");
        assert_eq!(first_row_y, Some(HDR_H + TITLE_GAP + DIV_H));
        t.set_title(None);
        assert_eq!(t.measured_height(), bare);
    }

    /// **A trailing empty headerless section is nothing**: no divider, no height, no over-scroll
    /// (a `Form` section whose items all fell through stays visible but empty).
    #[test]
    fn a_trailing_empty_section_adds_nothing() {
        let rows = || vec![Section::new("A").row(Row::new("a")), Section::new("B").row(Row::new("b"))];
        let count = |t: &TableView| {
            let mut d = 0;
            t.walk(|_, gi, _, _| d += (gi == WALK_DIVIDER) as i32);
            d
        };
        let mut bare = TableView::new();
        bare.set_sections(rows(), 0, false);
        let mut t = TableView::new();
        let mut with = rows();
        with.push(Section::new(""));
        t.set_sections(with, 0, false);
        assert_eq!(t.measured_height(), bare.measured_height());
        assert_eq!(count(&t), count(&bare));
        assert_eq!(t.content_h(), bare.content_h());
    }

    /// **A title installed AFTER the sections still finds the pill on its row**: the band moves
    /// every row, so the springs re-snap instead of gliding from the old place.
    #[test]
    fn set_title_after_the_sections_resnaps_the_pill() {
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("").row(Row::new("a")).row(Row::new("b"))], 1, false);
        t.set_title(Some("Style".into()));
        let top = t.row_top(1);
        assert_eq!(t.hl_top.pos, top + PILL_INSET);
        assert_eq!(t.hl_bot.pos, top + t.row_height(1) - PILL_INSET);
        t.set_title(None);
        assert_eq!(t.hl_top.pos, t.row_top(1) + PILL_INSET);
    }

    /// **The title band has a rect that follows the scroll, and no section dims it.**
    #[test]
    fn title_rect_follows_scroll_and_the_title_is_never_dimmed() {
        let frame = Rect::new(10.0, 20.0, 400.0, 300.0);
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("").row(Row::new("a")).dim(true)], 0, false);
        assert_eq!(t.title_rect(frame), None);
        t.set_title(Some("Style".into()));
        let r = t.title_rect(frame).expect("band");
        assert_eq!((r.x, r.y, r.h), (frame.x + SIDE, frame.y + TOP_PAD, HDR_H));
        t.scroll.jump(10.0);
        assert_eq!(t.title_rect(frame).map(|r| r.y), Some(frame.y + TOP_PAD - 10.0));
        t.scroll.jump(HDR_H + 50.0);
        assert_eq!(t.title_rect(frame), None, "scrolled out of the viewport");
        // the dim is the section's alone: title and its hairline keep the page's painter
        let (mut page, mut rows) = (Vec::new(), Vec::new());
        t.walk(|_, gi, si, _| {
            let o = t.event_painter(Painter::root(), gi, si).opacity();
            if gi == WALK_TITLE || gi == WALK_DIVIDER { page.push(o) } else { rows.push(o) }
        });
        assert_eq!(page, [1.0, 1.0], "title and its hairline");
        assert_eq!(rows, [GROUP_DIM_A], "the dimmed section's row");
    }

    /// **The fit gate reports a title that cannot fit**, at the width the localized text needs.
    #[test]
    fn fit_report_covers_the_title_band() {
        use plx_base::fontcov::advances::ShippedMeasure as M;
        use crate::fit::HEADROOM;
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("").row(Row::new("a"))], 0, false);
        t.set_title(Some("Idioma de los subtitulos disponibles en otros idiomas".into()));
        let narrow = t.fit_report(300.0, &M, HEADROOM);
        assert!(narrow.iter().any(|i| i.role == FitRole::Title), "a long title must be reported: {narrow:?}");
        let wide = t.fit_report(t.menu_panel_width(&M).max(t.measured_width(&M)), &M, HEADROOM);
        assert!(wide.iter().all(|i| i.role != FitRole::Title), "a panel sized from measured_width fits its title: {wide:?}");
    }

    // ---- the focused row's marquee -----------------------------------------------------------

    /// The x of every text run the recording painter is handed inside `row`'s band.
    fn row_text_xs(t: &TableView, frame: Rect, row: i32) -> Vec<f32> {
        let band = t.row_frame(frame, row).expect("row on screen");
        let log = crate::draw_census::capture(|| {
            t.draw(crate::Painter::recording(), frame, &crate::fixture::FixtureMeasure)
        });
        log.into_iter()
            .filter(|(tag, r)| *tag == 100 && r.y >= band.y - 1.0 && r.y < band.y + band.h)
            .map(|(_, r)| r.x)
            .collect()
    }

    fn marquee_table(sel: i32) -> TableView {
        let mut t = TableView::new();
        let long = "A.Very.Long.Subtitle.Release.Name.2026.1080p.WEB-DL.DDP5.1.H.264-GROUP";
        let sec = Section::new("S").row(Row::new(long).server_label()).row(Row::new("Off"));
        t.set_sections(vec![sec], sel, false);
        t
    }

    /// **A focused label that overflows scrolls, after a rest** — a downloaded subtitle's release
    /// name is several times the track menu's width, and eliding it hid the part that tells two
    /// releases apart. It rests (left edge in place, a frame bought but no damage claimed), then
    /// glides (damage reported every frame it moves), drawn as a run and its follower.
    #[test]
    fn a_focused_overflowing_label_rests_then_scrolls_and_reports_while_it_moves() {
        let _serial = plx_base::testlock::serial();
        let frame = Rect::new(0.0, 0.0, 500.0, 400.0);
        let t = marquee_table(0);
        ROW_MARQUEE.with(|c| c.release());
        plx_machine::idle::frame_begin(1.0 / 60.0);
        let _ = plx_machine::idle::take_local_damage();
        let rest = row_text_xs(&t, frame, 0);
        assert_eq!(rest.len(), 2, "a run and its follower: {rest:?}");
        assert_eq!(plx_machine::idle::take_local_damage(), 0, "resting is not damage");
        let x0 = rest.iter().copied().fold(f32::MAX, f32::min);

        plx_machine::idle::frame_begin((crate::marquee::ROW.hold_ms + 500.0) / 1000.0);
        let gliding = row_text_xs(&t, frame, 0);
        let x1 = gliding.iter().copied().fold(f32::MAX, f32::min);
        assert!(x1 < x0, "the run has moved left: {x0} -> {x1}");
        assert!(plx_machine::idle::take_local_damage() > 0, "a gliding frame reports");
    }

    /// **An Arabic release name scrolls mirrored**: it rests with its right edge (its first
    /// words) on the column's, its follower one gap to the left, and glides right.
    #[test]
    fn a_focused_right_to_left_label_rests_on_its_first_words_and_scrolls_right() {
        let _serial = plx_base::testlock::serial();
        use plx_machine::machine::Measure as _;
        let frame = Rect::new(0.0, 0.0, 500.0, 400.0);
        let long = "مسلسل الهيبة الموسم الثالث الحلقة الأولى ترجمة عربية كاملة بجودة عالية";
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("S").row(Row::new(long).server_label()).row(Row::new("Off"))], 0, false);
        let draw = || {
            let mut runs: Vec<crate::draw_census::Run> = crate::draw_census::capture_runs(|| {
                t.draw(crate::Painter::recording(), frame, &crate::fixture::FixtureMeasure)
            })
            .into_iter()
            .filter(|r| r.text == long)
            .collect();
            runs.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            runs
        };
        // the label's column: its left edge is where a Latin label rests, its width the row's
        ROW_MARQUEE.with(|c| c.release());
        plx_machine::idle::frame_begin(1.0 / 60.0);
        let col_x = row_text_xs(&marquee_table(0), frame, 0).into_iter().fold(f32::MAX, f32::min);
        let col_w = t.label_width(&Row::new(long).server_label(), frame.w, &crate::fixture::FixtureMeasure);
        ROW_MARQUEE.with(|c| c.release());
        let rest = draw();
        assert_eq!(rest.len(), 2, "a run and its follower: {rest:?}");
        let w = crate::fixture::FixtureMeasure.width_str(long, rest[1].size, rest[1].bold);
        assert!(w > col_w, "the fixture label overflows its column");
        assert!((rest[1].at.x + w - (col_x + col_w)).abs() < 0.01, "flush right: {rest:?} w {w} column {col_x}+{col_w}");
        assert!((rest[0].at.x + w + crate::marquee::GAP - rest[1].at.x).abs() < 0.01, "follower on the left");
        plx_machine::idle::frame_begin((crate::marquee::ROW.hold_ms + 500.0) / 1000.0);
        let moved = draw();
        assert!((moved[1].at.x - rest[1].at.x - 20.0).abs() < 0.01, "it glides right: {rest:?} -> {moved:?}");
        ROW_MARQUEE.with(|c| c.release());
    }

    /// **And nothing else moves**: a focused label that fits, and an overflowing label on a row
    /// that is NOT focused, draw one still run and report nothing — a settled menu stays quiet.
    #[test]
    fn a_fitting_or_unfocused_label_draws_one_still_run_and_reports_nothing() {
        let _serial = plx_base::testlock::serial();
        let frame = Rect::new(0.0, 0.0, 500.0, 400.0);
        let t = marquee_table(1); // focus on "Off"; the long row is unfocused
        for _ in 0..3 {
            plx_machine::idle::frame_begin(1.5);
            let _ = plx_machine::idle::take_local_damage();
            assert_eq!(row_text_xs(&t, frame, 1).len(), 1, "a fitting focused label: one run");
            assert_eq!(row_text_xs(&t, frame, 0).len(), 1, "an unfocused overflowing label: one elided run");
            assert_eq!(plx_machine::idle::take_local_damage(), 0, "nothing animates, nothing reports");
        }
    }

    // ---- the focus pill's ink and the page edges ----------------------------------------------

    fn three_rows(sel: i32) -> TableView {
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("S").row(Row::new("a")).row(Row::new("b")).row(Row::new("c"))], sel, false);
        t
    }

    fn cover_of(t: &TableView, gi: i32) -> f32 {
        t.pill_cover(t.row_top(gi), t.row_height(gi))
    }

    /// **A row's ink follows the sliding pill, not the selection.** The reporter filmed Settings:
    /// moving focus down turned the NEXT row's label black on the first frame, while the pill was
    /// still a row away, so for a few frames it was dark text on the dark ground. The ink is a
    /// function of how much of the row's ink band the pill covers NOW, so it lands with the pill
    /// and the row the pill is leaving brightens as it goes.
    #[test]
    fn a_rows_ink_follows_the_pill_not_the_selection() {
        let _serial = plx_base::testlock::serial();
        let mut t = three_rows(0);
        assert_eq!((cover_of(&t, 0), cover_of(&t, 1)), (1.0, 0.0), "at rest the pill's row is inked, the others are not");

        t.move_sel(1);
        assert_eq!(t.sel, 1);
        t.update(1.0 / 60.0, 600.0);
        assert_eq!(cover_of(&t, 1), 0.0, "one frame in, the pill has not reached the new row's ink: it stays light");
        assert_eq!(cover_of(&t, 0), 1.0, "the pill still sits under the old row's ink: it stays dark");

        let (mut prev_new, mut prev_old) = (0.0f32, 1.0f32);
        let mut crossed = false;
        for _ in 0..240 {
            t.update(1.0 / 60.0, 600.0);
            let (new, old) = (cover_of(&t, 1), cover_of(&t, 0));
            if new > 0.0 {
                // ink never leads the fill: the new row only darkens once the pill's leading edge
                // is inside its ink band
                assert!(t.hl_bot.pos > t.row_top(1) + ROW_INK_PAD + PILL_INSET, "ink led the pill: cover {new} with the edge at {}", t.hl_bot.pos);
            }
            assert!(new + 1e-4 >= prev_new, "the arriving row's ink only grows: {prev_new} -> {new}");
            assert!(old <= prev_old + 1e-4, "the leaving row's ink only fades: {prev_old} -> {old}");
            crossed |= new > 0.0 && new < 1.0 && old > 0.0 && old < 1.0;
            (prev_new, prev_old) = (new, old);
        }
        assert!(crossed, "the premise: some frame has both rows part-covered");
        assert_eq!((cover_of(&t, 1), cover_of(&t, 0)), (1.0, 0.0), "settled: the new row is inked, the old is released");
    }

    /// A table that does not hold focus draws no pill, so no row is inked for one.
    #[test]
    fn an_unfocused_table_inks_no_row() {
        let t = three_rows(1).unfocused();
        assert_eq!((0..3).map(|i| cover_of(&t, i)).sum::<f32>(), 0.0);
    }

    /// **A page's rows run off the bottom of the SCREEN** — no cut at its safe-area frame, no
    /// fade. A panel (a popover at its height cap) is bounded, so it keeps the fade that stops its
    /// scissor cutting a glyph; a page is not, and the fade there hid the rows it scrolled to.
    #[test]
    fn a_page_table_lets_rows_slide_off_the_bottom_screen_edge() {
        let _serial = plx_base::testlock::serial();
        let mut t = TableView::new();
        let mut sec = Section::new("S");
        for i in 0..30 {
            sec = sec.row(Row::new(format!("row {i}")));
        }
        t.set_sections(vec![sec], 0, false);
        let frame = Rect::new(0.0, 0.0, 700.0, 600.0);
        let text_ys = |page: bool| -> Vec<f32> {
            crate::draw_census::capture(|| {
                let p = crate::Painter::recording();
                if page {
                    t.draw_page(p, frame, &crate::fixture::FixtureMeasure)
                } else {
                    t.draw(p, frame, &crate::fixture::FixtureMeasure)
                }
            })
            .into_iter()
            .filter(|(tag, _)| *tag == 100)
            .map(|(_, r)| r.y)
            .collect()
        };
        let (page, panel) = (text_ys(true), text_ys(false));
        assert!(panel.iter().all(|&y| y < frame.h), "a panel is cut at its frame");
        assert!(page.iter().any(|&y| y >= frame.h), "a page keeps drawing rows below its frame");
        // a row that STARTS on screen is drawn (and scissored at the edge); one that starts past it is culled
        assert!(page.iter().all(|&y| y < crate::consts::SCR_H + ROW_H), "rows past the screen's bottom edge are culled");
    }

    /// **The top edge dissolves; whatever rests at the top never does.** The first section label,
    /// the page title band and the first row all sit at or below [`TOP_FADE_H`] from the frame's top,
    /// so a table at rest is fully opaque, and a row is gone before its ink reaches the frame's edge.
    #[test]
    fn rows_dissolve_under_the_top_edge_and_nothing_at_rest_does() {
        assert_eq!(TableView::top_edge_alpha(100.0 + TOP_FADE_H, 100.0), 1.0);
        assert_eq!(TableView::top_edge_alpha(100.0 + TOP_FADE_H * 0.5, 100.0), 0.5);
        assert_eq!(TableView::top_edge_alpha(100.0, 100.0), 0.0, "ink at the edge is gone");
        assert_eq!(TableView::top_edge_alpha(40.0, 100.0), 0.0, "and so is ink above it");
        assert!(FIRST_HEADER_CAP_OFFSET >= TOP_FADE_H);
        assert!(TOP_PAD + ROW_INK_PAD >= TOP_FADE_H);
    }

    /// A row's badges stand `BADGE_GAP` apart edge to edge, and the run's width is the last chip's
    /// trailing edge: the one layout the width, the elision budget and the paint all read.
    #[test]
    fn a_rows_badge_run_stands_one_gap_apart() {
        let badges = [Badge::Forced, Badge::Sdh, Badge::Ad];
        let m = &plx_base::fontcov::advances::ShippedMeasure;
        let spans: Vec<Span> = badge_spans(&badges, 40.0, m).map(|(_, s)| s).collect();
        assert_eq!(spans.len(), 3);
        for pair in spans.windows(2) {
            assert!((pair[1].x - (pair[0].x + pair[0].w) - BADGE_GAP).abs() < 0.01);
        }
        assert_eq!(spans[0].x, 40.0);
        let last = spans[2];
        assert!((badge_run_w(&badges, m) - (last.x + last.w - 40.0)).abs() < 0.01);
        assert_eq!(badge_run_w(&[], m), 0.0);
    }

    /// **The badge run is PAINTED flush right, `BADGE_GAP` apart.** Reads the drawn chip labels
    /// (not `badge_spans` against itself): each chip's left edge is its label's drawn x less the
    /// chip's side padding, and the last chip's right edge lands on `text_right - trailing`.
    #[test]
    fn a_rows_painted_badges_stand_a_gap_apart_and_end_at_the_trailing_edge() {
        let _serial = plx_base::testlock::serial();
        use plx_machine::machine::Measure;
        let m = &plx_base::fontcov::advances::ShippedMeasure;
        let frame = Rect::new(0.0, 0.0, 900.0, 300.0);
        let badges = [Badge::Forced, Badge::Sdh, Badge::Ad];
        let mut t = TableView::new();
        let row = Row::new("a").badge(Badge::Forced).badge(Badge::Sdh).badge(Badge::Ad);
        t.set_sections(vec![Section::new("S").row(row)], 0, false);
        let runs = crate::draw_census::capture_runs(|| t.draw(crate::Painter::recording(), frame, m));
        let text_right = frame.x + frame.w - SIDE - CONTENT_PAD;
        let mut chips: Vec<(f32, f32)> = Vec::new(); // (left, right) of each painted chip, in draw order
        for b in &badges {
            let r = runs
                .iter()
                .find(|r| r.text == b.text() && r.size == theme::size::CAPTION && r.bold)
                .unwrap_or_else(|| panic!("chip {:?} not painted: {runs:?}", b.text()));
            let w = crate::widgets::badge_w(b.text(), None, m);
            let tw = m.width_str(b.text(), theme::size::CAPTION, true);
            let left = r.at.x - (w - tw) * 0.5;
            chips.push((left, left + w));
        }
        for pair in chips.windows(2) {
            assert!((pair[1].0 - pair[0].1 - BADGE_GAP).abs() < 0.5, "chips a gap apart: {chips:?}");
        }
        assert!((chips[2].1 - text_right).abs() < 0.5, "last chip ends at the trailing edge {text_right}: {chips:?}");
    }

    /// **The whole trailing run is PAINTED as one stack**: read-out, badge run, then the chevron's
    /// INK, each `ACCESSORY_GAP` from the next and the ink on the content edge. Reads the painted
    /// runs against the closed form (the chevron's ink width from `ink_x`), not against the stack.
    #[test]
    fn a_rows_painted_trailing_run_is_readout_then_badges_then_icon_ink_a_gap_apart() {
        let _serial = plx_base::testlock::serial();
        use plx_machine::machine::Measure;
        let m = &plx_base::fontcov::advances::ShippedMeasure;
        let frame = Rect::new(0.0, 0.0, 900.0, 300.0);
        let mut t = TableView::new();
        let row = Row::new("a").value("All").badge(Badge::Forced).badge(Badge::Sdh).chevron(true);
        t.set_sections(vec![Section::new("S").row(row)], 0, false);
        let runs = crate::draw_census::capture_runs(|| t.draw(crate::Painter::recording(), frame, m));
        let text_right = frame.x + frame.w - SIDE - CONTENT_PAD;
        let (l, r) = crate::icons::ink_x(crate::icons::Icon::Chevron);
        let ink_left = text_right - (r - l) * ACCESSORY_ICON_W;
        let chip = |b: Badge| {
            let run = runs.iter().find(|r| r.text == b.text() && r.size == theme::size::CAPTION && r.bold).expect("chip painted");
            let w = crate::widgets::badge_w(b.text(), None, m);
            let left = run.at.x - (w - m.width_str(b.text(), theme::size::CAPTION, true)) * 0.5;
            (left, left + w)
        };
        let (first, last) = (chip(Badge::Forced), chip(Badge::Sdh));
        assert!((last.1 + ACCESSORY_GAP - ink_left).abs() < 0.5, "last chip {last:?} a gap before the chevron ink at {ink_left}");
        let value = runs.iter().find(|r| r.text == "All" && r.bold).expect("read-out painted");
        assert_eq!(value.align, 2, "the read-out is right-aligned at its anchor");
        assert!((value.at.x + ACCESSORY_GAP - first.0).abs() < 0.5, "read-out ends a gap before the first chip {first:?}: {value:?}");
    }
}

#[cfg(test)]
mod classic_tests {
    use super::*;
    use plx_machine::machine::{FontScale, Measure, TypographyMeasure};
    #[test]
    fn classic_paints_both_label_shapes_at_the_measured_body_style() {
        let _serial = plx_base::testlock::serial();
        struct Restore(FontScale);
        impl Drop for Restore {
            fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
        }
        let _restore = Restore(crate::typography::current());
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            crate::typography::apply(scale, &mut measure).unwrap();
            for classic in [true, false] {
                let mut table = TableView::new();
                assert!(!table.compact, "exercise the ordinary OTT table configuration");
                table.set_classic(classic);
                table.set_sections(vec![Section::new("")
                    .row(Row::new("Single"))
                    .row(Row::new("Double").detail("Detail"))], 0, false);
                table.fit_layout(&measure);
                let runs = crate::draw_census::capture_runs(|| table.draw(
                    crate::Painter::recording(), Rect::new(0.0, 0.0, 900.0, 700.0), &measure));
                let size = if classic { theme::size::BODY } else { theme::size::HEADLINE };
                for label in ["Single", "Double"] {
                    let run = runs.iter().find(|run| run.text == label)
                        .unwrap_or_else(|| panic!("{label} missing at {scale}%: {runs:?}"));
                    assert_eq!((run.size, run.bold), (measure.font_scale().pixels(size), !classic),
                        "{label} paint differs from the measured style at {scale}% (Classic={classic})");
                }
            }
        }
    }
    #[test]
    fn classic_uses_one_geometry_for_font_fit_scroll_and_pointer_hits() {
        let mut t = TableView::new();
        t.set_classic(true);
        t.set_sections(vec![Section::new("").row(Row::new("Save")).row(Row::new("Cancel"))], 0, false);
        for scale in [20, 100, 200] {
            let mut m = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            m.set_font_scale(FontScale::from_percent(scale).unwrap());
            t.fit_layout(&m);
            let frame = Rect::new(700.0, 160.0, 600.0, 650.0);
            t.update(1.0, frame.h);
            for i in 0..2 {
                let row = t.row_frame(frame, i).unwrap();
                assert!(row.h >= m.cap_h(theme::size::BODY));
                assert!(row.y >= frame.y && row.y + row.h <= frame.y + frame.h);
                assert_eq!(t.hit_row(frame, row.x + row.w * 0.5, row.y + row.h * 0.5), Some(i));
            }
        }
    }
    #[test]
    fn modern_geometry_is_restored_after_classic_without_changing_selection() {
        let mut t = TableView::new();
        t.set_sections(vec![Section::new("").row(Row::new("one")).row(Row::new("two"))], 0, false);
        t.sel = 1;
        let m = crate::fixture::FixtureMeasure;
        t.fit_layout(&m);
        let modern = t.metrics.get();
        t.set_classic(true); t.fit_layout(&m);
        assert!(t.metrics.get().plain < modern.plain);
        assert_eq!(t.sel, 1);
        t.set_classic(false); t.fit_layout(&m);
        assert!(t.metrics.get() == modern);
        assert_eq!(t.sel, 1);
    }
}

#[cfg(test)]
mod classic_navigation_tests {
    use super::*;
    #[test]
    fn classic_rows_wrap_and_pages_clamp_without_full_list_allocations() {
        let mut table = TableView::new();
        let mut section = Section::new("");
        for i in 0..80 { section = section.row(Row::new(format!("row {i}"))); }
        table.set_sections(vec![section], 0, false);
        table.set_classic(true);
        table.set_classic_catalogue(true);
        table.fit_layout(&crate::fixture::FixtureMeasure);
        let page = Some(table.plain_row_h() * 10.0);
        assert_eq!(table.catalogue_next(0, -1, None), Some(79));
        assert_eq!(table.catalogue_next(79, 1, None), Some(0));
        assert_eq!(table.catalogue_next(0, -1, page), Some(0));
        assert_eq!(table.catalogue_next(79, 1, page), Some(79));
        assert_eq!(table.catalogue_next(0, 1, page), Some(10));
        table.row_visits.set(0);
        assert_eq!(table.catalogue_next(10, 1, page), Some(20));
        assert!(table.row_visits.get() <= 160, "at most row_top + one selection walk");
    }
    #[test]
    fn empty_single_and_separator_lists_have_defined_classic_edges() {
        let mut table = TableView::new();
        assert_eq!(table.catalogue_next(0, 1, None), None);
        table.set_sections(vec![Section::new("").row(Row::new("one"))], 0, false);
        assert_eq!(table.catalogue_next(0, -1, None), Some(0));
        assert_eq!(table.catalogue_next(0, 1, Some(700.0)), Some(0));
        table.set_sections(vec![Section::new("").row(Row::new("one")).row(Row::separator()).row(Row::new("two"))], 0, false);
        assert_eq!(table.catalogue_next(0, 1, None), Some(2));
        assert_eq!(table.catalogue_next(2, 1, None), Some(0));
    }
}

#[cfg(test)]
mod preferred_page_tests {
    use super::*;
    use plx_machine::machine::{FontScale, Measure, TypographyMeasure};
    fn table() -> TableView {
        let mut table = TableView::new();
        table.set_sections(vec![Section::new("")
            .row(Row::new("one"))], 0, false);
        table.sections[0].rows = (0..83).map(|n| Row::new(format!("row {n}"))).collect();
        table.set_classic(true);
        table.set_classic_catalogue(true);
        table
    }
    #[test]
    fn preferred_count_drives_page_step_scroll_and_hit_geometry() {
        let frame = Rect::new(100.0, 100.0, 700.0, 880.0);
        for requested in [10, 25, 30] { for scale in [20, 100, 150, 200] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let mut table = table();
            table.set_classic_page_size(Some(requested), frame.h);
            table.fit_layout(&measure);
            let count = table.classic_page_rows().expect("opted in page");
            assert!(count > 0 && count <= requested);
            assert!(count <= (requested as f32 / (scale as f32 / 100.0).max(1.0)).floor() as usize);
            assert!(table.plain_row_h() >= measure.cap_h(table.label_style().0).floor());
            assert_eq!(table.catalogue_next(0, 1, Some(frame.h)), Some(count as i32));
            assert_eq!(table.catalogue_next(82, 1, Some(frame.h)), Some(82));
            table.sel = count as i32;
            for _ in 0..240 { table.update(1.0 / 60.0, frame.h); }
            assert!((table.scroll_pos() - table.row_top(count as i32)).abs() < 0.1);
            for n in count..count * 2 {
                let row = table.row_frame(frame, n as i32).unwrap();
                assert!(row.y >= frame.y - 0.1);
                assert!(row.y + row.h <= frame.y + frame.h + 0.1);
                assert_eq!(table.hit_row(frame, row.x + row.w * 0.5, row.y + row.h * 0.5), Some(n as i32));
            }
            assert_eq!(table.catalogue_next(0, -1, None), Some(82));
        } }
    }
    #[test]
    fn studio_catalogue_keeps_requested_density_and_full_row_hit_targets() {
        let frame = Rect::new(780.0, 150.0, 1080.0, 810.0);
        for (requested, scale, expected_rows, expected_height) in [
            (10, 100, 10, 81.0), (25, 100, 25, 32.0),
            (30, 100, 30, 27.0), (25, 150, 16, 50.0), (25, 200, 12, 67.0),
        ] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let mut table = table();
            table.set_classic_page_size(Some(requested), frame.h);
            table.fit_layout(&measure);
            assert_eq!(table.classic_page_rows(), Some(expected_rows));
            assert_eq!(table.plain_row_h(), expected_height);
            let first = table.row_frame(frame, 0).unwrap();
            assert_eq!(first, Rect::new(frame.x, frame.y, frame.w, expected_height));
            let last = table.row_frame(frame, expected_rows as i32 - 1).unwrap();
            assert!(last.y + last.h <= frame.y + frame.h);
            assert_eq!(table.content_clip(frame).h, expected_rows as f32 * expected_height);
            assert_eq!(table.hit_row(frame, frame.x + 1.0, first.y + 1.0), Some(0));
            assert_eq!(table.hit_row(frame, frame.x + frame.w - 1.0, last.y + last.h - 1.0), Some(expected_rows as i32 - 1));
            assert_eq!(table.hit_row(frame, frame.x + 1.0, last.y + last.h), None,
                "the next page must not be clickable in the integer-packing remainder");
            assert!(table.row_rect(frame, expected_rows as i32).is_none());
            let mut visible = Vec::new();
            table.for_each_visible_row(frame, |index, _| visible.push(index));
            assert_eq!(visible, (0..expected_rows as i32).collect::<Vec<_>>(),
                "registered hit regions must expose exactly the painted page");
            table.sel = expected_rows as i32 - 1;
            for _ in 0..240 { table.update(1.0 / 60.0, frame.h); }
            assert!((table.pill_cover(table.row_top(table.sel), last.h) - 1.0).abs() < 0.001,
                "dense selected rows must retain readable ink on the selection fill");
        }
    }
    #[test]
    fn preferred_rows_are_opt_in_and_do_not_change_modern_or_generic_tables() {
        let measure = crate::fixture::FixtureMeasure;
        let table = table();
        table.set_classic_page_size(Some(10), 880.0);
        table.set_classic(false);
        table.fit_layout(&measure);
        assert!(table.classic_page_rows().is_none());
        let modern = table.plain_row_h();
        table.set_classic_page_size(None, 880.0);
        table.fit_layout(&measure);
        assert_eq!(modern, table.plain_row_h());
        table.set_classic(true);
        table.set_classic_catalogue(false);
        table.set_classic_page_size(Some(10), 880.0);
        table.fit_layout(&measure);
        assert!(table.classic_page_rows().is_none());
        assert_eq!(table.plain_row_h(), 44.0);
    }
}

#[cfg(test)]
mod catalogue_number_tests {
    use super::*;
    use plx_machine::machine::{FontScale, Measure, TypographyMeasure};

    struct RestoreTypography(FontScale);
    impl Drop for RestoreTypography {
        fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
    }
    fn catalogue(pairs: &[(&str, &str)], frame: Rect, requested: usize) -> TableView {
        let mut table = TableView::new();
        table.set_classic(true);
        table.set_classic_catalogue(true);
        table.set_classic_page_size(Some(requested), frame.h);
        table.set_sections(vec![pairs.iter().fold(Section::new(""), |section, (number, title)|
            section.row(Row::new(*title).catalogue_number(*number)))], 0, false);
        table
    }

    #[test]
    fn ordinal_builder_accepts_only_ascii_digits_without_changing_the_title() {
        for value in ["", " 1", "+1", "1.0", "١", "１２", "one"] {
            let row = Row::new("Title").catalogue_number(value);
            assert!(row.catalogue_number.is_none(), "{value:?}");
            assert_eq!(row.label, "Title");
        }
        let row = Row::new("Title").catalogue_number("001");
        assert_eq!(row.catalogue_number.as_deref(), Some("001"));
        assert_eq!(row.label, "Title");
        assert!(Row::new("Title").catalogue_number.is_none());
    }

    #[test]
    fn painted_ordinals_are_right_aligned_separately_after_reorder_and_filter() {
        let _serial = plx_base::testlock::serial();
        let _restore = RestoreTypography(crate::typography::current());
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        crate::typography::apply(100, &mut measure).unwrap();
        let frame = Rect::new(780.0, 150.0, 1080.0, 810.0);
        let mut table = catalogue(&[("1", "Alpha"), ("1000", "Beta")], frame, 25);
        for pairs in [vec![("1", "Alpha"), ("1000", "Beta")],
            vec![("1000", "Beta"), ("1", "Alpha")], vec![("1", "Alpha")]] {
            table.set_sections(vec![pairs.iter().fold(Section::new(""), |section, (number, title)|
                section.row(Row::new(*title).catalogue_number(*number)))], 0, false);
            table.fit_layout(&measure);
            let size = crate::classic::catalogue_text_size(frame.h, 25);
            let digits = pairs.iter().map(|(number, _)| number.len()).max().unwrap();
            let number_right = frame.x + 10.0 + measure.width_str("9", size, false) * digits as f32 + 9.0;
            let title_x = number_right + measure.width_str("\u{a0}", size, false);
            let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            for (index, (number, title)) in pairs.iter().enumerate() {
                let n = runs.iter().find(|r| r.text == *number).expect("separate number run");
                let t = runs.iter().find(|r| r.text == *title).expect("unprefixed title run");
                assert_eq!(n.align, 2);
                assert!((n.at.x - number_right).abs() < 0.01);
                assert!((t.at.x - title_x).abs() < 0.01);
                assert!((n.at.y - t.at.y).abs() < 0.01);
                assert_eq!((n.size, t.size), (size, size));
                let row = table.row_frame(frame, index as i32).unwrap();
                assert_eq!(table.hit_row(frame, frame.x + 1.0, row.y + row.h * 0.5), Some(index as i32));
                assert_eq!(table.sections[0].rows[index].label, *title);
            }
        }
    }

    #[test]
    fn max_digits_scan_is_cached_and_invalidated_by_supported_row_mutations() {
        let frame = Rect::new(0.0, 0.0, 720.0, 540.0);
        let mut table = catalogue(&[("1", "Alpha"), ("9", "Beta")], frame, 30);
        assert_eq!(table.catalogue_max_digits(), 1);
        assert!(table.take_row_visits() > 0);
        for _ in 0..100 {
            let _ = table.row_columns(&table.sections[0].rows[0], frame.w, &crate::fixture::FixtureMeasure);
        }
        assert_eq!(table.take_row_visits(), 0, "column layout must not rescan the catalogue per row");
        table.row_mut(0).unwrap().catalogue_number = Some("1000".into());
        assert_eq!(table.catalogue_max_digits(), 4);
        assert!(table.take_row_visits() > 0);
        table.set_sections(vec![Section::new("").row(Row::new("Beta").catalogue_number("2"))], 0, false);
        assert_eq!(table.catalogue_max_digits(), 1, "filter/rebuild must release the old four-digit width");
    }

    #[test]
    fn hidden_numbers_remove_the_column_and_forms_ignore_ordinals() {
        let _serial = plx_base::testlock::serial();
        let _restore = RestoreTypography(crate::typography::current());
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        crate::typography::apply(100, &mut measure).unwrap();
        let frame = Rect::new(520.0, 100.0, 720.0, 540.0);
        let mut table = catalogue(&[("", "Alpha")], frame, 30);
        table.row_mut(0).unwrap().checked = true;
        let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Alpha");
        let size = crate::classic::catalogue_text_size(frame.h, 30);
        assert!((runs[0].at.x - frame.x - 10.0 - measure.width_str("\u{a0}", size, false)).abs() < 0.01);
        assert!(crate::draw_census::capture_icons(|| table.draw(Painter::recording(), frame, &measure)).is_empty(),
            "catalogue rows must not reserve or draw the form checkmark column");
        for classic in [false, true] {
            table.set_classic(classic);
            table.set_classic_catalogue(false);
            table.fit_layout(&measure);
            let before = table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w;
            let plain = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            table.row_mut(0).unwrap().catalogue_number = Some("1000".into());
            let numbered = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            assert_eq!(numbered.len(), plain.len());
            let title = numbered.iter().find(|r| r.text == "Alpha").unwrap();
            let original = plain.iter().find(|r| r.text == "Alpha").unwrap();
            assert_eq!((title.at, title.size, title.bold), (original.at, original.size, original.bold));
            assert_eq!(table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w, before);
            table.row_mut(0).unwrap().catalogue_number = None;
        }
    }

    #[test]
    fn thirty_rows_at_720p_use_the_same_small_font_for_number_title_and_value() {
        let _serial = plx_base::testlock::serial();
        let _restore = RestoreTypography(crate::typography::current());
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        crate::typography::apply(100, &mut measure).unwrap();
        let frame = Rect::new(520.0, 100.0, 720.0, 540.0);
        let mut table = catalogue(&[("1", "Alpha")], frame, 30);
        table.row_mut(0).unwrap().value = Some("Readout".into());
        table.fit_layout(&measure);
        assert_eq!(table.classic_page_rows(), Some(30));
        assert_eq!(table.plain_row_h(), 18.0);
        let size = crate::classic::catalogue_text_size(540.0, 30);
        assert_eq!(size, 17);
        let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
        for text in ["1", "Alpha", "Readout"] {
            let run = runs.iter().find(|run| run.text == text).expect("number/title/value painted");
            assert_eq!((run.size, run.bold), (17, false));
        }
    }
}

#[cfg(test)]
mod classic_choice_tests {
    use super::*;
    use plx_machine::machine::{FontScale, TypographyMeasure};

    #[test]
    fn compact_choices_share_density_and_hit_geometry_without_catalogue_admission() {
        let _serial = plx_base::testlock::serial();
        let frame = Rect::new(780.0, 150.0, 1080.0, 810.0);
        for (scale, count, height) in [(100, 25, 32.0), (200, 12, 67.0)] {
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let mut table = TableView::new();
            table.set_classic(true);
            table.set_classic_choices(true);
            table.set_classic_page_size(Some(25), frame.h);
            table.set_sections(vec![Section::new("").row(Row::new("Interface").value("Classic"))
                .row(Row::new("Save Settings")).row(Row::new("Selected").checked(true))], 0, false);
            table.fit_layout(&measure);
            assert!(!table.is_classic_catalogue(), "rendering cannot grant catalogue key semantics");
            assert_eq!(table.classic_page_rows(), Some(count));
            let first = table.row_frame(frame, 0).unwrap();
            assert_eq!(first, Rect::new(frame.x, frame.y, frame.w, height));
            assert_eq!(table.hit_row(frame, first.x + 1.0, first.y + 1.0), Some(0));
            assert_eq!(table.row_frame(frame, 1).unwrap().y, first.y + first.h);
            let row = &table.sections[0].rows[0];
            assert_eq!(row.readout(), Some("Classic"));
            assert!(table.row_columns(row, frame.w, &measure).value_w > 0.0);
            // A scaled NBSP is not a size oracle for the independent icon column.
            // Check the actual painted accessory and label instead of comparing row insets.
            let mut runs = Vec::new();
            let icons = crate::draw_census::capture_icons(|| {
                runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            });
            assert!(matches!(icons.as_slice(), [(crate::icons::Icon::Check, _)]));
            let check = icons[0].1;
            let label = runs.iter().find(|run| run.text == "Selected").expect("checked label painted");
            let checked_frame = table.row_frame(frame, 2).unwrap();
            assert!(check.x >= checked_frame.x && check.y >= checked_frame.y);
            assert!(check.y + check.h <= checked_frame.y + checked_frame.h);
            assert!(check.x + check.w < label.at.x, "checkmark must clear the label at {scale}%");
            assert!(label.at.x < checked_frame.x + checked_frame.w);
            assert_eq!(table.hit_row(frame, check.x + check.w * 0.5, check.y + check.h * 0.5), Some(2));
            assert!(!table.blank_like().is_classic_catalogue());
            assert!(table.blank_like().is_classic_choices());
        }
    }

    #[test]
    fn compact_choice_opt_in_does_not_change_modern_or_editor_geometry() {
        let measure = crate::fixture::FixtureMeasure;
        let table = TableView::new();
        table.set_classic_choices(true);
        table.set_classic_page_size(Some(25), 810.0);
        table.fit_layout(&measure);
        assert_eq!(table.plain_row_h(), ROW_H);
        assert!(table.classic_page_rows().is_none());
        table.set_classic(true);
        table.set_classic_choices(false);
        table.fit_layout(&measure);
        assert_eq!(table.plain_row_h(), 44.0);
        assert!(table.classic_page_rows().is_none());
    }
}


#[cfg(test)]
mod classic_progress_tests {
    use super::*;
    use plx_machine::machine::{FontScale, TypographyMeasure};

    fn table(frame: Rect, count: usize) -> TableView {
        let mut table = TableView::new();
        table.set_classic(true);
        table.set_classic_catalogue(true);
        table.set_classic_page_size(Some(25), frame.h);
        let rows = (0..count).fold(Section::new(""), |section, n|
            section.row(Row::new(format!("Channel {n}")).progress_interval(100, 200)));
        table.set_sections(vec![rows], 0, false);
        table
    }

    #[test]
    fn intervals_use_half_open_time_and_can_be_cleared_on_programme_rollover() {
        let mut row = Row::new("Channel").progress_interval(100, 200);
        assert_eq!(row.progress_bounds(), Some((100, 200)));
        for (now, expected) in [(None, 0.0), (Some(99), 0.0), (Some(100), 0.0),
            (Some(150), 0.5), (Some(199), 0.99), (Some(200), 0.0), (Some(201), 0.0)] {
            assert!((row.progress_fraction(now) - expected).abs() < 0.00001);
        }
        assert!(!row.set_progress_interval(Some((100, 200))));
        assert!(row.set_progress_interval(Some((200, 200))));
        assert_eq!(row.progress_bounds(), None);
        assert_eq!(row.progress_fraction(Some(200)), 0.0);
        assert!(!row.set_progress_interval(Some((300, 200))));
        assert!(row.set_progress_interval(Some((200, 300))));
        assert_eq!(row.progress_fraction(Some(250)), 0.5);
        assert!(row.set_progress_interval(None));
        assert_eq!(row.label, "Channel", "interval updates do not change identity-bearing labels");
        let wide = Row::new("Channel").progress_interval(i64::MIN, i64::MAX);
        assert!((wide.progress_fraction(Some(0)) - 0.5).abs() < 0.00001,
            "subtraction must not overflow even at signed UTC bounds");
    }

    #[test]
    fn progress_reserves_the_same_scaled_slot_in_fit_draw_and_hit_geometry() {
        for (width, height, scale) in [(720.0, 540.0, 100), (1080.0, 810.0, 100),
            (1080.0, 810.0, 200)] {
            let frame = Rect::new(20.0, 30.0, width, height);
            let table = table(frame, 2);
            let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            table.fit_layout(&measure);
            let row = &table.sections[0].rows[0];
            let plain = Row::new(&row.label);
            let reserved = width * 40.0 / 720.0;
            let loss = table.row_columns(&plain, width, &measure).label_w
                - table.row_columns(row, width, &measure).label_w;
            assert!((loss - reserved - ACCESSORY_GAP).abs() < 0.01);
            let items = trailing_items(row, None, &measure, table.progress_width(row, width));
            let span = trailing_stack(&items, frame.x + width - table.row_content_padding())
                .placed().find(|span| span.i == 3).unwrap();
            let band = table.row_frame(frame, 0).unwrap();
            let bar = progress_frame(span, band.y, band.h);
            assert_eq!(bar.w, reserved);
            assert!(bar.y >= band.y && bar.y + bar.h <= band.y + band.h);
            assert_eq!(table.hit_row(frame, bar.x + bar.w * 0.5, bar.y + bar.h * 0.5), Some(0));
            let label_end = frame.x + table.row_content_padding()
                + table.row_label_inset(row, width, &measure) + table.row_columns(row, width, &measure).label_w;
            assert!((label_end + ACCESSORY_GAP - bar.x).abs() < 0.01,
                "the title budget ends before the actual shared-stack bar");
        }
    }

    #[test]
    fn progress_coexists_with_values_badges_icons_and_intrinsic_width() {
        let frame = Rect::new(0.0, 0.0, 1080.0, 810.0);
        let mut table = table(frame, 1);
        let row = table.row_mut(0).unwrap();
        row.value = Some("HD".into());
        row.badges = vec![Badge::Cc];
        row.ticon = Some(crate::icons::Icon::Chevron);
        row.catalogue_number = Some("1000".into());
        let measure = crate::fixture::FixtureMeasure;
        let width = table.measured_width(&measure);
        assert!(table.fit_report(width, &measure, fit::HEADROOM).is_empty(),
            "intrinsic width must include its proportional progress/number columns");
        let row = &table.sections[0].rows[0];
        let cols = table.row_columns(row, frame.w, &measure);
        let items = trailing_items(row, Some(cols.value_w), &measure, table.progress_width(row, frame.w));
        let spans: Vec<_> = trailing_stack(&items, frame.w - table.row_content_padding()).placed().collect();
        assert_eq!(spans.iter().map(|span| span.i).collect::<Vec<_>>(), [0, 1, 2, 3]);
        for pair in spans.windows(2) {
            assert!((pair[1].x - pair[0].x - pair[0].w - ACCESSORY_GAP).abs() < 0.01);
        }
        assert_eq!(spans[3].w, 60.0);
        let paint_items = trailing_items(row, Some(0.0), &measure, table.progress_width(row, frame.w));
        let value_anchor = trailing_stack(&paint_items, frame.w - table.row_content_padding())
            .placed().find(|span| span.i == 0).unwrap();
        assert!((value_anchor.x - spans[0].x - spans[0].w).abs() < 0.01,
            "zero-width value used by paint must have the same trailing edge as fit");
    }

    #[test]
    fn accepted_clock_is_constant_work_and_survives_table_rebuilds() {
        let frame = Rect::new(0.0, 0.0, 720.0, 540.0);
        let table = table(frame, 10_049);
        table.fit_layout(&crate::fixture::FixtureMeasure);
        let band = table.row_frame(frame, 0).unwrap();
        let rev = table.layout_rev();
        let mut old = plx_machine::machine::Canon::new();
        table.write_motion(&mut old);
        table.take_row_visits();
        assert!(table.set_classic_clock(Some(150)));
        assert!(!table.set_classic_clock(Some(150)));
        assert_eq!(table.take_row_visits(), 0, "a clock sample must never walk or rewrite rows");
        assert_eq!(table.layout_rev(), rev);
        let mut new = plx_machine::machine::Canon::new();
        table.write_motion(&mut new);
        assert_ne!(old.finish(), new.finish());
        assert_eq!(table.row_frame(frame, 0).unwrap(), band);
        let mut rebuilt = table.blank_like();
        rebuilt.set_sections(vec![Section::new("").row(Row::new("Next").progress_interval(100, 200))], 0, false);
        assert_eq!(rebuilt.classic_clock(), Some(150), "Form.set uses blank_like; do not blank progress until next tick");
        assert!(rebuilt.set_classic_clock(None));
        assert_eq!(rebuilt.sections[0].rows[0].progress_fraction(rebuilt.classic_clock()), 0.0);
    }

    #[test]
    fn actual_visible_bar_paint_keeps_slot_and_fade_when_selected_or_deselected() {
        let _serial = plx_base::testlock::serial();
        let frame = Rect::new(0.0, 0.0, 720.0, 540.0);
        let mut table = table(frame, 80);
        let measure = crate::fixture::FixtureMeasure;
        table.fit_layout(&measure);
        let band = table.row_frame(frame, 0).unwrap();
        let row = &table.sections[0].rows[0];
        let items = trailing_items(row, None, &measure, table.progress_width(row, frame.w));
        let span = trailing_stack(&items, frame.w - table.row_content_padding()).placed().find(|s| s.i == 3).unwrap();
        let expected = progress_frame(span, band.y, band.h);
        for selected in [true, false] {
            table.list_focused = selected;
            for (now, count) in [(None, 1), (Some(99), 1), (Some(150), 2), (Some(200), 1)] {
                table.set_classic_clock(now);
                let painted = crate::draw_census::capture_inks(|| table.draw_page(
                    Painter::recording().alpha(0.5), frame, &measure));
                let bars: Vec<_> = painted.iter().filter(|(tag, r, _)| *tag == 2 && *r == expected).collect();
                assert_eq!(bars.len(), count, "selected={selected}, now={now:?}");
                assert!((bars.last().unwrap().2 - 0.5).abs() < 0.001, "track inherits caller alpha");
                if count == 2 { assert!((bars[0].2 - 0.5).abs() < 0.001, "fill inherits caller alpha"); }
                let all_bars = painted.iter().filter(|(tag, r, _)| *tag == 2 && r.w == expected.w && r.h == expected.h).count();
                assert!(all_bars <= 2 * 26, "only visible rows may paint progress");
            }
        }
    }

    #[test]
    fn compact_archive_slot_and_logo_fit_dense_rows_without_changing_hit_identity() {
        let measure=crate::fixture::FixtureMeasure;
        let frame=Rect::new(0.0,0.0,720.0,540.0);
        for count in [25,50] {
            let mut table=table(frame,1);
            table.set_classic_page_size(Some(count),frame.h);
            table.fit_layout(&measure);
            let plain=table.row_columns(&table.sections[0].rows[0],frame.w,&measure).label_w;
            let row=table.row_mut(0).unwrap();
            *row=Row::new("Station  Current programme").catalogue_archive(true)
                .catalogue_icon(crate::icons::Icon::Show).progress_interval(100,200);
            table.fit_layout(&measure);
            let row=&table.sections[0].rows[0];
            let art=table.catalogue_art_width(row)-6.0;
            assert!(art/1.5<=table.plain_row_h()-2.0);
            assert!((plain-table.row_columns(row,frame.w,&measure).label_w-9.0-art-6.0).abs()<0.01);
            let width=table.row_columns(row,frame.w,&measure).label_w;
            table.row_mut(0).unwrap().catalogue_archive=Some(false);
            assert_eq!(table.row_columns(&table.sections[0].rows[0],frame.w,&measure).label_w,width,
                "unavailable archive leaves an aligned blank slot");
            let row=table.row_frame(frame,0).unwrap();
            assert_eq!(table.hit_row(frame,frame.x+frame.w-1.0,row.y+row.h*0.5),Some(0));
        }
    }

    #[test]
    fn modern_and_classic_choices_ignore_progress_metadata() {
        let frame = Rect::new(0.0, 0.0, 720.0, 540.0);
        for classic in [false, true] {
            let mut table = table(frame, 1);
            table.set_classic(classic);
            table.set_classic_catalogue(false);
            table.set_classic_choices(true);
            let measure = crate::fixture::FixtureMeasure;
            table.fit_layout(&measure);
            let width = table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w;
            let measured = table.measured_width(&measure);
            let band = table.row_frame(frame, 0).unwrap();
            assert_eq!(table.progress_width(&table.sections[0].rows[0], frame.w), None);
            table.row_mut(0).unwrap().set_progress_interval(None);
            assert_eq!(table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w, width);
            assert_eq!(table.measured_width(&measure), measured);
            assert_eq!(table.row_frame(frame, 0).unwrap(), band);
        }
    }
}

#[cfg(test)]
mod classic_secondary_tests {
    use super::*;
    use plx_machine::machine::{FontScale, Measure, TypographyMeasure};

    struct RestoreTypography(FontScale);
    impl Drop for RestoreTypography {
        fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
    }

    #[test]
    fn secondary_boundary_is_optional_utf8_safe_and_does_not_change_label_or_layout() {
        let mut row = Row::new("ČT  Zprávy");
        let h = row.height_in(92.0, 32.0);
        row.set_secondary_label_start(Some(1));
        assert_eq!(row.secondary_label_start(), None);
        row.set_secondary_label_start(Some("ČT  ".len()));
        assert_eq!(row.secondary_label_start(), Some(5));
        assert_eq!(row.label, "ČT  Zprávy");
        assert_eq!(row.height_in(92.0, 32.0), h);
        row.set_secondary_label_start(Some(row.label.len()));
        assert_eq!(row.secondary_label_start(), None);
    }

    #[test]
    fn classic_secondary_paints_two_tones_without_changing_columns_or_hit_target() {
        let _serial = plx_base::testlock::serial();
        let _restore = RestoreTypography(crate::typography::current());
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        crate::typography::apply(100, &mut measure).unwrap();
        let frame = Rect::new(780.0, 150.0, 1080.0, 810.0);
        let mut table = TableView::new();
        table.set_classic(true);
        table.set_classic_catalogue(true);
        table.set_classic_page_size(Some(25), frame.h);
        table.set_sections(vec![Section::new("").row(Row::new("News  Programme").catalogue_number("3"))], 0, false);
        table.fit_layout(&measure);
        let before = table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w;
        let band = table.row_frame(frame, 0).unwrap();
        table.row_mut(0).unwrap().set_secondary_label_start(Some(6));
        for selected in [false, true] {
            table.list_focused = selected;
            let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            let name = runs.iter().find(|r| r.text == "News  ").unwrap();
            let programme = runs.iter().find(|r| r.text == "Programme").unwrap();
            assert_eq!(name.ink, if selected { theme::CLASSIC_SELECTION_INK } else { theme::CLASSIC_TEXT });
            assert_eq!(programme.ink, if selected { theme::CLASSIC_SECONDARY_ON_SELECTION } else { theme::CLASSIC_SECONDARY });
            assert_eq!((name.size, name.bold, name.at.y), (programme.size, programme.bold, programme.at.y));
            assert!((programme.at.x - name.at.x - measure.width_str("News  ", name.size, name.bold)).abs() < 0.01);
        }
        assert_eq!(table.row_columns(&table.sections[0].rows[0], frame.w, &measure).label_w, before);
        assert_eq!(table.row_frame(frame, 0).unwrap(), band);
        assert_eq!(table.hit_row(frame, band.x + 1.0, band.y + band.h * 0.5), Some(0));
        table.set_classic_catalogue(false);
        let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
        assert!(runs.iter().any(|r| r.text == "News  Programme"), "ordinary forms retain their whole label run");
    }

    #[test]
    fn mixed_direction_and_bidi_controls_keep_the_original_whole_run() {
        let _serial = plx_base::testlock::serial();
        let _restore = RestoreTypography(crate::typography::current());
        let mut measure = TypographyMeasure::new(crate::fixture::FixtureMeasure);
        crate::typography::apply(100, &mut measure).unwrap();
        let frame = Rect::new(0.0, 0.0, 1080.0, 810.0);
        for title in ["News  חדשות 12", "News  الأخبار 12", "News  \u{202a}ABC\u{202c}",
            "News  \u{2068}ABC\u{2069}", "News  \u{200e}ABC"] {
            assert!(!plx_gfx::text::is_rtl_paragraph(title), "the old first-strong guard missed this");
            assert!(plx_gfx::text::needs_bidi_context(title));
            let mut table = TableView::new();
            table.set_classic(true);
            table.set_classic_catalogue(true);
            table.set_classic_page_size(Some(25), frame.h);
            table.set_sections(vec![Section::new("").row(Row::new(title))], 0, false);
            table.fit_layout(&measure);
            let before = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            let mut original = plx_machine::machine::Canon::new();
            table.write_motion(&mut original);
            table.row_mut(0).unwrap().set_secondary_label_start(Some(6));
            let after = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            let mut styled = plx_machine::machine::Canon::new();
            table.write_motion(&mut styled);
            assert_eq!(after, before, "do not split independently shaped bidi paragraphs");
            assert_eq!(original.finish(), styled.finish(), "derived colour metadata is not a motion/state input");
            assert!(after.iter().any(|run| run.text == title));
        }
        assert!(!plx_gfx::text::needs_bidi_context("News  Programme 12"));
    }
}

#[cfg(test)]
mod classic_menu_tests {
    use super::*;
    #[test]
    fn menu_opt_in_hides_only_chrome_and_detail_without_discarding_rows() {
        let _serial = plx_base::testlock::serial();
        let measure = crate::fixture::FixtureMeasure;
        let frame = Rect::new(780.0, 150.0, 1080.0, 810.0);
        let mut table = TableView::new();
        table.compact = true;
        table.set_sections(vec![Section::new("Options").row(Row::new("Alpha").detail("Help").value("On")),
            Section::new("Other").row(Row::new("Beta").checked(true))], 0, false);
        table.set_title(Some("Quality".into()));
        table.sel = 1;
        table.set_classic(true);
        table.set_classic_choices(true);
        table.fit_layout(&measure);
        let before = (table.row_top(0), table.row_top(1));
        assert!(table.title_rect(frame).is_some());
        table.set_classic_menu(true);
        table.fit_layout(&measure);
        assert_eq!(table.n_rows(), 2);
        assert_eq!(table.sel, 1);
        assert_eq!(table.row_top(0), 0.0);
        assert_eq!(table.row_top(1), table.plain_row_h());
        assert!(table.title_rect(frame).is_none());
        assert_eq!(table.sections[0].header, "Options");
        assert_eq!(table.sections[0].rows[0].detail, "Help");
        assert_eq!(table.sections[0].rows[0].value.as_deref(), Some("On"));
        assert!(table.sections[1].rows[0].checked);
        let row = table.row_frame(frame, 0).unwrap();
        assert_eq!(row.h, table.plain_row_h());
        assert_eq!(table.hit_row(frame, row.x + row.w / 2.0, row.y + row.h / 2.0), Some(0));
        let runs = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
        assert!(runs.iter().any(|r| r.text == "Alpha"));
        assert!(!runs.iter().any(|r| matches!(r.text.as_str(), "OPTIONS" | "OTHER" | "QUALITY" | "Help")));
        assert!(table.blank_like().is_classic_menu());
        table.set_classic_menu(false);
        table.fit_layout(&measure);
        assert_eq!((table.row_top(0), table.row_top(1)), before);
        table.set_classic(false);
        table.fit_layout(&measure);
        let modern = (table.row_top(0), table.row_top(1));
        table.set_classic_menu(true);
        table.fit_layout(&measure);
        assert_eq!((table.row_top(0), table.row_top(1)), modern, "opt-in alone cannot change Modern");
        assert!(table.title_rect(frame).is_some());
    }
}

#[cfg(test)]
#[path = "table_window_tests.rs"]
mod window_tests;

#[cfg(test)]
mod palette_tests {
    use super::*;
    #[test]
    fn existing_table_reads_new_palette_on_draw_in_both_modes_and_reset_restores_it() {
        let _serial = plx_base::testlock::serial();
        struct Restore(crate::palette::Config);
        impl Drop for Restore { fn drop(&mut self) { crate::palette::project(self.0); } }
        let _restore = Restore(crate::palette::current());
        let measure = crate::fixture::FixtureMeasure;
        let frame = Rect::new(80.0,100.0,1000.0,700.0);
        for classic in [false,true] {
            let mut table = TableView::new();
            table.set_classic(classic);
            table.set_sections(vec![Section::new("").row(Row::new("Palette row"))],0,false);
            table.fit_layout(&measure);
            table.list_focused = false;
            let draw = |table:&TableView| crate::draw_census::capture_runs(||
                table.draw(Painter::recording(),frame,&measure));
            crate::palette::project(crate::palette::Config::default());
            let original = draw(&table).into_iter().find(|r|r.text=="Palette row").unwrap().ink;
            let config = crate::palette::Config::default()
                .with(crate::palette::Role::Text,Some("#123456")).unwrap()
                .with(crate::palette::Role::SelectionText,Some("#ABCDEF")).unwrap();
            crate::palette::project(config);
            let current = draw(&table).into_iter().find(|r|r.text=="Palette row").unwrap().ink;
            assert_eq!(current,config.resolve(crate::palette::Role::Text,original));
            table.list_focused = true;
            let selected = draw(&table).into_iter().find(|r|r.text=="Palette row").unwrap().ink;
            assert_eq!(selected,config.resolve(crate::palette::Role::SelectionText,original));
            table.list_focused = false;
            crate::palette::project(crate::palette::Config::default());
            assert_eq!(draw(&table).into_iter().find(|r|r.text=="Palette row").unwrap().ink,original);
        }
    }
}

#[cfg(test)]
mod detail_marker_tests {
    use super::*;
    #[test]
    fn accepted_marker_paints_before_detail_without_shifting_title_in_both_modes() {
        let _serial = plx_base::testlock::serial();
        let measure = crate::fixture::FixtureMeasure;
        let frame = Rect::new(0.0, 0.0, 900.0, 300.0);
        for classic in [false, true] {
            let mut table = TableView::new(); table.set_classic(classic);
            let make = || Row::new("Programme").detail("12:34");
            table.set_sections(vec![Section::new("").row(make())], 0, false);
            let plain = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            let title = plain.iter().find(|r| r.text == "Programme").unwrap().at.x;
            let date = plain.iter().find(|r| r.text == "12:34").unwrap().at.x;
            table.set_sections(vec![Section::new("").row(make().detail_marker(theme::CLASSIC_REMOTE_YELLOW))], 0, false);
            let mut marked = Vec::new();
            let rects = crate::draw_census::capture_rect_inks(|| {
                marked = crate::draw_census::capture_runs(|| table.draw(Painter::recording(), frame, &measure));
            });
            let detail = marked.iter().find(|r| r.text == "12:34").unwrap();
            assert_eq!(marked.iter().find(|r| r.text == "Programme").unwrap().at.x, title);
            assert_eq!(detail.at.x, date + 16.0);
            let dot = rects.iter().find(|(rect, _, _)| rect.w == 8.0 && rect.h == 8.0).unwrap().0;
            assert_eq!(dot.x, date); assert!(dot.x + dot.w < detail.at.x);
            assert_eq!(table.sections[0].rows[0].detail_marker, Some(theme::CLASSIC_REMOTE_YELLOW));
            table.set_sections(vec![Section::new("").row(make())], 0, false);
            let removed = crate::draw_census::capture_rect_inks(|| table.draw(Painter::recording(), frame, &measure));
            assert!(!removed.iter().any(|(rect, _, _)| rect.w == 8.0 && rect.h == 8.0));
        }
    }
}

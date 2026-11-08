//! **The classic layout**: its constants (`SB_*`, `BTN_*`, `CTRL_*`), the hit rects, the geometry it
//! reports ([`frame_geometry`], [`geometry`]) and every draw of the transport: scrim, title, playbar,
//! control row, bottom tabs, scrub preview and the failure read-out.
//!
//! Every draw takes `&`[`HudModel`](super::model::HudModel) for the values that used to be read from
//! process globals in the middle of drawing, and a [`GroupId`] for the focus, so this file names no
//! player global. The logic it draws ([`super::state`]) and the animation state it reads
//! ([`super::motion`]) are separate modules; a layout other than this one has to answer the same
//! [`HudGeometry`](super::geometry::HudGeometry).
use super::geometry::{HudGeometry, RowAnchor, GROUP_ROW, GROUP_SCRUB, GROUP_TABS, HUD_TABS};
use super::model::{HudModel, OsdOpacity};
use super::motion::*;
use super::state::*;
use super::HudBand;
use plx_machine::machine::{GroupId, Measure};
use plx_ui::consts::{SCR_H, SCR_W};
use plx_ui::hstack::{Align, HStack, Item};
use plx_ui::theme;
use plx_ui::widgets::{
    ControlGround, Spinner, StatusKind, StatusOverlay, TabPill,
};
use plx_ui::{Env, Painter, Rect, View};
use std::ffi::CString;
use std::os::raw::c_int;

/// The HUD widgets draw purely from their own fields and ignore their Env.
pub(super) fn hud_env() -> Env {
    Env::inert()
}

// The now-playing title under the playbar is a HUD *display* title — deliberately larger than
// `theme::size::TITLE`, so it sits OUTSIDE the shared type scale as a documented carve-out (like the
// failure-message caption in `draw_subtitle_message`). Both are media chrome with their own legibility contract and
// both are already well above the couch floor; they're named here rather than left as bare literals.
// The HUD package format's `HUD_TITLE` type rung (`hud_format::schema::SIZE_RUNGS`) reads this very const.
pub(crate) const HUD_TITLE_SZ: i32 = 54;

/// the shared playback clock ([`plx_ui::fmt::clock`]) with the HUD's leading '-' for remaining
pub(super) fn fmt_time(ns: i64, neg: bool) -> String {
    let c = plx_ui::fmt::clock(ns / 1_000_000);
    if neg {
        format!("-{c}")
    } else {
        c
    }
}

// ---- HUD geometry (shared by draw_hud + the pointer hit-tests in app.rs) ----
// scrubber (and title, and the bottom tab pills) left margin — the app's own, not a second copy of
// it: this was a literal 90 and so stayed put when `MARGIN_X` moved to the overscan-safe 96
pub(super) const SB_X: f32 = plx_ui::consts::MARGIN_X;
pub const fn sb_w() -> f32 {
    SCR_W - 2.0 * SB_X
}
pub(super) const SB_Y: f32 = SCR_H - 198.0;
pub(super) const SB_H: f32 = 8.0;
/// The radius of the largest thing the bar draws around the playhead, the focused knob's glow (a 34 px
/// disc centred on the track). The caption band counts it as part of the bar: `hud_format::compile`.
pub const KNOB_GLOW_R: f32 = 17.0;
pub(super) const BTN_S: f32 = 64.0; // right-side control button size (mockup ≈ 68)
pub(super) const BTN_Y: f32 = HudBand::CLASSIC.controls_top_y;

// The control row, exported for `skip_pill` — while a marker is under the playhead the Skip button
// STANDS IN for the two discs, and it has to land on exactly their row and right edge or the
// transport visibly jumps when a segment begins.
/// control-row height (one disc's diameter)
pub const CTRL_H: f32 = BTN_S;
/// control-row top
pub const CTRL_Y: f32 = BTN_Y;
/// control-row right edge — the app's side margin, matching the track-menu panel.
///
/// The mock's `right: 80` put the Subtitles/Audio/`…` discs 16px past the 5% overscan frame, and the
/// track and `…` panels were aligned to the same 80 (`track_menu`/`more_menu`). All three moved onto
/// `MARGIN_X` together on 2026-08-23 — they are one right edge by design, so they take one number.
pub const CTRL_RIGHT: f32 = SCR_W - plx_ui::consts::MARGIN_X;
/// width the disc row occupies, the floor for anything replacing it
pub fn ctrl_row_w() -> f32 {
    disc_row().width()
}

/// **The baseline disc row as ONE run**: absent Previous/Next followed by the three always-present
/// discs of [`BTN_S`], `CONTROL_GAP` apart, anchored on [`CTRL_RIGHT`]. [`btn_x`], the row's width
/// ([`ctrl_row_w`]) and the hit rects all read this one stack.
fn disc_row() -> HStack<'static> {
    const DISCS: [Item; BTN_N as usize] = disc_items(false);
    disc_stack(&DISCS)
}

/// The disc row's one layout — its gap and its anchor on [`CTRL_RIGHT`] — over either item set.
fn disc_stack(items: &[Item]) -> HStack<'_> {
    HStack::new(items, plx_ui::widgets::CONTROL_GAP, Align::Trailing(CTRL_RIGHT))
}

/// The shared control-row slot for anything that STANDS IN for the disc pair: right-aligned to the
/// discs' own edge, and never narrower than the pair it replaces so the row does not visibly shrink
/// when it appears. ONE geometry for both stand-ins and for the pointer hit-test.
///
/// The measured width is memoised per label, avoiding repeated capability calls and string
/// preparation for fixed labels in addition to the native font-metric cache.
pub fn ctrl_slot(row: &mut HudMotion, label: &str, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let memo = &mut row.widths;
    let w = match memo.iter().find(|(l, _)| l == label) {
        Some((_, w)) => *w,
        None => {
            let measured = ctrl_slot_measure(label, measure);
            // `text_width` reads 0 until `init_text` has run — don't cache a pre-init measurement
            if measured > ctrl_row_w() {
                memo.push((label.to_string(), measured));
            }
            measured
        }
    };
    Rect::new(CTRL_RIGHT - w, CTRL_Y, w, CTRL_H)
}

/// [`ctrl_slot`]'s width without writing the memo — the cached width when the draw has measured
/// `label`, else the same measurement it would cache.
pub fn ctrl_slot_w(row: &HudMotion, label: &str, measure: &dyn plx_machine::machine::Measure) -> f32 {
    row.widths.iter().find(|(l, _)| l == label).map_or_else(|| ctrl_slot_measure(label, measure), |(_, w)| *w)
}

pub(super) fn ctrl_slot_measure(label: &str, measure: &dyn plx_machine::machine::Measure) -> f32 {
    stand_in_w(measure.width_str(label, theme::size::BODY, true), ctrl_row_w())
}

/// A stand-in pill's side padding: the air between its label and its edge, each side.
const STAND_IN_PAD_X: f32 = 34.0;

/// **The width of a stand-in pill** (Skip, Next Episode): its label's bold advance and the padding
/// each side, never narrower than `floor`. Pure, so [`ctrl_slot_measure`] (the draw and the hit
/// test) and the HUD layout compiler (`hud_format::compile`, which supplies the floor its package's
/// replaced segment gives) size the pill with the one formula.
pub fn stand_in_w(advance: f32, floor: f32) -> f32 {
    (advance + 2.0 * STAND_IN_PAD_X).max(floor)
}

/// The five possible discs, in one right-anchored layout. Absent episode controls contribute
/// neither width nor gaps, so Subtitles, Audio and More never move when the pair appears.
const fn disc_items(pair: bool) -> [Item; BTN_N as usize] {
    [if pair { Item::new(BTN_S) } else { Item::absent() },
     if pair { Item::new(BTN_S).after(2.0 * plx_ui::widgets::CONTROL_GAP) } else { Item::absent() },
     Item::new(BTN_S), Item::new(BTN_S), Item::new(BTN_S)]
}

impl Controls {
    pub fn rect(self, i: i32) -> Option<Rect> {
        let items = disc_items(self.pair());
        let (x, w) = disc_stack(&items).span(usize::try_from(i).ok()?)?;
        Some(Rect::new(x, BTN_Y, w, BTN_S))
    }
}

impl ControlSlot {
    /// **Item `idx` of whatever occupies the row, at the rect it is DRAWN at** — the one
    /// geometry `PlayerScreen`'s `Focusable::place` answers with, and therefore the rect its stop
    /// is registered at, for all three occupants and every item of each (`0..items()`). `None`
    /// past the occupant's last item or at an absent disc. Disabled visible discs still have rects.
    ///
    /// Read-only over `row`'s label-width memo ([`ctrl_slot_w`]): `place` is `&self`, and a width
    /// the draw has not cached yet is measured the same way the draw measures it.
    pub fn item_rect(self, row: &HudMotion, idx: c_int, measure: &dyn plx_machine::machine::Measure) -> Option<Rect> {
        if idx < 0 || idx >= self.items() {
            return None;
        }
        Some(match self {
            ControlSlot::Discs(controls) => controls.rect(idx)?,
            ControlSlot::Skip(pr) => {
                let w = ctrl_slot_w(row, pr.label(), measure);
                Rect::new(CTRL_RIGHT - w, CTRL_Y, w, CTRL_H)
            }
            ControlSlot::UpNext(_) => {
                let l = crate::up_next::layout_peek(row, measure);
                if idx == crate::up_next::BTN_NEXT { l.next } else { l.credits }
            }
        })
    }
}

/// The bottom scrim's height — the transport's dark ground, transparent at its top edge and
/// `theme::scrim_black(0.86)` at the panel bottom. Named because the read-out's placement is graded
/// against it (see [`readout_frame`] and its test) rather than against a second hand-typed 470. The HUD
/// package format bounds a scrim's `extra` and `height` by it (`hud_format::schema`), the same const.
pub(crate) const SCRIM_H: f32 = 470.0;

/// The read-out's frame: **the whole panel**, because the wait is about the whole picture.
///
/// It is deliberately NOT carved down to dodge the transport. The block [`StatusOverlay`] builds is
/// only ~91 px tall, so centred on the panel it spans y≈482…573 — clear of the scrim's top edge at
/// `SCR_H - SCRIM_H` (610) and of the transport's highest ink (the Up Next still at ≈597). The
/// carve-out this replaces (`SCR_H - 340`) centred it at y=370, visibly high, and bought nothing:
/// at 370 the block was already 200 px above a scrim that is fully TRANSPARENT at its top edge
/// anyway. Every "sit above the transport" framing pushes it UP — the un-scrimmed band's own centre
/// is 305 — which is the opposite of the fix the report asked for.
pub(super) fn readout_frame() -> Rect {
    Rect::FULL
}

/// The load / seek / failure read-out. Drawn EVERY player frame, independent of the transport's
/// auto-hide, because its visibility is the PLAYBACK STATE and not the HUD timer.
///
/// It used to live inside [`draw_hud`], and two things came of that. Historically: `PlaybackState`
/// was published every frame and NOTHING read it, so the initial load drew a live-looking transport
/// at 0:00 / -0:00 and a dead producer drew a fully black screen with no message and no hint that
/// BACK is the way out. Then, once it did read it: in `Error` — which is deliberately not
/// `is_busy()`, hence not covered by `app.rs`'s `|| player::loading()` HUD pin — "Playback failed"
/// disappeared 4.5 s in with the HUD linger, leaving exactly the silent black screen the read-out
/// exists to prevent. A read-out is not transport chrome.
pub fn draw_readout(
    ps: &plx_media::route::PlaybackSession,
    busy: Busy,
    now: u32,
    failure_focus: usize,
    measure: &dyn plx_machine::machine::Measure,
) {
    let Busy::Readout(kind, caption) = busy else {
        return;
    };
    if kind == StatusKind::Failed {
        // The failure is a page read-out (`FailureReadout`): the shared page placement, a
        // constant verdict, one reason, a row of controls. The caption is not drawn here: the
        // verdict line is constant by design, so a viewer who has seen it once recognises it
        // before reading, and the caption's suffix is re-derived as the reason.
        draw_failed_readout(ps, Painter::root(), failure_focus, measure);
        return;
    }
    StatusOverlay::new(readout_frame(), caption, kind)
        .phase(now)
        .draw(&hud_env(), Painter::root());
}

/// The support line's cap top this far above the canvas bottom — inside the safe area's
/// bottom margin band's upper edge, clear of the row by construction, and
/// `the_diagnostics_footer_sits_below_the_row_inside_the_safe_bottom_band` fails if a taller row or
/// note ever meets it.
pub(super) const FOOTER_TOP: f32 = SCR_H - plx_ui::consts::MARGIN_Y - theme::space::XL;

impl FailureReadout {
    fn overlay(&self, labels: &[&'static std::ffi::CStr], focus: Option<usize>) -> StatusOverlay<'_> {
        StatusOverlay::new(Rect::FULL, plx_platform::i18n::msg::widgets_status_failed_c(), StatusKind::Failed)
            .page(self.glyph)
            .reason(&self.reason)
            .row(labels)
            .note(self.note.as_deref())
            .focus(focus)
    }

    fn labels(&self) -> Vec<&'static std::ffi::CStr> {
        self.actions.iter().map(|a| failure_action_label(*a)).collect()
    }

    /// Each control's rect, by row index — the geometry the draw uses.
    pub fn frames(&self, measure: &dyn plx_machine::machine::Measure) -> [Option<Rect>; plx_ui::widgets::STATUS_ROW_MAX] {
        let labels = self.labels();
        self.overlay(&labels, None).row_frames_measured(measure)
    }

    pub(super) fn draw(&self, p: Painter, focus: usize, measure: &dyn plx_machine::machine::Measure) {
        let labels = self.labels();
        self.overlay(&labels, Some(focus.min(labels.len().saturating_sub(1))))
            .draw_measured(&hud_env(), p, measure);
        plx_ui::text_view::TextView::new(self.footer.to_str().unwrap_or(""), theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_TERTIARY))
            .h(plx_ui::label::HAlign::Center)
            .max_lines(1)
            // the full safe width, not the reason's: the failure code is the line's LAST field
            // and the one support needs, so it must not be the part an ellipsis eats
            .draw(p, Rect::new(plx_ui::consts::MARGIN_X, FOOTER_TOP, SCR_W - 2.0 * plx_ui::consts::MARGIN_X, 0.0));
    }
}

/// The scrubber's GRAB band — deliberately much taller than the bar itself, because it is a
/// pointer grab zone. Placed by `PlayerScreen::place` and registered from it as the LOWEST stop, so
/// every control over the band outranks it (issue #162); it was `scrub_hit`'s own rectangle until phase 12 made
/// the hit map the one thing that tests it.
pub(super) fn scrub_hit_rect() -> Rect {
    Rect::new(SB_X, SCR_H - 270.0, sb_w(), 160.0)
}

/// **The control row's band at its floor width** ([`ctrl_row_w`]) — the row GROUP's extent only.
///
/// It is not any item's hit region: every item of whatever occupies the row — a disc, the Skip
/// pill, either Up Next button — is placed and registered at its own drawn rect by
/// [`ControlSlot::item_rect`]. This one region used to BE the stand-ins' registered stop, which
/// left *Watch Credits* (drawn left of it) unclickable and made a click on *Next Episode* land on
/// item 0, *Watch Credits* (issue #162's audit).
pub(super) fn ctrl_row_hit_rect() -> Rect {
    let w = ctrl_row_w();
    Rect::new(CTRL_RIGHT - w, CTRL_Y, w, CTRL_H)
}

/// One bottom tab's rect, matching the left-to-right layout [`draw_hud`] lays the pills out with.
pub fn tab_hit_rect(idx: i32, has_chapters: bool, measure: &dyn plx_machine::machine::Measure) -> Option<Rect> {
    let tabs = tab_labels(has_chapters);
    let items = tab_items(tabs, measure);
    let (px, pw) = tab_row(&items).span(usize::try_from(idx).ok()?)?;
    Some(Rect::new(px, tab_y(), pw, BTN_S))
}

/// The bottom tab pills' labels: Info always, Chapters only when the item has any; `None` is a
/// tab the row does not carry.
fn tab_labels(has_chapters: bool) -> [Option<&'static str>; HUD_TABS] {
    [
        Some(plx_platform::i18n::msg::widgets_player_info()),
        has_chapters.then(plx_platform::i18n::msg::widgets_player_chapters),
    ]
}

/// The tabs' top: BTN_S tall, centred between the play bar (scrubber, at `SB_Y`) and the bottom edge.
pub(super) fn tab_y() -> f32 {
    (SB_Y + SCR_H) * 0.5 - BTN_S * 0.5
}

/// One item per label, as wide as its pill ([`TabPill::width_measured`]); a tab the row does not
/// carry is ABSENT. Both the draw and [`tab_hit_rect`] build the row from these, so a pill is drawn
/// where it is hit.
fn tab_items(tabs: [Option<&str>; HUD_TABS], measure: &dyn plx_machine::machine::Measure) -> [Item; HUD_TABS] {
    tabs.map(|l| match l {
        Some(l) => Item::new(TabPill::width_measured(l, theme::size::BODY, measure)),
        None => Item::absent(),
    })
}

/// The tab pills as ONE run from the scrubber's left edge, `space::SM` apart.
fn tab_row(items: &[Item; HUD_TABS]) -> HStack<'_> {
    HStack::new(items, theme::space::SM, Align::Leading(SB_X))
}

fn draw_failed_readout(
    ps: &plx_media::route::PlaybackSession,
    p: Painter,
    focus: usize,
    measure: &dyn plx_machine::machine::Measure,
) {
    // The GROUND, first: a full-bleed opaque black, one quad. Without it the read-out stood on
    // whatever the video plane happened to be holding: `app.rs` clears the graphics plane to alpha
    // 0 on the player route, and the transport is not drawn at all under a failure. "The plane is
    // black by the time an error is up" is usually true and not always — `busy_surface` resolves
    // Error to Failed for BOTH values of `seen_frame`, i.e. a mid-playback failure over a held
    // frame is a state the code contemplates.
    p.rrect(Rect::FULL, 0.0, 0.0, theme::scrim_black(1.0));
    FailureReadout::now(ps).draw(p, focus, measure);
}

/// x of control button `idx`, left to right: 0 = Subtitles, 1 = Audio, 2 = More.
///
/// The row is RIGHT-anchored — the LAST disc's edge is `CTRL_RIGHT` — so adding a button pushes
/// the existing ones left instead of moving the row's right margin, which is what keeps the discs
/// and every stand-in (`ctrl_slot`) sharing one edge.
#[cfg(test)]
pub(super) fn btn_x(idx: i32) -> f32 {
    disc_row().span(idx as usize).map_or(CTRL_RIGHT, |(x, _)| x)
}

/// Which enabled control disc a pointer at (cx,cy) is over, by its stable row index.
/// Returns stable row indices, skipping disabled controls; geometry is shared with draw_hud.
///
/// `slot` is passed in rather than re-derived: while a stand-in owns the row the discs are not on
/// screen, and a click in that band must not open a track menu the user cannot see.
pub fn icon_hit(slot: ControlSlot, cx: f32, cy: f32) -> Option<i32> {
    let ControlSlot::Discs(controls) = slot else { return None };
    if cy < BTN_Y || cy > BTN_Y + BTN_S {
        return None;
    }
    (0..slot.items()).find(|&idx| {
        controls.enabled(idx) && controls.rect(idx).is_some_and(|r| cx >= r.x && cx <= r.x + r.w)
    })
}

// (`scrub_hit` stood here — the scrub band's own `(cx,cy) -> Option<frac>` predicate, `icon_hit`'s
// twin for the bar. Restructure phase 12 (PX-PLAYER) retired it: the band is a registered `Stop`
// now ([`scrub_hit_rect`], the SAME rectangle it tested), so the shared hit map answers "is the
// pointer on the bar" and the screen asks only "where along it" — `HudGeometry::scrub_frac`, the
// half that was always x-only. Deleting it is also what makes `ci/check-deps.sh`'s `hittest`
// gate meaningful rather than merely satisfied: there is no raw hit-tester left for `app/` to
// call.)

// ---- the rail carries NO marks ------------------------------------------------------------
// Nothing is drawn on the scrubber but the track, the watched fill and the playhead. Two kinds of
// mark have been tried here and both were removed, for the same reason and by the same judgement:
//
//   * chapter-boundary ticks — the rail reads cleaner as one continuous bar. The chapter LIST is
//     the affordance (`appkit/chapters_panel.rs`, off `metadata::playing_chapters()`).
//   * intro/credits segment bands — removed 2026-08-04. They were `RAIL_MARKER`, white at 0.42
//     against a 0.20 track, so a credits band on a feature film was a bright unlabelled patch
//     floating near the right end of an otherwise empty rail. Reviewed cold on a screenshot it
//     read as a RENDERING ARTIFACT, not as information, which is a complete failure of the thing.
//
// Neither loses anything: the Skip Intro / Skip Credits pill (`appkit/skip_pill.rs`) is driven from
// the very same `metadata::playing_markers()` and appears exactly when a marker is reachable, so
// the band was decoration duplicating a control that already announces itself. Do not re-add
// either as a "cheap win" — the marker data is already in memory, which is precisely what makes
// drawing it tempting and still wrong.

/// draw a clock string centred on `cx` as ONE label box, clamped so it stays inside [lo, hi].
/// (The old last-':'-anchor read visibly lopsided once the clock grew to H:MM:SS — "1:05" left of
/// the knob vs "53" right.) The box width is measured on a same-shape template with every digit
/// as '0', so the box — and the returned extents — stay stable while digits tick instead of
/// wobbling with proportional digit widths. Returns the label's (left, right) x extents.
fn draw_clock(
    p: Painter,
    text: &str,
    cx: f32,
    y: f32,
    sz: i32,
    col: [f32; 4],
    lo: f32,
    hi: f32,
    measure: &dyn plx_machine::machine::Measure,
) -> (f32, f32) {
    let template: String = text
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    let w = measure.width_str(&template, sz, true);
    let half = w * 0.5;
    let cx = cx.clamp(lo + half, (hi - half).max(lo + half));
    if let Ok(cs) = CString::new(text) {
        p.text(cs.as_ptr(), cx, y, sz, col, 1, 1);
    }
    (cx - half, cx + half)
}

/// An open Info/Chapters panel owns input; its retained HUD tab indicates mode only.
/// Returning to the transport restores its actionable tab focus without losing that cursor.
pub(super) fn hud_tab_state(transport: bool, focus: GroupId, tab: i32, index: i32) -> (bool, bool) {
    let current = tab == index;
    (transport && focus == GROUP_TABS && current, !transport && current)
}

/// The transport's dark ground: transparent at its top edge, `theme::scrim_black(0.86)` at the
/// panel bottom, [`SCRIM_H`] tall. Drawn under every transport, the player's and the trailer's.
pub fn draw_scrim(p: Painter) {
    draw_scrim_with_opacity(p, OsdOpacity::default());
}

/// Adjust only background ink; callers retain their text and animation painter.
pub(super) fn draw_scrim_with_opacity(p: Painter, opacity: OsdOpacity) {
    p.rect(
        Rect::new(0.0, SCR_H - SCRIM_H, SCR_W, SCRIM_H),
        0.0,
        theme::scrim_black(0.0),
        theme::scrim_black(opacity.background_alpha(0.86)),
        0.0,
    );
}

/// The line over a transport's display title. Non-owning `*const c_char`, the `Label` rule
/// (`ui/CLAUDE.md`): keep the `CString` (or the session's fixed buffer) alive across the draw.
#[derive(Clone, Copy, Debug)]
pub enum Kicker {
    /// an episode's own address and name ("S1, E1 · Pilot"): primary ink, bold — it IS the
    /// episode's title, with the show's name as the display title under it
    Episode(*const std::os::raw::c_char),
    /// a context label (`metadata::context_label`: an extra's kind, a movie's route ctxline):
    /// secondary ink, regular — it only says what the display title is
    Context(*const std::os::raw::c_char),
}

/// The trailer's unbounded title block (it has no player control row).
pub fn draw_title(p: Painter, kicker: Kicker, title: *const std::os::raw::c_char) {
    let (text, ink, bold) = match kicker {
        Kicker::Episode(text) => (text, theme::TEXT_PRIMARY, 1),
        Kicker::Context(text) => (text, theme::TEXT_SECONDARY, 0),
    };
    p.text(text, SB_X, SCR_H - 312.0, theme::size::CAPTION, ink, 0, bold);
    p.text(title, SB_X, SCR_H - 278.0, HUD_TITLE_SZ, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY), 0, 1);
}

/// The player's title is always contained before the control row. While playing it is
/// ellipsised; while paused the full line glides once to reveal its end, then rests.
pub(super) fn draw_player_title(
    p: Painter,
    kicker: Kicker,
    title: *const std::os::raw::c_char,
    right: f32,
    measure: &dyn Measure,
    paused: bool,
    clock: &plx_ui::marquee::Clock,
) {
    let width = (right - SB_X).max(0.0);
    let _clip = plx_ui::screen::ClipScope::open_in(p, Rect::new(SB_X, 0.0, width, SCR_H));
    let (text, ink, bold) = match kicker {
        Kicker::Episode(text) => (text, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY), true),
        Kicker::Context(text) => (text, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY), false),
    };
    if !text.is_null() {
        let label = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
        let fitted = measure.fit_line(&label, width, theme::size::CAPTION, bold);
        p.text(fitted.as_ptr(), SB_X, SCR_H - 312.0, theme::size::CAPTION, ink, 0, i32::from(bold));
    }
    if title.is_null() {
        clock.release();
        return;
    }
    let full = unsafe { std::ffi::CStr::from_ptr(title) };
    let label = full.to_string_lossy();
    let full_width = measure.width(full, HUD_TITLE_SZ, true);
    if paused && full_width > width {
        // A show's display title can stay the same across episodes; the kicker is
        // part of the visible identity and must restart the reveal when it changes.
        let kicker = if text.is_null() { "" } else { unsafe { std::ffi::CStr::from_ptr(text) }.to_str().unwrap_or("") };
        let key = format!("{kicker}\u{1}{label}");
        let elapsed = clock.read(&key) as f32;
        let offset = plx_ui::marquee::TITLE.reveal_once(elapsed, full_width, width);
        if offset < full_width - width {
            if elapsed < plx_ui::marquee::TITLE.hold_ms {
                plx_machine::idle::wake();
            } else {
                plx_machine::idle::invalidate();
            }
        }
        if offset > 0.0 {
            // an Arabic or Hebrew title's end is on its left: it moves right instead
            let rtl = plx_gfx::text::is_rtl_paragraph(&label);
            let x = plx_ui::marquee::TITLE.reveal_once_at(elapsed, full_width, width, rtl);
            p.text(title, SB_X + x, SCR_H - 278.0, HUD_TITLE_SZ, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY), 0, 1);
            return;
        }
    } else {
        clock.release();
    }
    let fitted = measure.fit_line(&label, width, HUD_TITLE_SZ, true);
    p.text(fitted.as_ptr(), SB_X, SCR_H - 278.0, HUD_TITLE_SZ, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY), 0, 1);
}

/// Reserve the same geometry used by focus placement, plus focus pop and a reading gap.
/// Use at least the settled focus scale so moving focus does not make the title breathe; a
/// spring overshoot still reserves its actual drawn extent. Stand-ins are measured, not guessed.
pub(crate) fn title_right(slot: ControlSlot, row: &HudMotion, measure: &dyn Measure) -> f32 {
    let mut left = CTRL_RIGHT;
    for i in 0..slot.items() {
        if let Some(rect) = slot.item_rect(row, i, measure) {
            left = left.min(rect.scaled(row.scale(i).max(plx_ui::widgets::CTRL_FOCUS_SCALE)).x);
        }
    }
    if matches!(slot, ControlSlot::UpNext(_)) {
        let layout = crate::up_next::layout_peek(row, measure);
        left = left.min(layout.still.x).min(layout.caption.x);
    }
    (left - theme::space::MD).max(SB_X)
}

/// How the playhead is drawn — see [`draw_playbar`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Knob {
    /// the scrubber holds the focus ring: a glowing knob
    Focused,
    /// a scrub preview is being dragged with the ring elsewhere: a plain knob
    Held,
    /// neither: a thin tick
    Tick,
}

/// Everything [`draw_playbar`] draws from, resolved by its caller. A value rather than a read of
/// the transport's globals inside the draw, because TWO surfaces draw this bar and they resolve it
/// differently: the player route from its own scrub gesture, seek target and focus ring
/// ([`Playbar::live`]), and the detail page's full-trailer transport from a preview session that
/// has none of the three.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Playbar {
    /// the DISPLAYED playhead (a seek target, a scrub preview or the published position)
    pub pos_ns: i64,
    pub dur_ns: i64,
    pub knob: Knob,
    /// the state read-out beside the elapsed clock, from [`transport_mark`]
    pub mark: TransportMark,
    /// drives the inline spinner's rotation
    pub now: u32,
    /// how far the knob clock has grown, 0 (resting, [`theme::size::CAPTION`]) to 1
    /// ([`crate::scrub_preview::CLOCK_GROWN_SZ`]), while a scrub shows no card
    /// ([`crate::scrub_preview::Motion::clock_grow`]). The two sizes are CROSSFADED on it
    /// ([`crate::scrub_preview::clock_faces`]); a size is never tweened.
    pub clock_grow: f32,
}

impl Playbar {
    /// The player route's bar: the live scrub preview and seek target over the published
    /// playhead, the knob from the HUD's own focus ring, and the four-state read-out.
    fn live(
        model: &HudModel,
        busy: Busy,
        focus: GroupId,
        since_play_ms: Option<u32>,
        now: u32,
    ) -> Self {
        // The scrub preview, the seek target and the published playhead are ONE sample each, taken
        // with the frame's model: the state read-out takes its travel direction from the difference
        // between the displayed position and the PUBLISHED one, and two loads of a live atomic can
        // straddle a tick.
        let scrub = model.scrub_ns;
        // while a seek is loading, freeze the playhead at the target (no wobble through the reopen);
        // else follow the live scrub preview, else the real playhead.
        let livepos = model.playpos_ns;
        let dispos = display_pos_ns(model.loading, model.seek_display_ns, scrub, livepos);
        let knob = if focus == GROUP_SCRUB {
            Knob::Focused
        } else if scrub >= 0 {
            Knob::Held
        } else {
            Knob::Tick
        };
        // Gated on `busy`, not on `loading()`: with `loading()` the transport lit the same spinner
        // the centred read-out was already showing, for the whole of every load AND every seek.
        let paused = model.paused;
        Self {
            pos_ns: dispos,
            dur_ns: model.duration_ns,
            knob,
            mark: transport_mark(paused, busy, scrub >= 0, dispos, livepos, since_play_ms),
            now,
            clock_grow: 0.0,
        }
    }
}

/// The playbar: scrubber, playhead knob, elapsed/remaining clocks and the state read-out. The
/// player HUD and the detail page's full-trailer transport both draw it, so the two cannot drift.
pub fn draw_playbar(p: Painter, bar: Playbar, measure: &dyn plx_machine::machine::Measure) {
    draw_playbar_at(p, bar, measure, Rect::new(SB_X, SB_Y, sb_w(), SB_H), None);
}

fn draw_playbar_track(p: Painter, bar: Playbar, rail: Rect) {
    let white = plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY);
    let track = theme::RAIL_TRACK;
    let (sx, sw, sy, sh) = (rail.x, rail.w, rail.y, rail.h);
    let dispos = bar.pos_ns;
    let dur = bar.dur_ns;
    let frac = if dur > 0 {
        (dispos as f64 / dur as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.rect(Rect::new(sx, sy, sw, sh), sh * 0.5, track, track, 0.0);
    let fw = (sw as f64 * frac) as f32;
    if fw > sh * 0.5 {
        p.rrect(Rect::new(sx, sy, fw, sh), sh * 0.5, 0.0, white);
    } else if fw > 0.0 {
        p.rrect(Rect::new(sx, sy, fw, sh), fw * 0.5, 0.0, white);
    }
    // playhead: a focus-glowing knob when the scrubber is focused, a plain knob while scrubbing,
    // else a thin tick.
    let hx = sx + fw;
    let cy = sy + sh * 0.5;
    if bar.knob == Knob::Focused {
        let glow = [1.0f32, 1.0, 1.0, 0.22];
        p.rect(
            Rect::new(hx - KNOB_GLOW_R, cy - KNOB_GLOW_R, 2.0 * KNOB_GLOW_R, 2.0 * KNOB_GLOW_R),
            KNOB_GLOW_R,
            glow,
            glow,
            0.0,
        );
        p.rect(
            Rect::new(hx - 11.0, cy - 11.0, 22.0, 22.0),
            11.0,
            white,
            white,
            0.0,
        );
    } else if bar.knob == Knob::Held {
        p.rect(
            Rect::new(hx - 9.0, cy - 9.0, 18.0, 18.0),
            9.0,
            white,
            white,
            0.0,
        );
    } else {
        p.rect(
            Rect::new(hx - 1.5, sy - 4.0, 3.0, sh + 8.0),
            0.0,
            white,
            white,
            0.0,
        );
    }

}

fn draw_playbar_at(p: Painter, bar: Playbar, measure: &dyn Measure, rail: Rect, clocks: Option<Rect>) {
    draw_playbar_track(p, bar, rail);
    let e = hud_env();
    let white = plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY);
    let dim = plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY);
    let (sx, sw, sy) = (rail.x, rail.w, rail.y);
    let dispos = bar.pos_ns;
    let dur = bar.dur_ns;
    let now = bar.now;
    let frac = if dur > 0 { (dispos as f64 / dur as f64).clamp(0.0, 1.0) } else { 0.0 };

    // elapsed under the playhead (':' centered on the knob, clamped to the bar); remaining at the
    // right — hidden once the moving elapsed label would overlap it (near the end of the movie).
    let clock_frame = clocks.map(|mut frame| {
        // Rewind (and a right-edge fallback mark) sits left of the clamped
        // elapsed label. Keep its 30px glyph + 14px gap inside this lane.
        let inset = 48.0f32.min(frame.w);
        frame.x += inset; frame.w -= inset; frame
    }).unwrap_or(Rect::new(sx, sy + 30.0, sw, 0.0));
    let (sx, sw, ty) = (clock_frame.x, clock_frame.w, clock_frame.y);
    let hx = sx + (sw as f64 * frac) as f32;
    // A grown knob clock keeps its BASELINE, so it grows upward and never into the remaining clock's
    // line. It is a CROSSFADE of two ladder rungs (the resting CAPTION and the one above it), each
    // laid out at its own size, never a tweened size or a scaled texture; the extents the rest of
    // the line clears are the union of the faces drawn.
    let elapsed = fmt_time(dispos, false);
    let (mut el_l, mut el_r) = (f32::INFINITY, f32::NEG_INFINITY);
    for (sz, face_a) in crate::scrub_preview::clock_faces(theme::size::CAPTION, bar.clock_grow).into_iter().flatten() {
        let ety = plx_ui::typography::baseline_y(sz, 1, theme::size::CAPTION, 1, ty);
        let (l, r) = draw_clock(p.alpha(face_a), &elapsed, hx, ety, sz, white, sx, sx + sw, measure);
        el_l = el_l.min(l);
        el_r = el_r.max(r);
    }
    let rem = fmt_time(dur - dispos, true);
    // MEASURE the label, never estimate it. `chars * CAPTION * 0.52` was an Arial-calibrated
    // constant guarding a real behaviour — the remaining clock hides before the moving elapsed
    // clock can reach it — and it only ever worked because it over-estimated: under Arial it
    // returned ~99.8px against a true ~85px. Under the shipped Inter that margin had already
    // halved, and freezing tabular figures (which the clock's own template idiom requires, see
    // `draw_clock`) widens numerals further, leaving about a pixel of slack. At that point the
    // guard stops guarding and the two clocks can overlap near the end of a long item.
    // Same '0'-template `draw_clock` uses, so the two agree by construction: with tabular figures
    // the template's width IS the real string's width at every tick.
    let rem_tmpl: String = rem
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    let rem_w = measure.width_str(&rem_tmpl, theme::size::CAPTION, true);
    let rem_l = sx + sw - rem_w;
    let rem_shown = el_r + 20.0 < rem_l;
    if rem_shown {
        if let Ok(cs) = CString::new(rem.as_str()) {
            p.text(cs.as_ptr(), sx + sw, ty, theme::size::CAPTION, dim, 2, 0);
        }
    }
    // transport state indicator just past the elapsed clock — the four-state READ-OUT resolved by
    // [`transport_mark`] (rewind / pause / fast-forward / play, plus the narrowed spinner), and
    // NOTHING while playing steadily. A read-out, not an action toggle: nothing here is focusable
    // or hit-tested, and the control row stays the three discs it has always been.
    // Gated on `busy`, not on `loading()`: with `loading()` the transport lit the same spinner the
    // centred read-out was already showing, for the whole of every load AND every seek. Centered on
    // the clock's line box; drops to the clock's LEFT when the right side is against the remaining
    // label / screen edge.
    let mark = bar.mark;
    if mark != TransportMark::None {
        // pause bars under-fill their viewBox (14/24 tall) — a 30px box renders ~17px of ink,
        // matching the CAPTION clock's cap height so the glyph reads as the label's size. The two
        // travel marks are drawn to that SAME 14-unit band (see `Icon::Rewind`), which is what lets
        // one box serve the whole family without the slot changing weight as the state flips.
        let isz = 30.0f32;
        // The odd member of the family — `play.svg` is authored to 16 units where the other three
        // are 14 — is corrected by asking `icons::band` rather than by a constant here. See that
        // function: the metric is a property of the asset, and this was the second screen to
        // transcribe one out of an SVG by hand.
        // **The gap is measured to the INK, not to the box.** Every member of this family carries a
        // different left bearing inside its 24-unit viewBox — pause's bars open at x=7, play's
        // triangle at x=6, the travel marks at x=2.6 — so ONE box origin gives each state a
        // visibly different gap after the clock, and the slot appears to twitch as the state flips.
        // Measuring to the ink makes the gap the eye sees a single number. It is also what the old
        // spacing really was: a box placed 14px out put pause's ink at 14 + 7/24*30 ~= 23px, which
        // is the gap that read as too wide. The bearings come from `icons::ink_x`, not from a table
        // here — they are the asset's, and this screen was the second place to copy them out.
        const GAP: f32 = 14.0;
        let glyph = match mark {
            TransportMark::Pause => Some(plx_ui::icons::Icon::Pause),
            TransportMark::Play => Some(plx_ui::icons::Icon::Play),
            TransportMark::Rewind => Some(plx_ui::icons::Icon::Rewind),
            TransportMark::FastForward => Some(plx_ui::icons::Icon::FastForward),
            TransportMark::Working | TransportMark::None => None,
        };
        let ink = glyph.map_or((0.0, 1.0), plx_ui::icons::ink_x);
        let icy = ty + plx_ui::typography::text_height(theme::size::CAPTION, 1) * 0.5; // vertical center of the clock line
                                                                                // scaled so every member of the family lands the SAME height of ink in this one box
        let bs = glyph.map_or(isz, |g| {
            isz * plx_ui::icons::band(plx_ui::icons::Icon::Pause)
                / plx_ui::icons::band(g)
        });
        // **Rewind sits to the LEFT of the clock; everything else to the right.** The mark points
        // the way the playhead is travelling, so `<<` after the time would point back at the number
        // it is leaving. The right-hand placement still falls back to the left when the remaining
        // label or the screen edge crowds it, which is the case this branch was originally for.
        let need = (ink.1 - ink.0) * bs + 6.0;
        let room_right = el_r + GAP + need < if rem_shown { rem_l - 8.0 } else { sx + sw };
        let on_left = mark == TransportMark::Rewind || !room_right;
        // Placed by ink on whichever side it lands: the trailing edge sits GAP before the clock's
        // left, or the leading edge GAP after the clock's right.
        let bx = if on_left {
            el_l - GAP - ink.1 * bs
        } else {
            el_r + GAP - ink.0 * bs
        };
        match glyph {
            Some(id) => {
                plx_ui::icons::draw(p, id, Rect::new(bx, icy - bs * 0.5, bs, bs), white)
            }
            // placeholder-exempt: the transport's inline busy mark on a button (R_INLINE), reported by the playback pipeline, not a content stand-in
            None => Spinner::new(bx + isz * 0.5, icy, Spinner::R_INLINE)
                .phase(now)
                .tint(white)
                .draw(&e, p),
        }
    }
}

/// The guide is a read-only rail only when there is no seekable decoder
/// window. A live/DVR decoder window always retains its own painted duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailDomain {
    Decoder,
    GuideReadOnly { position_ns: i64, duration_ns: i64 },
}
impl RailDomain {
    pub fn seek_duration(self, model_duration_ns: i64) -> i64 {
        match self { Self::Decoder => model_duration_ns.max(0), Self::GuideReadOnly { .. } => 0 }
    }
    pub fn is_read_only(self) -> bool { matches!(self, Self::GuideReadOnly { .. }) }
    pub fn apply(self, bar: &mut Playbar) {
        if let Self::GuideReadOnly { position_ns, duration_ns } = self {
            bar.pos_ns = position_ns; bar.dur_ns = duration_ns; bar.knob = Knob::Tick;
        }
    }
}
pub fn rail_domain(mode: plx_machine::machine::UiMode, model: &HudModel,
    playing: Option<&plx_data::metadata::NowPlaying>) -> RailDomain {
    if mode == plx_machine::machine::UiMode::Classic && model.duration_ns <= 0
        && model.scrub_ns < 0 && model.seek_display_ns < 0 {
        if let Some(playing) = playing.filter(|playing| playing.ott.as_ref().is_some_and(|info| info.live.is_some())) {
            let (position_ns, duration_ns) = super::ott_footer::live_timeline(playing, model.wall_now_s).unwrap_or((0, 0));
            return RailDomain::GuideReadOnly { position_ns, duration_ns };
        }
    }
    RailDomain::Decoder
}

/// The compact Classic footer. The same value places paint, input stops and
/// caption clearance; no geometry is recovered from a painted widget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OttClassicLayout {
    pub geometry: HudGeometry,
    pub panel: Rect,
    pub heading: Rect,
    pub programme_rows: [Rect; 2],
    pub art: Rect,
    pub transport_offset: (f32, f32),
}

fn classic_row_heights(measure: &dyn Measure, contextual_controls: bool) -> [f32; 4] {
    use plx_ui::text_view::TextView;
    let line_h = |size| TextView::new("", size, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT)).with_measure(measure).line_h();
    let heading = line_h(theme::size::HEADLINE);
    [if contextual_controls { heading.max(BTN_S * plx_ui::widgets::CTRL_FOCUS_SCALE) } else { heading },
        SB_H, line_h(theme::size::BODY), line_h(theme::size::BODY)]
}

fn classic_panel_height(playing: Option<&plx_data::metadata::NowPlaying>, measure: &dyn Measure,
    contextual_controls: bool) -> f32 {
    let heights = classic_row_heights(measure, contextual_controls);
    let content = plx_ui::vstack::VStack::heights(&heights, theme::space::XS,
        plx_ui::vstack::Anchor::Top(0.0)).height();
    let art = if playing.and_then(|p| p.ott.as_ref()).is_some_and(|p| p.art.is_some()) { 120.0 } else { 0.0 };
    (content.max(art) + 24.0).max(180.0)
}

/// The expanded Classic description ends at this same measured footer edge.
/// It has no transport stand-ins while the description owns input.
pub fn classic_footer_top(playing: Option<&plx_data::metadata::NowPlaying>, measure: &dyn Measure) -> f32 {
    SCR_H - classic_panel_height(playing, measure, false)
}

pub(crate) fn ott_classic_layout(slot: ControlSlot, motion: &HudMotion, _tabs: usize,
    playing: Option<&plx_data::metadata::NowPlaying>, measure: &dyn Measure) -> OttClassicLayout {
    // Same Classic channel/rail/current/next structure for every OTT source.
    // Only actual contextual Skip/Up Next offers reserve an action row; ordinary
    // transport actions belong to Menu, never invisible discs or tab stops.
    let contextual = !matches!(slot, ControlSlot::Discs(_));
    let inset = 32.0;
    let pad = 12.0;
    let gap = theme::space::XS;
    let right = SCR_W - inset;
    let art_size = if playing.and_then(|p| p.ott.as_ref()).is_some_and(|p| p.art.is_some()) { 120.0 } else { 0.0 };
    let content_x = inset + if art_size > 0.0 { art_size + theme::space::MD } else { 0.0 };
    let heights = classic_row_heights(measure, contextual);
    let height = classic_panel_height(playing, measure, contextual);
    let top = SCR_H - height;
    let stack = plx_ui::vstack::VStack::heights(&heights, gap, plx_ui::vstack::Anchor::Top(top + pad));
    let frames: [Rect; 4] = std::array::from_fn(|index| {
        let (y, h) = stack.span(index).expect("four present Classic footer rows");
        Rect::new(content_x, y, right - content_x, h)
    });
    let row_top = frames[0].y + (frames[0].h - BTN_S) * 0.5;
    let offset = (right - CTRL_RIGHT, row_top - CTRL_Y);
    let shifted = |mut rect: Rect| { rect.x += offset.0; rect.y += offset.1; rect };
    let mut geometry = geometry(slot, motion, 0, measure);
    geometry.items = if contextual { geometry.items.map(|rect| rect.map(shifted)) }
        else { [None; BTN_N as usize] };
    geometry.row_anchor = RowAnchor { right, top: row_top, h: if contextual { BTN_S } else { 0.0 } };
    let control_left = geometry.items.iter().flatten()
        .map(|rect| rect.scaled(plx_ui::widgets::CTRL_FOCUS_SCALE).x)
        .fold(right, f32::min);
    geometry.row = Rect::new(control_left, frames[0].y, right - control_left,
        if contextual { frames[0].h } else { 0.0 });
    geometry.playbar = frames[1];
    geometry.grab = Rect::new(content_x, frames[1].y - gap, right - content_x, SB_H + gap * 2.0);
    geometry.tabs = [None; HUD_TABS];
    geometry.tab_row = Rect::new(right, frames[3].y, 0.0, 0.0);
    let heading = Rect::new(content_x, frames[0].y,
        (control_left - if contextual { gap } else { 0.0 } - content_x).max(0.0), frames[0].h);
    geometry.title = heading;
    let mut panel_top = top;
    if matches!(slot, ControlSlot::UpNext(_)) {
        let offer = crate::up_next::layout_peek(motion, measure);
        panel_top = panel_top.min(shifted(offer.still).y - gap).min(shifted(offer.caption).y - gap);
    }
    geometry.band = HudBand::from_controls_top(panel_top);
    OttClassicLayout {
        geometry,
        panel: Rect::new(0.0, panel_top, SCR_W, SCR_H - panel_top),
        heading,
        programme_rows: [frames[2], frames[3]],
        art: Rect::new(inset, top + (height - art_size) * 0.5, art_size, art_size),
        transport_offset: offset,
    }
}

/// The transport HUD, composed from retui widgets through a root `Painter`.
/// `model`: the frame's snapshot of every global the draw reads ([`HudModel`]); `ps` is read only for the
/// route's strings and the scrub preview's part. `focus`: [`GROUP_SCRUB`], [`GROUP_ROW`] (the right control row) or
/// [`GROUP_TABS`] (bottom tabs). `btn` (0..1) / `tab` (0..1) are the
/// focused item within their row (only meaningful when `focus` selects that row). `now` drives the
/// loading spinner's rotation. `transport`: draw the scrubber/title/buttons/clocks; pass false for
/// Info mode, where only the scrim + bottom tabs render and the Info card fills the middle.
/// `busy`: the caller's single resolve of who owns the working signal — see [`busy_surface`]; the
/// transport draws its inline spinner only when it is [`Busy::Transport`], and the centred read-out
/// is [`draw_readout`]'s, drawn by the caller AFTER this.
pub fn draw_hud(
    ps: &plx_media::route::PlaybackSession,
    model: &HudModel,
    row: &mut HudMotion,
    up: &crate::up_next::Countdown,
    slot: ControlSlot,
    busy: Busy,
    focus: GroupId,
    btn: i32,
    tab: i32,
    now: u32,
    transport: bool,
    measure: &dyn plx_machine::machine::Measure,
    meta: plx_data::metadata::MetadataView<'_>,
) {
    draw_hud_in_mode(ps, model, row, up, slot, busy, focus, btn, tab, now,
        transport, measure, meta, plx_machine::machine::UiMode::Modern);
}

/// The same player and controls, with the active presentation sharing its
/// measured geometry between painting, focus, pointer stops and captions.
pub fn draw_hud_in_mode(
    ps: &plx_media::route::PlaybackSession,
    model: &HudModel,
    row: &mut HudMotion,
    up: &crate::up_next::Countdown,
    slot: ControlSlot,
    busy: Busy,
    focus: GroupId,
    btn: i32,
    tab: i32,
    now: u32,
    transport: bool,
    measure: &dyn plx_machine::machine::Measure,
    meta: plx_data::metadata::MetadataView<'_>,
    mode: plx_machine::machine::UiMode,
) {
    // A FAILURE owns the frame, and it outranks every branch below — including the Up Next card,
    // which cannot coexist with one but must not be the arm that decides so. `Player Screen.dc.html`
    // sets `hudDisplay:none` AND `tabsDisplay:none` on the failed variant, and its read-out block
    // says why in one line: "the transport is not drawn: with no picture and no position a
    // live-looking scrubber at 0:00 is the historical bug this read-out exists to end."
    //
    // Without this the bug was only half-fixed. `Error` is not `is_busy()`, so it never pins the
    // HUD — but it does not HIDE it either, so for the ~4.5 s of `app.rs`'s linger after entry the
    // scrubber, clocks and tabs drew live at 0:00 UNDER the read-out, and only then went away. The
    // read-out itself already outlives the linger (`draw_readout` is not transport chrome), so this
    // is the missing half of that same rule rather than a new one.
    //
    // Gated on the resolved `busy` and not on `player::state()`: [`busy_surface`] is the ONE place
    // the surfaces are divided, and re-reading the state here is exactly the second derivation its
    // doc exists to forbid. Deliberately NOT gated on `transport` — the Info card / Chapters strip
    // pass false and would otherwise put the bottom scrim + tabs back under a failure.
    if readout_owns_frame(busy) {
        row.reset_title();
        return;
    }
    if mode == plx_machine::machine::UiMode::Modern {
        draw_modern(ps, model, row, up, slot, busy, focus, btn, tab, now, transport, measure, meta);
        return;
    }
    // An expanded Classic description sits above this same footer, not in its
    // place. Contextual offers stay on the actual transport surface only.
    let visible_slot = if transport { slot } else { ControlSlot::Discs(Controls::NONE) };
    let layout = ott_classic_layout(visible_slot, row, 0, meta.now_playing(), measure);
    let p = Painter::root();
    let color = theme::with_a(plx_ui::palette::color(plx_ui::palette::Role::Background, theme::CLASSIC_BACKGROUND), model.osd_opacity.background_alpha(0.78));
    p.rect(layout.panel, 0.0, color, color, 0.0);
    let chrome = row.chrome_visible(now);
    let pc = p.alpha(row.preview.motion.chrome_alpha(now));
    row.reset_title();
    let mut bar = Playbar::live(model, busy, if transport { focus } else { GROUP_ROW }, row.since_play_ms(now), now);
    let decoder = Some((bar.pos_ns, bar.dur_ns));
    if chrome {
        if let Some(playing) = meta.now_playing() {
            super::ott_footer::draw_classic(pc, playing, playing.ott.as_ref(), &mut row.ott_clock_labels,
                &layout, measure, model.wall_now_s, decoder);
        } else {
            draw_route_title(pc, plx_media::route::title_cptr(ps),
                plx_media::route::ctxline_cptr(ps), &layout);
        }
        let controls = pc.translate(layout.transport_offset.0, layout.transport_offset.1);
        match visible_slot {
            ControlSlot::UpNext(_) => crate::up_next::draw(ps, row, up, controls,
                focus == GROUP_ROW, btn, now, measure),
            ControlSlot::Skip(prompt) => crate::skip_pill::draw(row, controls, prompt,
                focus == GROUP_ROW, measure),
            ControlSlot::Discs(_) => {},
        }
    }
    bar.clock_grow = row.preview.motion.clock_grow(now);
    bar.pos_ns = row.spring.apply(bar.pos_ns, now);
    rail_domain(mode, model, meta.now_playing()).apply(&mut bar);
    draw_playbar_track(p, bar, layout.geometry.playbar);
    if transport { draw_scrub_preview_at(p, ps, model, row, bar, layout.geometry.playbar); }

}

/// Metadata may not have arrived yet. Keep the route's real title/context,
/// clipped into the compact cells; this fallback manufactures no guide data.
fn draw_route_title(painter: Painter, title: *const std::os::raw::c_char,
    context: *const std::os::raw::c_char, layout: &OttClassicLayout) {
    use plx_ui::label::{HAlign, Label};
    {
        let _clip = plx_ui::screen::ClipScope::open_in(painter, layout.heading);
        Label::new(title, theme::size::BODY, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT))
            .h(HAlign::Start).draw(painter, layout.heading);
    }
    let _clip = plx_ui::screen::ClipScope::open_in(painter, layout.programme_rows[0]);
    Label::new(context, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY))
        .h(HAlign::Start).draw(painter, layout.programme_rows[0]);
}

/// Modern is a composition of the same controls, transport and tabs. The old
/// unlabelled Modern branch is replaced here; Classic and stand-in actions keep
/// their existing performers and hit identities.
fn draw_modern(
    ps: &plx_media::route::PlaybackSession, model: &HudModel, row: &mut HudMotion,
    up: &crate::up_next::Countdown, slot: ControlSlot, busy: Busy, focus: GroupId,
    btn: i32, tab: i32, now: u32, transport: bool, measure: &dyn Measure,
    meta: plx_data::metadata::MetadataView<'_>,
) {
    let p = Painter::root();
    let focused = (focus == GROUP_ROW).then_some(btn);
    let label = super::modern::focused_label(slot, focused);
    let tabs = if crate::chapters_panel::has_chapters(meta) { 2 } else { 1 };
    let layout = super::modern::compose(geometry(slot, row, tabs, measure), slot, row,
        focused, label, meta.now_playing(), model.wall_now_s, measure);
    if transport { super::modern::draw_panel(p, &layout, model.osd_opacity); }
    else { draw_scrim_with_opacity(p, model.osd_opacity); }
    let chrome = row.chrome_visible(now);
    let pc = p.alpha(row.preview.motion.chrome_alpha(now));
    row.reset_title();
    if transport {
        if chrome {
            super::modern::draw_information(pc, &layout, meta.now_playing(), model.wall_now_s,
                &mut row.ott_clock_labels, measure);
            match slot {
                ControlSlot::Discs(controls) => super::modern::draw_controls(pc, &layout,
                    controls, row, focused, label, measure),
                ControlSlot::Skip(prompt) => crate::skip_pill::draw(row, pc, prompt,
                    focus == GROUP_ROW, measure),
                ControlSlot::UpNext(_) => crate::up_next::draw(ps, row, up, pc,
                    focus == GROUP_ROW, btn, now, measure),
            }
        }
        if super::modern::shows_timeline(model, meta.now_playing()) {
            let mut bar = Playbar::live(model, busy, focus, row.since_play_ms(now), now);
            bar.clock_grow = row.preview.motion.clock_grow(now);
            bar.pos_ns = row.spring.apply(bar.pos_ns, now);
            draw_playbar(p, bar, measure);
            draw_scrub_preview(p, ps, model, row, bar);
        }
    }
    draw_tabs(&hud_env(), pc, tabs > 1, chrome,
        |i| hud_tab_state(transport, focus, tab, i), measure);
}

/// The bottom tab pills: ONE run from the scrubber's left edge ([`tab_row`]), the same one
/// [`tab_hit_rect`] answers from. `state(i)` is tab `i`'s `(focused, selected)`; `chrome` false
/// draws none (and measures none: a replay answers only the keys it recorded).
pub(super) fn draw_tabs(
    e: &Env,
    pc: Painter,
    has_chapters: bool,
    chrome: bool,
    state: impl Fn(i32) -> (bool, bool),
    measure: &dyn plx_machine::machine::Measure,
) {
    let tabs = tab_labels(has_chapters);
    // tabs match the transport control buttons' height (BTN_S), centred vertically between the
    // play bar (scrubber, at SB_Y) and the bottom edge of the screen
    let ph = BTN_S; // = 64, same as the Subtitles/Audio buttons
    let py = tab_y();
    let items = if chrome { tab_items(tabs, measure) } else { [Item::absent(); HUD_TABS] };
    for placed in tab_row(&items).placed() {
        let (px, pw, i) = (placed.x, placed.w, placed.i);
        let (focused, selected) = state(i as i32);
        if let Some(Ok(cs)) = tabs[i].map(CString::new) {
            TabPill::new(cs.as_ptr(), theme::size::BODY, Rect::new(px, py, pw, ph))
                .focused(focused)
                .selected(selected)
                // Same row band, same ramp, same ground as the transport discs — everything the
                // HUD draws stands on the video plane (`ControlGround`).
                .ground(ControlGround::Unkeyed)
                .draw(e, pc);
        }
    }
}

/// The bar's x for a position `pos_ns` of `dur_ns`, clamped to the bar's ends.
fn bar_x(pos_ns: i64, dur_ns: i64) -> f32 {
    SB_X + (sb_w() as f64 * (pos_ns as f64 / dur_ns.max(1) as f64).clamp(0.0, 1.0)) as f32
}

/// **The scrub preview** ([`crate::scrub_preview`]): while a live scrub preview is up, the still
/// at the knob's position, floated above the knob; after it, the same still fading out. Draws
/// nothing for a part with no BIF index, and nothing until the first still lands.
fn draw_scrub_preview(p: Painter, ps: &plx_media::route::PlaybackSession, model: &HudModel, row: &mut HudMotion, bar: Playbar) {
    draw_scrub_preview_at(p, ps, model, row, bar, Rect::new(SB_X, SB_Y, sb_w(), SB_H));
}

fn draw_scrub_preview_at(p: Painter, ps: &plx_media::route::PlaybackSession, model: &HudModel,
    row: &mut HudMotion, bar: Playbar, rail: Rect) {
    use crate::scrub_preview::{card_rect, card_size, draw, scaled_about, still_ms, still_path, Look, REQ_H, REQ_W};
    let part = plx_media::route::cur_part_id(ps);
    let sid = plx_media::route::item_sid(plx_media::route::cur_sid(ps)).raw();
    let show = if model.scrub_ns < 0 {
        row.preview.motion.exit(bar.now)
    } else if bar.dur_ns <= 0 {
        None
    } else {
        let generation = plx_media::route::sess(ps);
        let target = still_ms(bar.pos_ns);
        let look = |ms: i64| {
            let path = still_path(part, ms);
            if plx_ui::tex::resolve_on(sid, &path, REQ_W, REQ_H, false) != 0 {
                Look::Ready
            } else if plx_ui::tex::failed_on(sid, &path, REQ_W, REQ_H, false) {
                Look::Failed
            } else {
                Look::Waiting
            }
        };
        let shown = row.preview.step((part, &generation), target, bar.now, look);
        let absent = row.preview.absent();
        row.preview.motion.observe(shown, target, absent,
            rail.x + (rail.w as f64 * (bar.pos_ns as f64 / bar.dur_ns.max(1) as f64).clamp(0.0, 1.0)) as f32, bar.now)
    };
    let Some(sh) = show else { return };
    let (tex, tw, th) = plx_ui::tex::resolve_wh_on(sid, &still_path(part, sh.ms), REQ_W, REQ_H, false);
    if tex == 0 || tw <= 0.0 || th <= 0.0 {
        return;
    }
    // In-place crossfade only between two resident stills of the same aspect; otherwise the swap is
    // immediate (the card's width would have to jump mid-fade).
    let prev = sh.prev.and_then(|(old, t)| {
        let (otex, ow, oh) = plx_ui::tex::resolve_wh_on(sid, &still_path(part, old), REQ_W, REQ_H, false);
        (otex != 0 && oh > 0.0 && ((ow / oh) - (tw / th)).abs() < 0.01).then_some((otex, t))
    });
    let (w, _) = card_size(tw, th);
    let rest = card_rect(sh.knob_x, rail.x, rail.w, rail.y, w);
    let r = scaled_about(rest, sh.knob_x, rail.y + rail.h * 0.5, sh.scale);
    draw(p.alpha(sh.alpha), r, tex, tw, th, prev);
}

/// **The classic layout's slot-independent geometry**: the rects `scrub_hit_rect`, the playbar, the
/// control row, its anchor and the tab / title origins, from the constants this file draws with. It
/// measures nothing, so it is safe anywhere (`overscan_rects`, the stand-ins' layout). `items` and
/// `tabs` are left `None`; [`geometry`] fills them.
pub fn frame_geometry() -> HudGeometry {
    let row_anchor = RowAnchor { right: CTRL_RIGHT, top: CTRL_Y, h: CTRL_H };
    HudGeometry {
        grab: scrub_hit_rect(),
        playbar: Rect::new(SB_X, SB_Y, sb_w(), SB_H),
        row: ctrl_row_hit_rect(),
        row_anchor,
        items: [None; BTN_N as usize],
        tab_row: Rect::new(SB_X, tab_y(), 0.0, BTN_S),
        title: Rect::new(SB_X, SCR_H - 312.0, 0.0, 0.0),
        tabs: [None; HUD_TABS],
        // the compact row's top: the highest interactive extent, whichever slot occupies the row,
        // because the Skip and Up Next stand-ins land on `row_anchor` exactly
        band: HudBand::from_controls_top(row_anchor.top),
    }
}

/// **The whole of the classic geometry for one frame**: [`frame_geometry`] plus item `i` of whatever
/// occupies the row at the rect it is DRAWN at ([`ControlSlot::item_rect`]) and each of the `tabs`
/// bottom pills ([`tab_hit_rect`]; two tabs means the item has chapters). It MEASURES, like the
/// draw it describes (the Skip pill, the Up Next pair and the pills are as wide as their labels), so
/// it is called where the screen already measures for the same frame: `Focusable::place` and
/// the pointer stops.
pub fn geometry_in_mode(slot: ControlSlot, motion: &HudMotion, tabs: usize, measure: &dyn Measure,
    mode: plx_machine::machine::UiMode, focused: Option<i32>,
    playing: Option<&plx_data::metadata::NowPlaying>, wall_now_s: Option<i64>) -> HudGeometry {
    let geometry = geometry(slot, motion, tabs, measure);
    if mode == plx_machine::machine::UiMode::Modern {
        return super::modern::compose(geometry, slot, motion, focused,
            super::modern::focused_label(slot, focused), playing, wall_now_s, measure).geometry;
    }
    ott_classic_layout(slot, motion, tabs, playing, measure).geometry
}

pub fn geometry(slot: ControlSlot, motion: &HudMotion, tabs: usize, measure: &dyn Measure) -> HudGeometry {
    let mut g = frame_geometry();
    for i in 0..slot.items() {
        if let Some(cell) = g.items.get_mut(i as usize) {
            *cell = slot.item_rect(motion, i, measure);
        }
    }
    for i in 0..tabs.min(HUD_TABS) {
        g.tabs[i] = tab_hit_rect(i as i32, tabs > 1, measure);
    }
    g
}

#[cfg(test)]
mod ott_classic_geometry_tests {
    use super::*;
    use plx_machine::machine::{FontScale, TypographyMeasure, UiMode};
    fn same(a: Rect, b: Rect) -> bool {
        (a.x-b.x).abs()<0.01 && (a.y-b.y).abs()<0.01 && (a.w-b.w).abs()<0.01 && (a.h-b.h).abs()<0.01
    }
    #[test]
    fn classic_geometry_reserves_programmes_and_modern_keeps_transport_coordinates() {
        let slot = ControlSlot::Discs(Controls::NONE); let motion = HudMotion::new();
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let old = geometry(slot, &motion, 2, &measure);
            let modern = geometry_in_mode(slot, &motion, 2, &measure, UiMode::Modern, None, None, None);
            assert_eq!(modern.playbar, old.playbar);
            assert_eq!(modern.grab, old.grab);
            assert_eq!(modern.tabs, old.tabs);
            let layout = ott_classic_layout(slot, &motion, 2, None, &measure);
            let new = geometry_in_mode(slot, &motion, 2, &measure, UiMode::Classic, None, None, None);
            assert_eq!(new, layout.geometry);
            assert!(layout.heading.y >= 0.0);
            assert!(layout.heading.y + layout.heading.h < new.playbar.y);
            assert!(new.playbar.y + new.playbar.h <= layout.programme_rows[0].y);
            assert!(new.grab.y + new.grab.h <= layout.programme_rows[0].y);
            assert!(layout.programme_rows[0].y + layout.programme_rows[0].h <= layout.programme_rows[1].y);
            assert!(new.tabs.iter().all(Option::is_none));
            assert_eq!(layout.programme_rows[1].x + layout.programme_rows[1].w, new.playbar.x + new.playbar.w);
            assert!(new.tab_row.y + new.tab_row.h <= SCR_H);
            assert_eq!(new.scrub_frac(new.playbar.x), 0.0);
            assert_eq!(new.scrub_frac(new.playbar.cx()), 0.5);
            assert_eq!(new.scrub_frac(new.playbar.x + new.playbar.w), 1.0);
            assert!(new.band().ceil_y < layout.heading.y);
            if scale == 100 { assert!((180.0..=220.0).contains(&layout.panel.h), "{}", layout.panel.h); }
        }
    }

    #[test]
    fn compact_classic_has_no_old_full_height_scrim_while_transport_is_visible() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut motion = HudMotion::new();
        let slot = ControlSlot::Discs(Controls::NONE);
        let layout = ott_classic_layout(slot, &motion, 1, None, &plx_ui::fixture::FixtureMeasure);
        let paint = plx_ui::draw_census::capture(|| plx_ui::record_walk(|| draw_hud_in_mode(
            plx_media::route::idle_session_for_test(), &HudModel::default(), &mut motion,
            &crate::up_next::Countdown::default(), slot, Busy::None, GROUP_ROW, BTN_MORE, 0, 1_000,
            true, &plx_ui::fixture::FixtureMeasure, store.view(), UiMode::Classic)));
        assert!(paint.iter().any(|(_, r)| same(*r, layout.panel)));
        assert!(!paint.iter().any(|(_, r)| same(*r, Rect::new(0.0, SCR_H-SCRIM_H, SCR_W, SCRIM_H))));
    }

    #[test]
    fn compact_classic_keeps_same_footer_under_expanded_info_without_old_scrim() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut motion = HudMotion::new();
        let paint = plx_ui::draw_census::capture(|| plx_ui::record_walk(|| draw_hud_in_mode(
            plx_media::route::idle_session_for_test(), &HudModel::default(), &mut motion,
            &crate::up_next::Countdown::default(), ControlSlot::Discs(Controls::NONE), Busy::None,
            GROUP_TABS, BTN_MORE, 0, 1_000, false, &plx_ui::fixture::FixtureMeasure,
            store.view(), UiMode::Classic)));
        assert!(!paint.iter().any(|(_, r)| same(*r, Rect::new(0.0, SCR_H-SCRIM_H, SCR_W, SCRIM_H))));
        let top = classic_footer_top(None, &plx_ui::fixture::FixtureMeasure);
        assert!(paint.iter().any(|(_, r)| same(*r, Rect::new(0.0, top, SCR_W, SCR_H - top))));
    }

    #[test]
    fn compact_route_fallback_keeps_real_title_and_context() {
        let _guard = plx_base::testlock::serial();
        let layout = ott_classic_layout(ControlSlot::Discs(Controls::NONE),
            &HudMotion::new(), 1, None, &plx_ui::fixture::FixtureMeasure);
        let runs = plx_ui::draw_census::capture_runs(|| draw_route_title(Painter::recording(),
            c"Accepted route title".as_ptr(), c"Accepted route context".as_ptr(), &layout));
        assert!(runs.iter().any(|run| run.text == "Accepted route title"));
        assert!(runs.iter().any(|run| run.text == "Accepted route context"));
    }

    #[test]
    fn shared_playbar_rewind_mark_at_zero_stays_inside_its_clock_lane() {
        let _guard = plx_base::testlock::serial();
        let model = HudModel { duration_ns: 120_000_000_000, ..HudModel::default() };
        let rail = frame_geometry().playbar;
        let clocks = Rect::new(SCR_W - 460.0, rail.y + 32.0, 380.0, 60.0);
        let mut bar = Playbar::live(&model, Busy::None, GROUP_ROW, Some(0), 1_000);
        bar.mark = TransportMark::Rewind;
        bar.knob = Knob::Tick;
        let icons = plx_ui::draw_census::capture_icons(|| draw_playbar_at(Painter::recording(),
            bar, &plx_ui::fixture::FixtureMeasure, rail, Some(clocks)));
        let (_, mark) = icons.iter().find(|(icon, _)| *icon == plx_ui::icons::Icon::Rewind)
            .expect("the actual rewind mark must remain visible");
        assert!(mark.x >= clocks.x);
        assert!(mark.x + mark.w <= clocks.x + clocks.w);
    }

    #[test]
    fn compact_standins_keep_item_identities_and_clear_their_full_artwork() {
        use plx_data::metadata::{Marker, MarkerKind};
        let marker = Marker { kind: MarkerKind::Credits, start_ms: 1_000, end_ms: 2_000, final_seg: true };
        let slots = [ControlSlot::Skip(crate::skip_pill::prompt_for(marker)), ControlSlot::UpNext(marker)];
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            for slot in slots {
                let motion = HudMotion::new();
                let layout = ott_classic_layout(slot, &motion, 2, None, &measure);
                for i in 0..slot.items() {
                    let old = slot.item_rect(&motion, i, &measure);
                    let translated = old.map(|mut rect| {
                        rect.x += layout.transport_offset.0; rect.y += layout.transport_offset.1; rect
                    });
                    assert_eq!(layout.geometry.items[i as usize], translated);
                    if slot.enabled(i) { assert!(translated.is_some()); }
                }
                let items: Vec<_> = layout.geometry.items.iter().flatten().collect();
                for pair in items.windows(2) { assert!(pair[0].x + pair[0].w <= pair[1].x); }
                assert!(layout.geometry.row.y + layout.geometry.row.h < layout.geometry.playbar.y);
                if matches!(slot, ControlSlot::UpNext(_)) {
                    let offer = crate::up_next::layout_peek(&motion, &measure);
                    for rect in [offer.still, offer.caption] {
                        assert!(layout.geometry.band().ceil_y < rect.y + layout.transport_offset.1);
                        assert!(layout.panel.y <= rect.y + layout.transport_offset.1);
                    }
                }
            }
        }
    }

    #[test]
    fn both_huds_paint_rail_and_only_their_advertised_controls_at_reported_geometry() {
        let _guard = plx_base::testlock::serial();
        let slot = ControlSlot::Discs(Controls::NONE);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let up = crate::up_next::Countdown::default();
        let model = HudModel { playpos_ns: 20_000_000_000, duration_ns: 100_000_000_000, ..HudModel::default() };
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            for mode in [UiMode::Classic, UiMode::Modern] {
                let mut motion = HudMotion::new();
                let expected = geometry_in_mode(slot, &motion, 1, &measure, mode, None, None, None);
                let paint = plx_ui::draw_census::capture(|| plx_ui::record_walk(|| draw_hud_in_mode(
                    plx_media::route::idle_session_for_test(), &model, &mut motion, &up, slot, Busy::None,
                    GROUP_SCRUB, BTN_SUBTITLES, 0, 1_000, true, &measure, store.view(), mode)));
                assert!(paint.iter().any(|(_, rect)| same(*rect, expected.playbar)), "rail not painted at reported geometry {mode:?}/{scale}");
                for (i, rect) in expected.items.iter().enumerate() {
                    if let Some(rect) = rect.filter(|_| slot.enabled(i as i32)) {
                        assert!(paint.iter().any(|(_, painted)| same(*painted, rect)), "control {i} paint/hit mismatch {mode:?}/{scale}");
                    }
                }
                if mode == UiMode::Classic {
                    assert!(expected.items.iter().all(Option::is_none));
                    assert!(expected.tabs.iter().all(Option::is_none));
                }
                for rect in expected.tabs.iter().flatten() {
                    assert!(paint.iter().any(|(_, drawn)| (drawn.cx() - rect.cx()).abs() < 0.01
                        && (drawn.cy() - rect.cy()).abs() < 0.01 && (drawn.w - rect.w).abs() < 0.01
                        && drawn.h >= rect.h && drawn.h <= rect.h * 1.25), "tab paint/hit mismatch {mode:?}/{scale}");
                }
            }
        }
    }
    #[test]
    fn classic_regular_footer_has_no_invisible_native_controls_or_tabs() {
        let motion = HudMotion::new();
        let slot = ControlSlot::Discs(Controls { previous: true, next: true, restart: false });
        let measure = plx_ui::fixture::FixtureMeasure;
        let classic = geometry_in_mode(slot, &motion, 2, &measure, UiMode::Classic, None, None, None);
        assert!(classic.items.iter().all(Option::is_none));
        assert!(classic.tabs.iter().all(Option::is_none));
        assert_eq!(classic.row.w, 0.0);
        assert_eq!(classic.tab_row.w, 0.0);
        let modern = geometry_in_mode(slot, &motion, 2, &measure, UiMode::Modern, None, None, None);
        assert!(modern.items.iter().all(Option::is_some));
        assert!(modern.tabs.iter().all(Option::is_some));
    }

    #[test]
    fn classic_paint_omits_native_discs_while_modern_keeps_them() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        let controls = Controls::NONE;
        for mode in [UiMode::Classic, UiMode::Modern] {
            let mut motion = HudMotion::new();
            let icons = plx_ui::draw_census::capture_icons(|| plx_ui::record_walk(|| draw_hud_in_mode(
                plx_media::route::idle_session_for_test(), &HudModel::default(), &mut motion,
                &crate::up_next::Countdown::default(), ControlSlot::Discs(controls), Busy::None,
                GROUP_SCRUB, BTN_MORE, 0, 1_000, true, &plx_ui::fixture::FixtureMeasure, store.view(), mode)));
            for index in [BTN_SUBTITLES, BTN_AUDIO, BTN_MORE] {
                assert_eq!(icons.iter().any(|(icon, _)| *icon == controls.icon(index)),
                    mode == UiMode::Modern, "{mode:?} control {index}: {icons:?}");
            }
        }
    }

}

//! Passive multi-view chrome and semantic selection events. The owner supplies
//! an admitted scene and decides whether this overlay owns input. Nothing here
//! opens media, changes audio, stores configuration, or enables multi-view.

use crate::multiview_layout::{GeometryError, Layout, PointerAction, PointerMap, RasterPlan, SlotId};
use crate::player_hud::{model::OsdOpacity, pending::PendingFooter};
use plx_data::metadata::NowPlaying;
use plx_data::ott::multiview::{MediaRef, SourceRef};
use plx_machine::machine::{Measure, UiMode};
use plx_ui::{screen::Dir, Painter, Rect, Zoom};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState { Empty, Loading, Playing, Failed }

/// Closed provider outcomes suitable for display on a shared TV screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderNotice { SessionDenied, Interrupted, RefreshTimeout, Unavailable }

impl ProviderNotice {
    pub fn from_stop(stop: Option<&str>) -> Option<Self> {
        match stop {
            Some("provider_session_denied") => Some(Self::SessionDenied),
            Some("provider_interrupted" | "provider_lease_expired") => Some(Self::Interrupted),
            Some("source_temporarily_unavailable") => Some(Self::Unavailable),
            Some("provider_refresh_timeout") => Some(Self::RefreshTimeout),
            _ => None,
        }
    }
    fn message(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Self::SessionDenied => msg::multiview_provider_session_denied(),
            Self::Interrupted => msg::multiview_provider_interrupted(),
            Self::Unavailable => msg::multiview_failed(),
            Self::RefreshTimeout => msg::multiview_provider_refresh_timeout(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotView {
    pub id: SlotId,
    pub title: String,
    pub state: SlotState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewState {
    /// Unique issuance serial for this UI publication, independent of geometry.
    pub revision: u64,
    /// Revision of the backend-admitted pixel rectangle receipt.
    pub geometry_revision: u64,
    pub active: Option<SlotId>,
    pub audio: Option<SlotId>,
    pub focused: Option<SlotId>,
    /// Explicit input ownership, not inferred from a visible video or slot count.
    pub input_owned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input { Direction(Dir), Confirm, Other }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    FocusSlot { revision: u64, slot: SlotId },
    ActivateSlot { revision: u64, slot: SlotId },
    OpenWindowChannels { revision: u64, slot: SlotId },
    OpenWindowMenu { revision: u64, slot: SlotId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeError { Geometry(GeometryError), InvalidSlot, DuplicateSlot, InvalidRevision, TitleTooLong }

#[derive(Clone, Debug, PartialEq)]
pub struct SlotChrome {
    pub id: SlotId,
    pub bounds: Rect,
    pub title: String,
    pub state: SlotState,
    pub active: bool,
    pub audio: bool,
    pub focused: bool,
    footer: Option<Arc<NowPlaying>>,
    notice: Option<ProviderNotice>,
    identity: Option<(SourceRef, MediaRef)>,
    decoder: Option<(i64, i64)>,
    wall_now_s: Option<i64>,

}

#[derive(Clone, Debug, PartialEq)]
pub struct ChromeFrame {
    layout: Layout,
    state: ViewState,
    slots: Vec<SlotChrome>,
    window_actions: bool,
}

impl ChromeFrame {
    pub fn from_raster(plan: &RasterPlan, views: &[SlotView], state: ViewState, chrome_scale: f32)
        -> Result<Self, ChromeError>
    {
        if plan.revision() != state.geometry_revision { return Err(ChromeError::InvalidRevision); }
        Self::new(Layout::from_raster(plan, chrome_scale).map_err(ChromeError::Geometry)?, views, state)
    }

    pub fn from_raster_in(plan: &RasterPlan, views: &[SlotView], state: ViewState,
        chrome_scale: f32, surface: Rect) -> Result<Self, ChromeError>
    {
        if plan.revision() != state.geometry_revision { return Err(ChromeError::InvalidRevision); }
        let layout = Layout::from_raster(plan, chrome_scale).and_then(|layout| layout.project_contain(surface))
            .map_err(ChromeError::Geometry)?;
        Self::new(layout, views, state)
    }

    pub fn new(layout: Layout, views: &[SlotView], state: ViewState) -> Result<Self, ChromeError> {
        for id in [state.active, state.audio, state.focused].into_iter().flatten() {
            if layout.slot(id).is_none() { return Err(ChromeError::InvalidSlot); }
        }
        let mut seen = [false; 9];
        for view in views {
            if layout.slot(view.id).is_none() { return Err(ChromeError::InvalidSlot); }
            if std::mem::replace(&mut seen[usize::from(view.id)], true) { return Err(ChromeError::DuplicateSlot); }
            if view.title.len() > 4096 { return Err(ChromeError::TitleTooLong); }
        }
        let slots = layout.slots().iter().map(|slot| {
            let view = views.iter().find(|view| view.id == slot.id);
            SlotChrome {
                id: slot.id,
                bounds: slot.bounds,
                title: view.map(|view| view.title.chars().filter(|c| !c.is_control()).take(256).collect()).unwrap_or_default(),
                state: view.map_or(SlotState::Empty, |view| view.state),
                active: state.active == Some(slot.id),
                audio: state.audio == Some(slot.id),
                focused: state.focused == Some(slot.id),
                footer: None, notice: None, identity: None, decoder: None, wall_now_s: None,
            }
        }).collect();
        Ok(Self { layout, state, slots, window_actions: false })
    }

    pub fn slots(&self) -> &[SlotChrome] { &self.slots }
    pub fn layout(&self) -> &Layout { &self.layout }
    pub fn state(&self) -> ViewState { self.state }
    pub fn enable_window_actions(&mut self) { self.window_actions = true; }
    pub fn window_actions(&self) -> bool { self.window_actions && self.state.input_owned }

    pub fn window_menu(&self) -> Option<Event> {
        if !self.window_actions() { return None; }
        self.state.active.map(|slot| Event::OpenWindowMenu { revision: self.state.revision, slot })
    }

    pub fn pointer(&self, map: &PointerMap, action: PointerAction, x: f32, y: f32) -> Option<Event> {
        if !self.state.input_owned { return None; }
        let slot = self.layout.activate_pointer(map, action, x, y)?;
        let revision = self.state.revision;
        if !self.window_actions() { return Some(Event::ActivateSlot { revision, slot }); }
        let (_, y) = map.to_logical(x, y)?;
        let bounds = self.layout.slot(slot)?.bounds;
        Some(if y < bounds.y + bounds.h * 0.2 {
            Event::OpenWindowMenu { revision, slot }
        } else {
            Event::OpenWindowChannels { revision, slot }
        })
    }

    /// These are semantic overlay inputs, not a global remote-key hook. Ordinary
    /// transport keys remain the active player's responsibility outside this widget.
    pub fn input(&self, input: Input) -> Option<Event> {
        if !self.state.input_owned { return None; }
        let from = self.state.focused.or(self.state.active).unwrap_or(0);
        match input {
            Input::Direction(direction) => self.layout.neighbor(from, direction)
                .map(|slot| Event::FocusSlot { revision: self.state.revision, slot }),
            Input::Confirm => self.state.focused.or(self.state.active)
                .map(|slot| {
                    let revision = self.state.revision;
                    if self.window_actions() { Event::OpenWindowChannels { revision, slot } }
                    else { Event::ActivateSlot { revision, slot } }
                }),
            Input::Other => None,
        }
    }

    pub fn set_provider_notice(&mut self, id: SlotId, notice: ProviderNotice) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id && slot.state != SlotState::Playing) {
            slot.notice = Some(notice);
        }
    }

    /// Attach only the target slot's verified catalogue/source facts.
    pub fn set_footer(&mut self, id: SlotId, playing: NowPlaying) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id && slot.state != SlotState::Empty) {
            slot.footer = Some(Arc::new(playing));
        }
    }
    pub fn set_footer_identity(&mut self, id: SlotId, source: SourceRef, media: MediaRef) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id) {
            slot.identity = Some((source, media));
        }
    }

    pub fn set_footer_clock(&mut self, id: SlotId, decoder: Option<(i64, i64)>, wall_now_s: Option<i64>) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id) {
            slot.decoder = decoder.filter(|(position, duration)| *position >= 0 && *duration > 0);
            slot.wall_now_s = wall_now_s;
        }
    }
}

#[derive(Default)]
struct FooterSlot {
    source: Option<Arc<NowPlaying>>,
    notice: Option<ProviderNotice>,
    identity: Option<(SourceRef, MediaRef)>,
    shown_at: Option<u32>,
    renderer: Option<Box<PendingFooter>>,
}

/// State travels with the player screen. Routine status and EPG publications
/// refresh their content without extending the user's normal OSD timeout.
#[derive(Default)]
pub struct WindowFooters {
    slots: [FooterSlot; 9],
    focused: Option<SlotId>,
    active: Option<SlotId>,
}

impl WindowFooters {
    pub fn reveal(&mut self, id: SlotId, now: u32) {
        if let Some(slot) = self.slots.get_mut(usize::from(id)) { slot.shown_at = Some(now); }
    }
    pub fn dismiss(&mut self) {
        for slot in &mut self.slots { slot.shown_at = None; }
    }

    pub fn visible_mask(&self, now: u32, timeout_ms: u32) -> u16 {
        self.slots.iter().enumerate().fold(0, |mask, (slot, state)| {
            if (state.source.is_some() || state.notice.is_some()) && state.shown_at.is_some_and(|at| now.wrapping_sub(at) < timeout_ms) {
                mask | (1 << slot)
            } else { mask }
        })
    }

    pub fn draw(&mut self, frame: &ChromeFrame, painter: Painter, measure: &dyn Measure,
        mode: UiMode, opacity: OsdOpacity, wall_now_s: Option<i64>, now: u32, timeout_ms: u32) {
        if self.active != frame.state.active {
            if let Some(id) = frame.state.active { self.reveal(id, now); }
            self.active = frame.state.active;
        }
        if self.focused != frame.state.focused {
            if let Some(id) = frame.state.focused { self.reveal(id, now); }
            self.focused = frame.state.focused;
        }
        for (index, slot) in frame.slots.iter().enumerate() {
            let state = &mut self.slots[usize::from(slot.id)];
            if slot.notice.is_none() && slot.footer.is_none() {
                *state = FooterSlot::default();
                continue;
            }
            let new_source = slot.footer.as_ref().is_some_and(|source| !state.source.as_ref()
                .is_some_and(|prev| slot.identity.is_some() || Arc::ptr_eq(prev, source)
                    || (prev.title == source.title && prev.detail_rk == source.detail_rk
                        && prev.ep_title == source.ep_title && prev.index == source.index
                        && prev.season == source.season)));
            if state.notice != slot.notice || state.identity != slot.identity || new_source {
                state.shown_at = Some(now);
            }
            state.notice = slot.notice;
            state.identity = slot.identity;
            state.source.clone_from(&slot.footer);
            if !state.shown_at.is_some_and(|since| now.wrapping_sub(since) < timeout_ms) {
                state.shown_at = None;
                continue;
            }
            let bounds = slot.bounds;
            let scale = (bounds.w / Rect::FULL.w).min(bounds.h / Rect::FULL.h);
            for clip in visible_regions(bounds, frame.slots[index + 1..].iter().map(|above| above.bounds)) {
                let _scope = plx_ui::screen::ClipScope::open_in(painter, clip);
                let target = painter.clipped(clip).translate(bounds.x + (bounds.w - Rect::FULL.w * scale) * 0.5,
                    bounds.y + bounds.h - Rect::FULL.h * scale).zoomed(Zoom { ox: 0.0, oy: 0.0, s: scale });
                if painter.is_recording() { painter.clip(clip); }
                if let Some(notice) = slot.notice {
                    use plx_ui::{theme, text_view::TextView};
                    let text = TextView::new(notice.message(), theme::size::OTT_FOOTER, theme::TEXT_PRIMARY)
                        .max_lines(2).break_long_words().with_measure(measure);
                    let pad = theme::space::LG;
                    let width = Rect::FULL.w - 2.0 * pad;
                    let height = text.measure_h(width) + 2.0 * pad;
                    let ground = Rect::new(0.0, Rect::FULL.h - height, Rect::FULL.w, height);
                    let color = theme::with_a(theme::CLASSIC_BACKGROUND, opacity.background_alpha(0.78));
                    target.rect(ground, 0.0, color, color, 0.0);
                    text.draw(target, Rect::new(pad, ground.y + pad, width, height - 2.0 * pad));
                } else if let Some(source) = &slot.footer {
                    let footer = state.renderer.get_or_insert_with(Box::default);
                    footer.draw_with_clock(target, source, slot.wall_now_s.or(wall_now_s), slot.decoder, opacity, mode, measure);
                }
                if painter.is_recording() { painter.clip_clear(); }
            }
        }
    }
}

// Disjoint clipping preserves lower-slot paint without covering higher native
// pictures. Layout validation allows overlap only for the two-slot PiP mode.
fn visible_regions(bounds: Rect, occluders: impl Iterator<Item = Rect>) -> Vec<Rect> {
    let mut regions = vec![bounds];
    for cover in occluders {
        let mut next = Vec::new();
        for region in regions {
            let x = region.x.max(cover.x);
            let y = region.y.max(cover.y);
            let right = (region.x + region.w).min(cover.x + cover.w);
            let bottom = (region.y + region.h).min(cover.y + cover.h);
            if x >= right || y >= bottom { next.push(region); continue; }
            for (left, top, right, bottom) in [
                (region.x, region.y, region.x + region.w, y),
                (region.x, bottom, region.x + region.w, region.y + region.h),
                (region.x, y, x, bottom),
                (right, y, region.x + region.w, bottom),
            ] {
                if left < right && top < bottom {
                    let mut width = right - left;
                    let mut height = bottom - top;
                    if left + width > right { width = width.next_down(); }
                    if top + height > bottom { height = height.next_down(); }
                    if width > 0.0 && height > 0.0 { next.push(Rect::new(left, top, width, height)); }
                }
            }
        }
        regions = next;
    }
    regions
}

#[cfg(test)]
mod tests;

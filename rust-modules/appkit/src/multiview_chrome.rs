//! Passive multi-view chrome and semantic selection events. The owner supplies
//! an admitted scene and decides whether this overlay owns input. Nothing here
//! opens media, changes audio, stores configuration, or enables multi-view.

use crate::multiview_layout::{GeometryError, Layout, PointerAction, PointerMap, RasterPlan, SlotId};
use plx_machine::machine::Measure;
use plx_ui::{icons::{self, Icon}, label::HAlign, screen::Dir, text_view::TextView, Painter, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState { Empty, Loading, Playing, Failed }

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeError { Geometry(GeometryError), InvalidSlot, DuplicateSlot, InvalidRevision, TitleTooLong }

/// Already localized by the host. A failed slot never paints a raw server error.
pub struct Labels<'a> { pub empty: &'a str, pub loading: &'a str, pub failed: &'a str }

#[derive(Clone, Debug, PartialEq)]
pub struct SlotChrome {
    pub id: SlotId,
    pub bounds: Rect,
    pub title: String,
    pub state: SlotState,
    pub active: bool,
    pub audio: bool,
    pub focused: bool,
    pub title_band: Rect,
    pub title_text: Rect,
    pub body: Rect,
    pub active_marker: Option<Rect>,
    pub audio_marker: Option<Rect>,
    pub border: [Rect; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChromeFrame {
    layout: Layout,
    state: ViewState,
    slots: Vec<SlotChrome>,
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
        let scale = layout.chrome_scale();
        let mut slots = Vec::with_capacity(layout.slots().len());
        for slot in layout.slots() {
            let view = views.iter().find(|view| view.id == slot.id);
            let r = slot.bounds;
            let pad = (8.0 * f64::from(scale)).min(f64::from(r.w.min(r.h)) * 0.04) as f32;
            let height = (36.0 * f64::from(scale)).min(f64::from(r.h) * 0.25) as f32;
            let title_band = Rect::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, height);
            let mark = (20.0 * f64::from(scale)).min(f64::from(height))
                .min(f64::from(title_band.w) * 0.15) as f32;
            let active = state.active == Some(slot.id);
            let audio = state.audio == Some(slot.id);
            let mut right = title_band.x + title_band.w - pad;
            let mut marker = || { right -= mark;
                let rect = Rect::new(right, title_band.cy() - mark * 0.5, mark, mark);
                right -= pad; rect };
            let audio_marker = audio.then(&mut marker);
            let active_marker = active.then(&mut marker);
            let title_text = Rect::new(title_band.x + pad, title_band.y,
                (right - title_band.x - pad).max(0.0), title_band.h);
            let body = Rect::new(r.x + pad, title_band.y + height + pad,
                r.w - 2.0 * pad, (r.y + r.h - pad - title_band.y - height - pad).max(0.0));
            let title = view.map(|view| view.title.chars().filter(|c| !c.is_control()).take(256).collect())
                .unwrap_or_default();
            slots.push(SlotChrome { id: slot.id, bounds: r, title,
                state: view.map_or(SlotState::Empty, |view| view.state), active, audio,
                focused: state.focused == Some(slot.id), title_band, title_text, body,
                active_marker, audio_marker,
                border: layout.active_border(slot.id).ok_or(ChromeError::Geometry(GeometryError::InvalidViewport))? });
        }
        Ok(Self { layout, state, slots })
    }

    pub fn slots(&self) -> &[SlotChrome] { &self.slots }
    pub fn layout(&self) -> &Layout { &self.layout }
    pub fn state(&self) -> ViewState { self.state }

    pub fn pointer(&self, map: &PointerMap, action: PointerAction, x: f32, y: f32) -> Option<Event> {
        if !self.state.input_owned { return None; }
        self.layout.activate_pointer(map, action, x, y).map(|slot|
            Event::ActivateSlot { revision: self.state.revision, slot })
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
                .map(|slot| Event::ActivateSlot { revision: self.state.revision, slot }),
            Input::Other => None,
        }
    }

    /// Paint chrome only. A Playing slot stays transparent over its video except
    /// for its title and markers; placeholders cover unavailable/stale pictures.
    pub fn draw(&self, painter: Painter, measure: &dyn Measure, labels: &Labels<'_>) {
        const WHITE: [f32; 4] = [0.95, 0.97, 1.0, 1.0];
        const ACTIVE: [f32; 4] = [0.22, 0.56, 1.0, 1.0];
        for (index, slot) in self.slots.iter().enumerate() {
            // Native video is already composed below this transparent UI plane.
            // Painting a lower placeholder over a higher Playing slot cannot be
            // undone by subsequently drawing that slot's transparent chrome.
            let visible = visible_regions(slot.bounds, self.slots[index + 1..].iter().map(|slot| slot.bounds));
            for clip in visible {
                painter.clip(clip);
                if slot.state != SlotState::Playing {
                    painter.rect(clip, 0.0, [0.025, 0.035, 0.05, 1.0], [0.025, 0.035, 0.05, 1.0], 0.0);
                }
                painter.rect(slot.title_band, 0.0, [0.0, 0.0, 0.0, 0.72], [0.0, 0.0, 0.0, 0.72], 0.0);
                let text = if slot.title.is_empty() { format!("{}", slot.id + 1) } else { slot.title.clone() };
                TextView::new(&text, 18, WHITE).bold().max_lines(1).break_long_words()
                    .with_measure(measure).h(HAlign::Start).draw(painter, slot.title_text);
                let placeholder = match slot.state { SlotState::Empty => Some(labels.empty),
                    SlotState::Loading => Some(labels.loading), SlotState::Failed => Some(labels.failed),
                    SlotState::Playing => None };
                if let Some(label) = placeholder {
                    let view = TextView::new(label, 18, WHITE).max_lines(2).break_long_words()
                        .with_measure(measure).h(HAlign::Center);
                    let height = view.measure_h(slot.body.w).min(slot.body.h);
                    view.draw(painter, Rect::new(slot.body.x, slot.body.cy() - height * 0.5, slot.body.w, height));
                }
                if let Some(rect) = slot.active_marker { icons::draw(painter, Icon::Check, rect, ACTIVE); }
                if let Some(rect) = slot.audio_marker { icons::draw(painter, Icon::Audio, rect, [1.0, 0.80, 0.24, 1.0]); }
                let border = if slot.active { ACTIVE } else if slot.focused { WHITE } else { [0.26, 0.30, 0.37, 1.0] };
                for band in slot.border { painter.rect(band, 0.0, border, border, 0.0); }
                painter.clip_clear();
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

//! Stable multi-view geometry, independent of sources, decoders and playback policy.
//! Grid cells exist even when the host has not configured their content. PiP is
//! the only overlapping layout; its inset is drawn and hit-tested above the main.

use plx_ui::{screen::Dir, Rect};

pub type SlotId = u8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Pip,
    Grid4,
    Grid9,
    /// Stable positions clockwise from north; zero leaves the central view alone.
    Perimeter { satellites: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    InvalidViewport,
    InvalidScale,
    TooManySatellites,
    InvalidRaster,
    CollapsedRasterSlot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub id: SlotId,
    pub bounds: Rect,
    /// Increasing draw order. Hit testing walks this order backwards.
    pub layer: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    mode: Mode,
    viewport: Rect,
    slots: Vec<Slot>,
    chrome_scale: f32,
}

/// A completed primary click is supplied by the input owner. Motion, scroll and
/// secondary clicks cannot change the selected slot through this geometry API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerAction {
    PrimaryClick,
    SecondaryClick,
    Move,
    Scroll,
}

fn valid_rect(r: Rect) -> bool {
    [r.x, r.y, r.w, r.h, r.x + r.w, r.y + r.h].iter().all(|v| v.is_finite())
        && r.w > 0.0 && r.h > 0.0 && r.x + r.w > r.x && r.y + r.h > r.y
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    // Half-open edges give adjacent cells one unambiguous owner.
    x.is_finite() && y.is_finite()
        && x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
}

fn inside(outer: Rect, inner: Rect) -> bool {
    valid_rect(inner) && inner.x >= outer.x && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

fn edges(x: f32, y: f32, right: f32, bottom: f32) -> Rect {
    let mut w = right - x;
    let mut h = bottom - y;
    // Subtraction followed by addition may round past the shared edge. Keep
    // the actual f32 paint rectangle inside it, including at fractional scale.
    if x + w > right { w = w.next_down(); }
    if y + h > bottom { h = h.next_down(); }
    Rect::new(x, y, w, h)
}

fn fraction(v: Rect, left: f64, top: f64, right: f64, bottom: f64) -> Rect {
    let x = (f64::from(v.x) + f64::from(v.w) * left) as f32;
    let y = (f64::from(v.y) + f64::from(v.h) * top) as f32;
    let right = ((f64::from(v.x) + f64::from(v.w) * right) as f32).min(v.x + v.w);
    let bottom = ((f64::from(v.y) + f64::from(v.h) * bottom) as f32).min(v.y + v.h);
    edges(x, y, right, bottom)
}

impl Layout {
    pub fn new(viewport: Rect, mode: Mode, chrome_scale: f32) -> Result<Self, GeometryError> {
        if !valid_rect(viewport) { return Err(GeometryError::InvalidViewport); }
        if !chrome_scale.is_finite() || chrome_scale <= 0.0 {
            return Err(GeometryError::InvalidScale);
        }
        let gap = (12.0 * f64::from(chrome_scale))
            .min(f64::from(viewport.w.min(viewport.h)) * 0.02);
        let gx = gap / f64::from(viewport.w);
        let gy = gap / f64::from(viewport.h);
        let mut slots = Vec::with_capacity(9);
        let mut push = |bounds: Rect| {
            let id = slots.len() as SlotId;
            slots.push(Slot { id, bounds, layer: id });
        };
        match mode {
            Mode::Pip => {
                push(viewport);
                push(fraction(viewport, 1.0 - gx - 0.28, 1.0 - gy - 0.28,
                    1.0 - gx, 1.0 - gy));
            }
            Mode::Grid4 | Mode::Grid9 => {
                let n = if mode == Mode::Grid4 { 2 } else { 3 };
                let w = (1.0 - gx * f64::from(n - 1)) / f64::from(n);
                let h = (1.0 - gy * f64::from(n - 1)) / f64::from(n);
                for row in 0..n {
                    for col in 0..n {
                        let left = f64::from(col) * (w + gx);
                        let top = f64::from(row) * (h + gy);
                        push(fraction(viewport, left, top,
                            if col + 1 == n { 1.0 } else { left + w },
                            if row + 1 == n { 1.0 } else { top + h }));
                    }
                }
            }
            Mode::Perimeter { satellites } => {
                if satellites > 8 { return Err(GeometryError::TooManySatellites); }
                push(fraction(viewport, 0.2 + gx, 0.2 + gy, 0.8 - gx, 0.8 - gy));
                // N, NE, E, SE, S, SW, W, NW. Adding a view never moves earlier slots.
                for &(x, y) in &[(0.5, 0.1), (0.9, 0.1), (0.9, 0.5), (0.9, 0.9),
                    (0.5, 0.9), (0.1, 0.9), (0.1, 0.5), (0.1, 0.1)][..usize::from(satellites)]
                {
                    push(fraction(viewport, x - 0.1 + gx * 0.5, y - 0.1 + gy * 0.5,
                        x + 0.1 - gx * 0.5, y + 0.1 - gy * 0.5));
                }
            }
        }
        if slots.iter().any(|slot| !inside(viewport, slot.bounds)) {
            return Err(GeometryError::InvalidViewport);
        }
        Ok(Self { mode, viewport, slots, chrome_scale })
    }

    pub fn mode(&self) -> Mode { self.mode }
    pub fn chrome_scale(&self) -> f32 { self.chrome_scale }
    pub fn viewport(&self) -> Rect { self.viewport }
    pub fn slots(&self) -> &[Slot] { &self.slots }
    pub fn slot(&self, id: SlotId) -> Option<&Slot> { self.slots.get(usize::from(id)) }

    /// Quantize inward on the compositor's output raster. This is a geometry
    /// limit, not a statement that a device can decode these views or dimensions.
    pub fn rasterize_even(&self, revision: u64) -> Result<RasterPlan, GeometryError> {
        let v = self.viewport;
        if v.x != 0.0 || v.y != 0.0 || v.w.fract() != 0.0 || v.h.fract() != 0.0
            || v.w < 2.0 || v.h < 2.0 || v.w > MAX_RASTER_EDGE as f32 || v.h > MAX_RASTER_EDGE as f32
            || v.w as u32 % 2 != 0 || v.h as u32 % 2 != 0
        { return Err(GeometryError::InvalidRaster); }
        let slots = self.slots.iter().map(|slot| {
            let r = slot.bounds;
            let x = (f64::from(r.x) / 2.0).ceil() as u32 * 2;
            let y = (f64::from(r.y) / 2.0).ceil() as u32 * 2;
            let right = (f64::from(r.x + r.w) / 2.0).floor() as u32 * 2;
            let bottom = (f64::from(r.y + r.h) / 2.0).floor() as u32 * 2;
            if right <= x || bottom <= y { return Err(GeometryError::CollapsedRasterSlot); }
            Ok(RasterSlot { id: slot.id, layer: slot.layer, x, y,
                width: right - x, height: bottom - y })
        }).collect::<Result<Vec<_>, _>>()?;
        RasterPlan::checked(revision, v.w as u32, v.h as u32, self.mode, slots)
    }

    /// Use checked actual compositor rectangles, without recalculating their
    /// layout. The owner separately verifies that the receipt matches its request.
    pub fn from_raster(plan: &RasterPlan, chrome_scale: f32) -> Result<Self, GeometryError> {
        if !chrome_scale.is_finite() || chrome_scale <= 0.0 { return Err(GeometryError::InvalidScale); }
        Ok(Self { mode: plan.mode, viewport: Rect::new(0.0, 0.0, plan.width as f32, plan.height as f32),
            slots: plan.slots.iter().map(|slot| Slot { id: slot.id, layer: slot.layer,
                bounds: slot.bounds() }).collect(), chrome_scale })
    }

    /// Uniformly project existing actual rectangles; never lay out the scene
    /// again from its mode when placing compositor receipts on the native UI.
    pub fn project_contain(&self, surface: Rect) -> Result<Self, GeometryError> {
        let map = PointerMap::contain(self.viewport, surface)?;
        let slots = self.slots.iter().map(|slot| Ok(Slot { id: slot.id, layer: slot.layer,
            bounds: map.project_rect(slot.bounds)? })).collect::<Result<Vec<_>, GeometryError>>()?;
        Ok(Self { mode: self.mode, viewport: map.painted, slots, chrome_scale: self.chrome_scale })
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<SlotId> {
        if !contains(self.viewport, x, y) { return None; }
        self.slots.iter().rev().find(|slot| contains(slot.bounds, x, y)).map(|slot| slot.id)
    }

    pub fn activate_pointer(&self, map: &PointerMap, action: PointerAction, x: f32, y: f32)
        -> Option<SlotId>
    {
        if action != PointerAction::PrimaryClick || map.logical != self.viewport { return None; }
        let (x, y) = map.to_logical(x, y)?;
        self.hit_test(x, y)
    }

    /// Four nonoverlapping paint bands, wholly inside the active slot. No
    /// centered stroke may escape into a neighboring view or beyond the viewport.
    pub fn active_border(&self, active: SlotId) -> Option<[Rect; 4]> {
        let r = self.slot(active)?.bounds;
        let t = (2.0 * f64::from(self.chrome_scale))
            .min(f64::from(r.w.min(r.h)) * 0.125) as f32;
        let right = r.x + r.w;
        let bottom = r.y + r.h;
        let bands = [edges(r.x, r.y, right, r.y + t),
            edges(r.x, bottom - t, right, bottom),
            edges(r.x, r.y + t, r.x + t, bottom - t),
            edges(right - t, r.y + t, right, bottom - t)];
        bands.iter().all(|band| inside(r, *band)).then_some(bands)
    }

    /// Prefer the next forward slot in the same directional beam, then the
    /// closest angle. IDs break exact ties; edges do not wrap or change selection.
    pub fn neighbor(&self, from: SlotId, direction: Dir) -> Option<SlotId> {
        let source = self.slot(from)?.bounds;
        let horizontal = matches!(direction, Dir::Left | Dir::Right);
        let mut best: Option<((u8, f64, f64, SlotId), SlotId)> = None;
        for slot in &self.slots {
            if slot.id == from { continue; }
            let target = slot.bounds;
            let dx = f64::from(target.cx()) - f64::from(source.cx());
            let dy = f64::from(target.cy()) - f64::from(source.cy());
            let forward = match direction { Dir::Left => -dx, Dir::Right => dx,
                Dir::Up => -dy, Dir::Down => dy };
            if forward <= 0.0 { continue; }
            let cross = if horizontal { dy.abs() } else { dx.abs() };
            let beam = if horizontal {
                source.y < target.y + target.h && target.y < source.y + source.h
            } else { source.x < target.x + target.w && target.x < source.x + source.w };
            let rank = (u8::from(!beam), if beam { forward } else { cross / forward },
                dx.hypot(dy), slot.id);
            let precedes = best.as_ref().is_none_or(|(old, _)| rank.0.cmp(&old.0)
                .then(rank.1.total_cmp(&old.1)).then(rank.2.total_cmp(&old.2))
                .then(rank.3.cmp(&old.3)).is_lt());
            if precedes { best = Some((rank, slot.id)); }
        }
        best.map(|(_, id)| id)
    }
}

/// A conservative integer validation ceiling only; backend admission may impose
/// a much smaller output and decoder budget.
pub const MAX_RASTER_EDGE: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterSlot {
    pub id: SlotId,
    pub layer: u8,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl RasterSlot {
    pub fn bounds(self) -> Rect { Rect::new(self.x as f32, self.y as f32, self.width as f32, self.height as f32) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterPlan {
    revision: u64,
    width: u32,
    height: u32,
    mode: Mode,
    slots: Vec<RasterSlot>,
}
impl RasterPlan {
    pub fn checked(revision: u64, width: u32, height: u32, mode: Mode, slots: Vec<RasterSlot>)
        -> Result<Self, GeometryError>
    {
        let count = match mode { Mode::Pip => 2, Mode::Grid4 => 4, Mode::Grid9 => 9,
            Mode::Perimeter { satellites } if satellites <= 8 => usize::from(satellites) + 1,
            Mode::Perimeter { .. } => return Err(GeometryError::TooManySatellites) };
        if !(2..=MAX_RASTER_EDGE).contains(&width) || !(2..=MAX_RASTER_EDGE).contains(&height)
            || width % 2 != 0 || height % 2 != 0 || slots.len() != count
        { return Err(GeometryError::InvalidRaster); }
        for (index, slot) in slots.iter().enumerate() {
            if usize::from(slot.id) != index || slot.layer != slot.id || slot.width == 0 || slot.height == 0
                || [slot.x, slot.y, slot.width, slot.height].iter().any(|v| v % 2 != 0)
                || slot.x.checked_add(slot.width).is_none_or(|right| right > width)
                || slot.y.checked_add(slot.height).is_none_or(|bottom| bottom > height)
            { return Err(GeometryError::InvalidRaster); }
        }
        let overlap = |a: RasterSlot, b: RasterSlot| a.x < b.x + b.width && b.x < a.x + a.width
            && a.y < b.y + b.height && b.y < a.y + a.height;
        if mode == Mode::Pip {
            let main = slots[0]; let inset = slots[1];
            if (main.x, main.y, main.width, main.height) != (0, 0, width, height)
                || inset.width >= width || inset.height >= height || !overlap(main, inset)
            { return Err(GeometryError::InvalidRaster); }
        } else {
            for (index, slot) in slots.iter().enumerate() {
                if slots[index + 1..].iter().any(|other| overlap(*slot, *other)) {
                    return Err(GeometryError::InvalidRaster);
                }
            }
        }
        Ok(Self { revision, width, height, mode, slots })
    }
    pub fn revision(&self) -> u64 { self.revision }
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn mode(&self) -> Mode { self.mode }
    pub fn slots(&self) -> &[RasterSlot] { &self.slots }
}

/// Uniform contain transform for the logical renderer viewport on a physical
/// surface. Letterbox bars are not input targets. It carries no device/DPI policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerMap {
    logical: Rect,
    painted: Rect,
}

impl PointerMap {
    pub fn contain(logical: Rect, surface: Rect) -> Result<Self, GeometryError> {
        if !valid_rect(logical) || !valid_rect(surface) { return Err(GeometryError::InvalidViewport); }
        let scale = (f64::from(surface.w) / f64::from(logical.w))
            .min(f64::from(surface.h) / f64::from(logical.h));
        let w = (f64::from(logical.w) * scale) as f32;
        let h = (f64::from(logical.h) * scale) as f32;
        let x = surface.x + (surface.w - w) * 0.5;
        let y = surface.y + (surface.h - h) * 0.5;
        let painted = edges(x, y, (x + w).min(surface.x + surface.w),
            (y + h).min(surface.y + surface.h));
        if !inside(surface, painted) { return Err(GeometryError::InvalidViewport); }
        Ok(Self { logical, painted })
    }
    pub fn painted(&self) -> Rect { self.painted }
    pub fn project_rect(&self, rect: Rect) -> Result<Rect, GeometryError> {
        if !inside(self.logical, rect) { return Err(GeometryError::InvalidViewport); }
        let left = (f64::from(rect.x) - f64::from(self.logical.x)) / f64::from(self.logical.w);
        let top = (f64::from(rect.y) - f64::from(self.logical.y)) / f64::from(self.logical.h);
        let right = (f64::from(rect.x + rect.w) - f64::from(self.logical.x)) / f64::from(self.logical.w);
        let bottom = (f64::from(rect.y + rect.h) - f64::from(self.logical.y)) / f64::from(self.logical.h);
        let projected = fraction(self.painted, left, top, right, bottom);
        inside(self.painted, projected).then_some(projected).ok_or(GeometryError::InvalidViewport)
    }
    pub fn to_logical(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if !contains(self.painted, x, y) { return None; }
        let x = (f64::from(self.logical.x) + (f64::from(x) - f64::from(self.painted.x))
            * f64::from(self.logical.w) / f64::from(self.painted.w)) as f32;
        let y = (f64::from(self.logical.y) + (f64::from(y) - f64::from(self.painted.y))
            * f64::from(self.logical.h) / f64::from(self.painted.h)) as f32;
        contains(self.logical, x, y).then_some((x, y))
    }
}

#[cfg(test)]
mod tests;

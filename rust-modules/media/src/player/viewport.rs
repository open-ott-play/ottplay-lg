//! Placement of an already admitted native picture. This never starts or resumes playback.
//! The application owns the screen lease; an Engine owns only its accepted placement.
use super::{
    adapter::PlayerAdapter,
    engine::Engine,
    shared::Stage,
    video_geometry::{Transform, Viewport},
};
use crate::route::{PlaybackIdentity, PlaybackSession, RouteStartAttempt, RouteStartStatus};
use plx_base::task::MainThread;
use std::sync::atomic::Ordering::Relaxed;

pub type LogicalRect = Viewport;

#[derive(Clone, PartialEq, Eq)]
pub struct Source {
    identity: PlaybackIdentity,
    native_epoch: u32,
    attempt: RouteStartAttempt,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Source(<opaque>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Fullscreen,
    Inline {
        rect: LogicalRect,
        canvas: (i32, i32),
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission {
    Submitted,
    Unchanged,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    StalePlayback,
    NotReady,
    InvalidGeometry,
    Unsupported,
    NativeRejected,
}

/// Supported content transforms for the current picture, independent of screen placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentCapability {
    FitOnly,
    FitFillZoom,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentState {
    pub capability: ContentCapability,
    pub transform: Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    mode: i32,
    source: (i32, i32),
    destination: Viewport,
}

pub(crate) struct State {
    target: Target,
    // Fullscreen intent survives temporary Inline placement; each new Engine starts at Fit.
    transform: Transform,
    submitted: Option<Placement>,
    refused: Option<Placement>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            target: Target::Fullscreen,
            transform: Transform::default(),
            submitted: None,
            refused: None,
        }
    }
}

fn owns(engine: &Engine, ps: &PlaybackSession, source: &Source) -> bool {
    engine.native_epoch == source.native_epoch
        && engine.route_start == source.attempt
        && crate::route::playback_identity_matches(ps, &source.identity)
        && super::SHARED
            .native_load_elapsed(engine.native_epoch)
            .is_some()
}
fn started(engine: &Engine) -> bool {
    matches!(
        crate::route::route_start_status(engine.route_start),
        RouteStartStatus::Started
    )
}
fn ready(engine: &Engine) -> bool {
    engine.stage >= Stage::Streaming
        && !engine.preview_abandon
        && super::SHARED.native_load_returned(engine.native_epoch)
}

pub fn capture_source(pa: &mut PlayerAdapter, ps: &PlaybackSession) -> Option<Source> {
    let engine = pa.engine()?;
    if !ready(engine) || !started(engine) {
        return None;
    }
    let source = Source {
        identity: crate::route::playback_identity(ps)?,
        native_epoch: engine.native_epoch,
        attempt: engine.route_start,
    };
    owns(engine, ps, &source).then_some(source)
}
/// Exact retained native/logical ownership, independent of readiness or a pending replacement.
/// A preparing replacement may fail before teardown; the old window still needs restoration.
pub fn source_is_current(pa: &mut PlayerAdapter, ps: &PlaybackSession, source: &Source) -> bool {
    pa.engine().is_some_and(|engine| owns(engine, ps, source))
}

/// This is passive eligibility, not a decoder probe or a playback command.
fn content_capability() -> Result<ContentCapability, Failure> {
    if !matches!(
        super::sink().window_mode(),
        super::VP_EXPORTED | super::VP_ACB
    ) {
        return Err(Failure::Unsupported);
    }
    #[cfg(feature = "hostsim")]
    {
        // A clock-only simulator has no picture to resize. LATEST is the existing
        // renderer's evidence; querying it never arms or starts the decoder.
        if !super::sim_video::has_picture() {
            return Err(Failure::NotReady);
        }
        if super::sink().window_mode() == super::VP_EXPORTED {
            return Ok(ContentCapability::FitFillZoom);
        }
    }
    Ok(ContentCapability::FitOnly)
}

/// Current fullscreen intent for an exact, started picture. Inline owns only its
/// aperture and always uses Fit; content controls wait until that lease is released.
pub fn content_state(
    pa: &mut PlayerAdapter,
    ps: &PlaybackSession,
    source: &Source,
) -> Result<ContentState, Failure> {
    let engine = pa.engine().ok_or(Failure::StalePlayback)?;
    if !owns(engine, ps, source) {
        return Err(Failure::StalePlayback);
    }
    if !ready(engine) || !started(engine) {
        return Err(Failure::NotReady);
    }
    if !matches!(engine.viewport.target, Target::Fullscreen) {
        return Err(Failure::Unsupported);
    }
    Ok(ContentState {
        capability: content_capability()?,
        transform: engine.viewport.transform,
    })
}

/// Change only this picture's fullscreen content mapping. Unsupported TV cropping
/// is refused before dispatch. A refusal leaves the accepted intent and window intact.
pub fn apply_content(
    pa: &mut PlayerAdapter,
    ps: &PlaybackSession,
    source: &Source,
    transform: Transform,
) -> Result<Submission, Failure> {
    let state = content_state(pa, ps, source)?;
    if state.capability == ContentCapability::FitOnly && transform != Transform::default() {
        return Err(Failure::Unsupported);
    }
    apply_to(pa, ps, source, Target::Fullscreen, transform)
}

fn validate(target: Target) -> Result<(), Failure> {
    let Target::Inline { rect, canvas } = target else {
        return Ok(());
    };
    if canvas.0 <= 0
        || canvas.1 <= 0
        || rect.x < 0
        || rect.y < 0
        || rect.width <= 0
        || rect.height <= 0
        || rect
            .x
            .checked_add(rect.width)
            .is_none_or(|end| end > canvas.0)
        || rect
            .y
            .checked_add(rect.height)
            .is_none_or(|end| end > canvas.1)
    {
        return Err(Failure::InvalidGeometry);
    }
    Ok(())
}

fn exported_destination(target: Target) -> Result<Viewport, Failure> {
    match target {
        // Preserve the existing full-window contract. Host placement uses this authored canvas;
        // the TV exported-window baseline also creates its surface in this coordinate system.
        Target::Fullscreen => Ok(Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
        Target::Inline { rect, canvas } => {
            validate(target)?;
            if canvas != (1920, 1080) {
                return Err(Failure::Unsupported);
            }
            // HostSink owns aspect fitting and the matte for the entire logical aperture.
            // Passing an already-fitted picture would lose the matte and round twice.
            if cfg!(feature = "hostsim") {
                return Ok(rect);
            }
            // A TV exported region
            // is surface-relative (LG webos-foreign protocol), not panel pixels. Until a scaled
            // TV surface mapping is qualified, accept only the established identity transform.
            if plx_base::surface::viewport() != (0, 0, 1920, 1080) {
                return Err(Failure::Unsupported);
            }
            let picture = super::video_viewport(rect.width, rect.height);
            Ok(Viewport {
                x: rect.x + picture.x,
                y: rect.y + picture.y,
                ..picture
            })
        }
    }
}

fn content_destination(target: Target, transform: Transform) -> Result<Viewport, Failure> {
    if !matches!(target, Target::Fullscreen) || transform == Transform::default() {
        // In particular Fit/1 retains the exact existing full-aperture request.
        return exported_destination(target);
    }
    if !cfg!(feature = "hostsim") {
        return Err(Failure::Unsupported);
    }
    let (w, h) = raster();
    let aspect = super::video_geometry::Aspect::unpack(
        super::SHARED
            .video_aspect
            .load(std::sync::atomic::Ordering::Acquire),
    )
    .or_else(|| super::video_geometry::Aspect::from_raster(w, h))
    .ok_or(Failure::InvalidGeometry)?;
    // This is the nominal window. HostSink applies its established aspect fit once;
    // the renderer's accepted-window query supplies the resulting picture to captions.
    transform
        .picture(aspect, 1920, 1080)
        .ok_or(Failure::InvalidGeometry)
}

fn raster() -> (i32, i32) {
    let size = super::SHARED.video_raster();
    if size.0 > 0 && size.1 > 0 {
        size
    } else {
        (1920, 1080)
    }
}

fn submit_exported(
    mt: &MainThread,
    engine: &mut Engine,
    target: Target,
    transform: Transform,
    explicit: bool,
) -> Result<Submission, Failure> {
    let placement = Placement {
        mode: super::VP_EXPORTED,
        source: raster(),
        destination: content_destination(target, transform)?,
    };
    if engine.viewport.submitted == Some(placement) {
        engine.viewport.target = target;
        engine.viewport.transform = transform;
        return Ok(Submission::Unchanged);
    }
    if !explicit && engine.viewport.refused == Some(placement) {
        return Err(Failure::NativeRejected);
    }
    let dst = placement.destination;
    // SAFETY: callers hold the current Engine's returned Load and its main-thread token.
    let rv = unsafe {
        super::sink().place_window(
            mt,
            placement.source.0,
            placement.source.1,
            dst.x,
            dst.y,
            dst.width,
            dst.height,
        )
    };
    engine.placed_src = placement.source;
    super::SHARED.dg_place_rv.store(rv, Relaxed);
    super::SHARED.dg_placed_w.store(placement.source.0, Relaxed);
    super::SHARED.dg_placed_h.store(placement.source.1, Relaxed);
    super::log(&format!(
        "vplane: exported window placed src={}x{} dst={},{},{}x{} rv={rv}",
        placement.source.0, placement.source.1, dst.x, dst.y, dst.width, dst.height
    ));
    if rv <= 0 {
        engine.viewport.refused = Some(placement);
        return Err(Failure::NativeRejected);
    }
    engine.viewport.target = target;
    engine.viewport.transform = transform;
    engine.viewport.submitted = Some(placement);
    engine.viewport.refused = None;
    Ok(Submission::Submitted)
}

/// Initial exported placement and source-raster/aspect correction share this actuator.
/// A failed identical automatic placement is not retried at frame rate; an explicit request
/// may retry it. A late raster correction never restores a fullscreen destination over inline.
pub(crate) fn refresh_exported(
    mt: &MainThread,
    engine: &mut Engine,
) -> Result<Submission, Failure> {
    if super::sink().window_mode() != super::VP_EXPORTED {
        return Err(Failure::Unsupported);
    }
    if engine.stage < Stage::Playing
        || engine.preview_abandon
        || !super::SHARED.native_load_returned(engine.native_epoch)
    {
        return Err(Failure::NotReady);
    }
    let target = engine.viewport.target;
    let transform = engine.viewport.transform;
    submit_exported(mt, engine, target, transform, false)
}

/// Submit geometry for exactly the captured native owner. No Load, Play, Pause, seek, source
/// resolution or viewer preference is issued. The result is dispatch evidence, not readback.
pub fn apply(
    pa: &mut PlayerAdapter,
    ps: &PlaybackSession,
    source: &Source,
    target: Target,
) -> Result<Submission, Failure> {
    let transform = pa
        .engine()
        .ok_or(Failure::StalePlayback)?
        .viewport
        .transform;
    apply_to(pa, ps, source, target, transform)
}

fn apply_to(
    pa: &mut PlayerAdapter,
    ps: &PlaybackSession,
    source: &Source,
    target: Target,
    transform: Transform,
) -> Result<Submission, Failure> {
    let (engine, mt) = pa.split();
    let engine = engine.ok_or(Failure::StalePlayback)?;
    if !owns(engine, ps, source) {
        return Err(Failure::StalePlayback);
    }
    if !ready(engine) || (matches!(target, Target::Inline { .. }) && !started(engine)) {
        return Err(Failure::NotReady);
    }
    validate(target)?;
    match super::sink().window_mode() {
        super::VP_EXPORTED => submit_exported(mt, engine, target, transform, true),
        super::VP_ACB => {
            // The ACB header proves fullScreen, but not a TV's logical-to-output transform.
            // Never forward logical inline coordinates to that display-output API by guessing.
            if !matches!(target, Target::Fullscreen) {
                return Err(Failure::Unsupported);
            }
            let placement = Placement {
                mode: super::VP_ACB,
                source: raster(),
                destination: Viewport {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            };
            if engine.viewport.submitted == Some(placement) {
                engine.viewport.target = target;
                engine.viewport.transform = transform;
                return Ok(Submission::Unchanged);
            }
            // SAFETY: exact current loaded Engine; fullScreen=true preserves the established
            // ACB full-window interpretation, and plane_place never sends setState(PLAYING).
            let rv = unsafe { super::sink().plane_place(mt, 0, 0, 1920, 1080, true) };
            if rv < 0 {
                return Err(Failure::NativeRejected);
            }
            engine.viewport.target = target;
            engine.viewport.transform = transform;
            engine.viewport.submitted = Some(placement);
            Ok(Submission::Submitted)
        }
        _ => Err(Failure::Unsupported),
    }
}

/// Scoped synthetic picture for cross-layer placement tests. This never starts
/// a decoder; the caller must hold the existing serial test lock.
#[cfg(all(feature = "hostsim", any(test, feature = "test-support")))]
pub fn picture_for_test() -> super::sim_video::PictureForTest {
    super::sim_video::picture_for_test()
}

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;

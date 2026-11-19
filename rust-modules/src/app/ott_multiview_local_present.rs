//! Main-thread output for independently owned local pictures. Rendering owns no source grants.
use super::*;
use plx_appkit::multiview_layout::RasterPlan;
use plx_data::ott::multiview::{ResolveTicket, SlotId};
use plx_gfx::gfx::video::{Matrix, Range, Texture};
use plx_media::local_multiview::{ColorMatrix, ColorRange};
use plx_platform::local_audio::{OwnerToken, Sink};

struct Previous {
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: CatalogEntry,
    favorite: Option<Favorite>,
}
type PreviousPrepared = (
    PlaybackRequest,
    ProviderProfile,
    Option<CatalogEntry>,
    Option<Favorite>,
);
impl Previous {
    fn capture(runtime: &Runtime, playing: &Playing, position_ms: u64) -> Option<Self> {
        let request = PlaybackRequest {
            kiosk_selection: playing.kiosk_selection.clone(),
            archive: playing.archive.clone(),
            transient_owner: playing.transient_owner,
            local_selection: playing.local_selection.clone(),
            continuation: None,
            id: playing.selection_id.clone(),
            position_ms,
            advance_on_failure: false,
        };
        let (bound, profile, _entry, favorite) =
            runtime.prepare_play_request(request.clone()).ok()?;
        if bound.id != request.id || profile != playing.profile {
            return None;
        }
        Some(Self {
            request,
            profile,
            entry: playing.entry.clone(),
            favorite,
        })
    }
    fn prepared(&self, runtime: &Runtime) -> Result<PreviousPrepared, &'static str> {
        let prepared = runtime.prepare_play_request(self.request.clone())?;
        if prepared.0.id != self.request.id
            || prepared.1 != self.profile
            || prepared.3 != self.favorite
        {
            return Err("Previous source changed; select it again");
        }
        if let Some(favorite) = self.favorite.as_ref() {
            if self.request.id != format!("favorite:{}", favorite.identity()) {
                return Err("Previous favorite identity changed");
            }
        } else if runtime.effective_profile() != Some(&self.profile)
            || prepared.2.as_ref() != Some(&self.entry)
        {
            return Err("Previous source changed; select it again");
        }
        Ok(prepared)
    }
}
struct LeaseClose {
    lease: Arc<std::sync::Mutex<Option<super::super::ott_vportal_lease::Lease>>>,
    done: Arc<AtomicBool>,
    success: Arc<AtomicBool>,
    started: bool,
}
impl LeaseClose {
    fn poll(&mut self) -> Option<bool> {
        if !self.started {
            let lease = Arc::clone(&self.lease);
            let done = Arc::clone(&self.done);
            let success = Arc::clone(&self.success);
            self.started = plx_base::task::spawn_off_frame("local-mv-prior-close", move |_| {
                let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    lease
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take()
                        .is_none_or(|l| l.finish_off_frame().is_ok())
                }))
                .unwrap_or(false);
                success.store(ok, Ordering::Release);
                done.store(true, Ordering::Release);
            });
        }
        self.done
            .load(Ordering::Acquire)
            .then(|| self.success.load(Ordering::Acquire))
    }
}
/// Each uploaded frame can contribute at most one successful draw.
#[derive(Clone, Copy, Default)]
struct RenderedFrames {
    pending_pts_us: Option<i64>,
    last_pts_us: Option<i64>,
    count: u64,
    last_draw_ms: Option<u64>,
    intervals_ms: [u32; 32],
    intervals_ge_250ms_count: u64,
    lifetime_max_ms: Option<u64>,
}
impl RenderedFrames {
    fn submitted(&mut self, now_ms: u64) {
        if let Some(pts) = self.pending_pts_us.take() {
            if let Some(previous) = self.last_draw_ms {
                // count is the number of pictures already drawn; the second
                // picture supplies the first interval.
                let index = (self.count.saturating_sub(1) % self.intervals_ms.len() as u64) as usize;
                let interval_ms = now_ms.saturating_sub(previous);
                self.intervals_ms[index] = interval_ms.min(u64::from(u32::MAX)) as u32;
                if interval_ms >= 250 {
                    self.intervals_ge_250ms_count = self.intervals_ge_250ms_count.saturating_add(1);
                }
                self.lifetime_max_ms = Some(self.lifetime_max_ms.unwrap_or(0).max(interval_ms));
            }
            self.last_draw_ms = Some(now_ms);
            self.count = self.count.saturating_add(1);
            self.last_pts_us = Some(pts);
        }
    }

    fn draw_intervals(&self) -> Option<(u32, u32)> {
        let count = self.count.saturating_sub(1).min(self.intervals_ms.len() as u64) as usize;
        if count == 0 { return None; }
        let mut ordered = self.intervals_ms;
        ordered[..count].sort_unstable();
        Some((ordered[(count * 95).div_ceil(100) - 1], ordered[count - 1]))
    }
}

pub(super) struct State {
    taken: bool,
    graphics_ready: bool,
    previous: Option<Previous>,
    lease: Option<LeaseClose>,
    lease_failed: bool,
    textures: [Option<Texture>; 9],
    frame_tickets: [Option<ResolveTicket>; 9],
    display_aspects: [f32; 9],
    frames: [RenderedFrames; 9],
    plan: Option<RasterPlan>,
    sink: Option<Sink>,
    audio: Option<(SlotId, ResolveTicket, OwnerToken)>,
    serial: u64,
    error: Option<&'static str>,
    audio_epoch: u64,
    pending_pcm: Option<(SlotId, ResolveTicket, plx_media::local_multiview::PcmChunk)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            taken: false,
            graphics_ready: false,
            previous: None,
            lease: None,
            lease_failed: false,
            textures: std::array::from_fn(|_| None),
            frame_tickets: [None; 9],
            display_aspects: [0.0; 9],
            frames: [RenderedFrames::default(); 9],
            plan: None,
            sink: None,
            audio: None,
            serial: 0,
            error: None,
            audio_epoch: 0,
            pending_pcm: None,
        }
    }
}
impl State {
    fn error(&mut self, runtime: &mut Runtime, error: &'static str) {
        if self.error != Some(error) {
            self.error = Some(error);
            runtime.status = error.into();
            runtime.changed();
        }
    }
    /// Apply the ONE player-owned desired volume only to this acknowledged local sink.
    /// Success is dispatch, never a physical volume readback or a TV volume grant.
    fn apply_volume(&mut self, runtime: &mut Runtime, app: &App) -> bool {
        if !self.taken
            || !self.graphics_ready
            || self.lease.is_some()
            || self.lease_failed
            || app.adapters.player.is_live()
            || app.adapters.player.release_busy()
            || app.adapters.player.has_pending_release()
            || !runtime.multiview_local.owns_ui(runtime, app)
        {
            return false;
        }
        let Some((slot, ticket, token)) = self.audio else {
            return false;
        };
        if runtime.multiview_local.output_ticket(runtime.multiview_local.audio_owner()) != Some((slot, ticket)) {
            return false;
        }
        let Some(sink) = self.sink.as_mut() else {
            return false;
        };
        match sink.set_gain(
            app.adapters.player.mt(),
            token,
            app.player.volume.desired().output(),
        ) {
            Ok(()) => true,
            Err(error) => {
                sink.clear(app.adapters.player.mt());
                self.audio = None;
                self.pending_pcm = None;
                self.error(runtime, error);
                let mut driver = std::mem::take(&mut runtime.multiview_local);
                driver.audio_output_failed(runtime);
                runtime.multiview_local = driver;
                false
            }
        }
    }
    fn clear_output(&mut self, app: &App) {
        if let Some(sink) = self.sink.as_mut() {
            sink.clear(app.adapters.player.mt());
        }
        self.sink = None;
        self.audio = None;
        self.pending_pcm = None;
        self.textures = std::array::from_fn(|_| None);
        self.frame_tickets = [None; 9];
        self.frames = [RenderedFrames::default(); 9];
        self.plan = None;
    }
    fn takeover(&mut self, runtime: &mut Runtime, app: &mut App) {
        self.previous = runtime
            .playing
            .as_ref()
            .filter(|p| {
                p.identity.as_ref().is_some_and(|id| {
                    plx_media::route::playback_identity_matches(&app.player.session, id)
                })
            })
            .and_then(|p| {
                Previous::capture(
                    runtime,
                    p,
                    if guide_ui::is_live(&p.profile, &p.entry) {
                        0
                    } else {
                        (playback::intended_pos(&app.player.session).max(0) as u64) / 1_000_000
                    },
                )
            });
        runtime.checkpoint(app, true);
        runtime.cancel_play_request();
        runtime.resume_pending = None;
        runtime.failed_playback = None;
        playback::replace_player_in_place(
            &mut app.player,
            &mut app.adapters.player,
            &mut app.repause_at,
            &mut app.pages,
        );
        runtime.playing = None;
        runtime.programme_leaf = None;
        runtime.live_hud = Default::default();
        runtime.metadata_playback_identity = None;
        app.bridge
            .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
        self.graphics_ready = Texture::probe(app.adapters.player.mt()).is_ok();
        if !self.graphics_ready {
            runtime.status = "Local video graphics unavailable".into();
            runtime.changed();
            let mut driver = std::mem::take(&mut runtime.multiview_local);
            driver.revoke(runtime);
            runtime.multiview_local = driver;
        }
        if let Some(held) = runtime.active_lease.take() {
            let lease = held.into_lease();
            lease.begin_release();
            self.lease = Some(LeaseClose {
                lease: Arc::new(std::sync::Mutex::new(Some(lease))),
                done: Arc::new(AtomicBool::new(false)),
                success: Arc::new(AtomicBool::new(false)),
                started: false,
            });
        }
        playback::enter_player(
            &mut app.pages,
            &mut app.bridge,
            playback::Origin::Here,
            None,
        );
        self.taken = true;
    }
    fn tick(&mut self, runtime: &mut Runtime, app: &mut App) {
        if let Some(close) = self.lease.as_mut() {
            if let Some(ok) = close.poll() {
                self.lease_failed |= !ok;
                self.lease = None;
                if !ok {
                    runtime.status = "Previous playback session did not finish closing".into();
                    runtime.changed();
                }
            }
        }
        let visible = runtime.multiview_local.active()
            && app.running
            && playback_allowed(app)
            && runtime
                .multiview
                .validate_active_owner(runtime, app)
                .is_ok();
        if visible && !self.taken {
            self.takeover(runtime, app);
        }
        if !visible {
            self.clear_output(app);
            if self.taken
                && !runtime.multiview_local.active()
                && !runtime.multiview_local.has_work()
                && self.lease.is_none()
                && !app.adapters.player.release_busy()
            {
                let restore = runtime.multiview_local.take_restore_previous();
                self.taken = false;
                let previous = self.previous.take();
                if restore
                    && !self.lease_failed
                    && app.running
                    && playback_allowed(app)
                    && runtime.play_work_allowed()
                    && !app.ott_standby
                {
                    if let Some(previous) = previous {
                        // Bind a Favorite by its captured canonical identity even when its
                        // provider is not the currently browsed provider. The ordinary
                        // prepared request still owns fresh policy and source resolution.
                        let result = previous.prepared(runtime).and_then(|prepared| {
                            runtime.play_prepared_request_with_quality_policy(app, prepared, false)
                        });
                        if result.is_err() {
                            runtime.status = "Previous source changed; select it again".into();
                            runtime.changed();
                        }
                    }
                }
                self.lease_failed = false;
            }
            return;
        }
        self.plan = runtime.multiview_local.geometry(
            plx_base::surface::LOGICAL_W as u32,
            plx_base::surface::LOGICAL_H as u32,
        );
        for index in 0..9 {
            let Some(slot) = SlotId::new(index as u8) else {
                continue;
            };
            let ticket = runtime.multiview_local.slot_ticket(slot);
            if !self.update_frame_ticket(index, ticket) {
                continue;
            }
            let Some(frame) = runtime.multiview_local.take_frame(slot) else {
                continue;
            };
            let result = (|| {
                if self.textures[index].is_none() {
                    self.textures[index] = Some(Texture::new(app.adapters.player.mt())?);
                }
                self.textures[index]
                    .as_mut()
                    .ok_or("Video texture unavailable")?
                    .upload(
                        app.adapters.player.mt(),
                        &frame.y,
                        &frame.u,
                        &frame.v,
                        frame.width,
                        frame.height,
                        frame.chroma_width,
                        frame.chroma_height,
                        match frame.range {
                            ColorRange::Limited => Range::Limited,
                            ColorRange::Full => Range::Full,
                        },
                        match frame.matrix {
                            ColorMatrix::Bt601 => Matrix::Bt601,
                            ColorMatrix::Bt709 => Matrix::Bt709,
                        },
                    )
            })();
            if let Err(error) = result {
                runtime.status = error.into();
                runtime.multiview_local.texture_failed(slot);
                if let Ok(t) = runtime.multiview.close_slot(slot) {
                    runtime.multiview_effects.extend(t.effects);
                }
                runtime.changed();
                self.textures[index] = None;
            } else {
                self.display_aspects[index] = frame.display_aspect;
                self.frames[index].pending_pts_us = Some(frame.pts_us);
            }
        }
        if app.adapters.player.release_busy() || self.lease.is_some() || self.lease_failed {
            if let Some(sink) = self.sink.as_mut() {
                sink.clear(app.adapters.player.mt());
            }
            return;
        }
        let output_epoch = runtime.multiview_local.audio_epoch();
        if self.audio_epoch != output_epoch {
            self.audio_epoch = output_epoch;
            self.pending_pcm = None;
            if let Some(sink) = self.sink.as_mut() {
                sink.clear(app.adapters.player.mt());
            }
        }
        let request = runtime.multiview_local.audio_request();
        let owner = request.map(|(_, _, slot)| slot).unwrap_or_else(|| runtime.multiview_local.audio_owner());
        let expected = runtime.multiview_local.output_ticket(owner);
        let current = self.audio.map(|(slot, ticket, _)| (slot, ticket));
        if request.is_some() || current != expected {
            let Some(next_serial) = self.serial.checked_add(1) else {
                if let Some(sink) = self.sink.as_mut() {
                    sink.clear(app.adapters.player.mt());
                }
                self.audio = None;
                let mut driver = std::mem::take(&mut runtime.multiview_local);
                driver.confirm_audio(runtime, false);
                runtime.multiview_local = driver;
                return;
            };
            self.serial = next_serial;
            let next = expected.map(|(slot, ticket)| (slot, ticket, OwnerToken {
                scene: self.serial,
                slot: slot.index() as u8,
                generation: self.serial,
            }));
            let result = if owner.is_none() || (expected.is_none() && runtime.multiview_local.mixing_audio()) {
                if let Some(sink) = self.sink.as_mut() {
                    sink.set_owner(app.adapters.player.mt(), None)
                } else {
                    Ok(())
                }
            } else if let Some((_, _, token)) = next {
                match self.sink.as_mut() {
                    Some(sink) => sink.set_owner(app.adapters.player.mt(), Some(token)),
                    None => match Sink::open(app.adapters.player.mt()) {
                        Ok(mut sink) => {
                            let result = sink.set_owner(app.adapters.player.mt(), Some(token));
                            self.sink = Some(sink);
                            result
                        }
                        Err(error) => Err(error),
                    },
                }
            } else {
                Err("Local audio source is not ready")
            };
            let accepted = result.is_ok();
            if let Err(error) = result {
                self.error(runtime, error);
            }
            self.pending_pcm = None;
            if !accepted {
                if let Some(sink) = self.sink.as_mut() {
                    sink.clear(app.adapters.player.mt());
                }
            }
            self.audio = if accepted { next } else { None };
            let mut driver = std::mem::take(&mut runtime.multiview_local);
            if request.is_some() {
                driver.confirm_audio(runtime, accepted);
            } else if !accepted {
                driver.audio_output_failed(runtime);
            }
            runtime.multiview_local = driver;
        }
        if self.audio.is_some_and(|(slot, ticket, _)| {
            runtime.multiview_local.output_ticket(runtime.multiview_local.audio_owner()) != Some((slot, ticket))
        }) {
            if let Some(sink) = self.sink.as_mut() {
                let _ = sink.set_owner(app.adapters.player.mt(), None);
            }
            self.audio = None;
        }
        // The default SDL gain is unity; apply an earlier mute/level immediately
        // after acknowledgement and before the first sample reaches the queue.
        if self.audio.is_some() && !self.apply_volume(runtime, app) {
            return;
        }
        for _ in 0..16 {
            let Some(sink) = self.sink.as_ref() else { break };
            let queued_frames = (sink.queued_bytes(app.adapters.player.mt()) / 4) as usize;
            let Some((slot, ticket, chunk)) = self
                .pending_pcm
                .take()
                .or_else(|| runtime.multiview_local.take_audio(queued_frames))
            else {
                break;
            };
            if let Some((owner, current, token)) = self.audio {
                if slot == owner
                    && ticket == current
                    && runtime.multiview_local.slot_ticket(slot) == Some(ticket)
                {
                    if let Some(sink) = self.sink.as_mut() {
                        let bytes = u32::try_from(chunk.samples.len())
                            .ok()
                            .and_then(|n| n.checked_mul(2));
                        if bytes.is_some_and(|bytes| {
                            bytes <= plx_platform::local_audio::MAX_QUEUED_BYTES
                                && sink
                                    .queued_bytes(app.adapters.player.mt())
                                    .checked_add(bytes)
                                    .is_none_or(|n| n > plx_platform::local_audio::MAX_QUEUED_BYTES)
                        }) {
                            self.pending_pcm = Some((slot, ticket, chunk));
                            break;
                        }
                        if let Err(error) =
                            sink.queue(app.adapters.player.mt(), token, &chunk.samples)
                        {
                            sink.clear(app.adapters.player.mt());
                            self.audio = None;
                            self.pending_pcm = None;
                            self.error(runtime, error);
                            let mut driver = std::mem::take(&mut runtime.multiview_local);
                            driver.audio_output_failed(runtime);
                            runtime.multiview_local = driver;
                            break;
                        }
                    }
                }
            }
        }
    }
    fn update_frame_ticket(&mut self, index: usize, ticket: Option<ResolveTicket>) -> bool {
        let Some(ticket) = ticket else {
            // Drop pixels immediately; keep only successfully drawn evidence under
            // the original ticket. A pending upload was never shown.
            self.textures[index] = None;
            self.frames[index].pending_pts_us = None;
            return false;
        };
        if self.frame_tickets[index] != Some(ticket) {
            self.textures[index] = None;
            self.frame_tickets[index] = Some(ticket);
            self.frames[index] = RenderedFrames::default();
        }
        true
    }

    fn draw(&mut self, app: &App, draw_ms: u64) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let black = [0.0, 0.0, 0.0, 1.0];
        plx_gfx::gfx::draw_rect(
            0.0,
            0.0,
            plan.width() as f32,
            plan.height() as f32,
            0.0,
            0.0,
            black.as_ptr(),
            black.as_ptr(),
            0.0,
            1.0,
        );
        for slot in plan.slots() {
            let index = usize::from(slot.id);
            let Some(texture) = self.textures[index].as_ref() else {
                continue;
            };
            let (rect, uv) = picture_placement(slot.bounds(), self.display_aspects[index], plan.slots().len());
            if texture.draw_uv(app.adapters.player.mt(), rect, uv, 1.0) == Ok(true)
                && !plx_gfx::gfx::blur_source_pass()
            {
                self.frames[index].submitted(draw_ms);
            }
        }
    }
}
pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    let mut state = std::mem::take(&mut runtime.multiview_local_present);
    state.tick(runtime, app);
    runtime.multiview_local_present = state;
}
/// A multi-window scene paints right up to the shared pixel edge. Crop the texture at
/// its centre rather than expanding the quad, which could overwrite a neighbouring slot.
fn picture_placement(bounds: plx_ui::Rect, aspect: f32, slots: usize) -> (plx_ui::Rect, [f32; 4]) {
    if slots > 1 {
        return (bounds, bounds.cover_uv(aspect, 1.0, plx_gfx::gfx::Crop::Centre));
    }
    // A perimeter with no satellites is a single picture; show the entire frame.
    let h = (bounds.w / aspect).min(bounds.h);
    let w = aspect * h;
    (plx_ui::Rect::new(bounds.cx() - w * 0.5, bounds.cy() - h * 0.5, w, h), plx_gfx::gfx::UV_FULL)
}

pub(super) fn draw(app: &mut App) {
    let Some(mut runtime) = app.ott.take() else {
        return;
    };
    if runtime.multiview_local_present.plan.is_some() {
        let draw_ms = runtime.now_ms();
        runtime.multiview_local_present.draw(app, draw_ms);
    }
    app.ott = Some(runtime);
}
/// Snapshot only the current ticket; a status read can interleave with queued cleanup.
pub(super) fn video_evidence(runtime: &Runtime) -> Option<Value> {
    if !runtime.multiview_local.active() {
        return None;
    }
    let scene = runtime.multiview.scene_status(runtime)?;
    let state = &runtime.multiview_local_present;
    let slots: Vec<_> = scene.slots.iter().map(|row| {
        let i = row.slot.index();
        let frames = state.frame_tickets[i]
            .filter(|&ticket| runtime.multiview.require_current_ticket(ticket).is_ok()
                && (runtime.multiview_local.slot_ticket(row.slot) == Some(ticket)
                    || row.status == "failed"))
            .map_or_else(RenderedFrames::default, |_| state.frames[i]);
        let intervals = frames.draw_intervals();
        json!({"slot_id": i, "slot_generation": row.slot_generation, "source_revision": row.source_revision,
            "drawn_frames": frames.count, "last_pts_us": frames.last_pts_us,
            "draw_interval_p95_ms": intervals.map(|pair| pair.0),
            "draw_interval_max_ms": intervals.map(|pair| pair.1),
            "draw_intervals_ge_250ms_count": frames.intervals_ge_250ms_count,
            "draw_interval_lifetime_max_ms": frames.lifetime_max_ms,
            "last_stop": runtime.multiview_local.last_stop(row.slot)})
    }).collect();
    Some(json!({"version": 1, "scene": scene.scene, "slots": slots}))
}
/// Decoder retirement does not transfer the previous fullscreen lease or output.
/// Keep admission fenced until the presenter's independent custody has settled.
pub(super) fn has_owned_work(runtime: &Runtime) -> bool {
    let state = &runtime.multiview_local_present;
    state.taken
        || state.previous.is_some()
        || state.lease.is_some()
        || state.sink.is_some()
        || state.textures.iter().any(Option::is_some)
        || state.pending_pcm.is_some()
}
pub(super) fn slots_ready(runtime: &Runtime, app: &App) -> bool {
    let state = &runtime.multiview_local_present;
    state.taken
        && state.graphics_ready
        && state.lease.is_none()
        && !state.lease_failed
        && !app.adapters.player.is_live()
        && !app.adapters.player.release_busy()
        && !app.adapters.player.has_pending_release()
        && app.running
        && playback_allowed(app)
        && !app.ott_standby
}
pub(super) fn flush_audio(runtime: &mut Runtime, app: &App) {
    runtime.multiview_local_present.pending_pcm = None;
    if let Some(sink) = runtime.multiview_local_present.sink.as_mut() {
        sink.clear(app.adapters.player.mt());
    }
}

#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    #[test]
    fn mismatched_pictures_fill_their_own_tiles_and_preserve_display_aspect() {
        let tile = plx_ui::Rect::new(961.0, 541.0, 960.0, 540.0);
        for slots in [2, 4, 9] {
            for (aspect, expected) in [
                (4.0 / 3.0, [0.0, 0.125, 1.0, 0.75]),
                (64.0 / 27.0, [0.125, 0.0, 0.75, 1.0]),
                (16.0 / 9.0, plx_gfx::gfx::UV_FULL),
            ] {
                let (rect, uv) = picture_placement(tile, aspect, slots);
                assert_eq!(rect, tile);
                for (actual, wanted) in uv.into_iter().zip(expected) {
                    assert!((actual - wanted).abs() < 1e-6);
                }
                assert!((aspect * uv[2] / uv[3] - rect.w / rect.h).abs() < 1e-6);
            }
        }
        assert_eq!(picture_placement(tile, 4.0 / 3.0, 1),
            (plx_ui::Rect::new(1081.0, 541.0, 720.0, 540.0), plx_gfx::gfx::UV_FULL));
    }

    #[test]
    fn rendered_frames_count_new_successful_draws_only() {
        let mut frames = RenderedFrames::default();
        frames.submitted(0);
        assert_eq!(frames.count, 0);
        frames.pending_pts_us = Some(12);
        frames.pending_pts_us = Some(16); // a newer upload superseded it before draw
        frames.submitted(0);
        frames.submitted(0); // a UI redraw of the same texture
        assert_eq!((frames.count, frames.last_pts_us), (1, Some(16)));
        frames.pending_pts_us = Some(2); // a new picture after seek
        frames.submitted(0);
        assert_eq!((frames.count, frames.last_pts_us), (2, Some(2)));
    }
    #[test]
    fn draw_intervals_measure_new_pictures_and_expire_old_pauses() {
        let mut frames = RenderedFrames::default();
        let mut now = 0u64;
        frames.pending_pts_us = Some(0);
        frames.submitted(now);
        assert_eq!(frames.draw_intervals(), None);
        for i in 1..=33 {
            now += if i == 1 { 900 } else { 40 };
            frames.submitted(now); // ordinary UI redraw
            frames.pending_pts_us = Some(i * 40_000);
            frames.submitted(now);
            if i == 32 { assert_eq!(frames.draw_intervals(), Some((40, 900))); }
        }
        assert_eq!(frames.draw_intervals(), Some((40, 40)));
    }
    #[test]
    fn lifetime_draw_gaps_outlast_the_ring_and_reset_on_a_new_ticket() {
        let _serial = plx_base::testlock::serial();
        use plx_data::ott::multiview::{Layout, Scene};
        let (_, _, _, handles, original, _) = crate::app::ott_runtime::multiview_local::lease_tests::queued_local_attempt();
        let handle = handles[0];
        let slot = original.slot();
        let mut scene = Scene::new(original.scene());
        scene.set_layout(Layout::Grid4).unwrap();
        scene.bind_source(slot, handle.source_ref()).unwrap();
        let first = scene.begin_resolution(slot, handle.source_ref(), handle.media_ref()).unwrap();
        let mut output = State::default();
        assert!(output.update_frame_ticket(slot.index(), Some(first)));
        let frames = &mut output.frames[slot.index()];
        let mut now = 0;
        for interval in [0, 249, 250, 900].into_iter().chain([40; 32]) {
            now += interval;
            frames.submitted(now); // repainting a held texture cannot close a gap
            frames.pending_pts_us = Some(now as i64 * 1_000);
            frames.submitted(now);
        }
        assert_eq!(frames.draw_intervals(), Some((40, 40)));
        assert_eq!(frames.intervals_ge_250ms_count, 2);
        assert_eq!(frames.lifetime_max_ms, Some(900));

        scene.report_pipeline_stopped(first).unwrap();
        scene.bind_source(slot, handle.source_ref()).unwrap();
        let second = scene.begin_resolution(slot, handle.source_ref(), handle.media_ref()).unwrap();
        assert_ne!(first, second);
        assert!(output.update_frame_ticket(slot.index(), Some(second)));
        let frames = &mut output.frames[slot.index()];
        frames.pending_pts_us = Some(0);
        frames.submitted(now + 2_000);
        assert_eq!(frames.intervals_ge_250ms_count, 0);
        assert_eq!(frames.lifetime_max_ms, None);
    }
    #[test]
    fn a_failed_current_window_keeps_its_last_drawn_frame_count_until_stop() {
        let _serial = plx_base::testlock::serial();
        use crate::app::ott_runtime::multiview_local::lease_tests::{queued_local_attempt, resolve_attempt};
        use plx_data::ott::multiview::AdmissionOutcome;
        let (app, mut runtime, mut driver, _, ticket, work) = queued_local_attempt();
        resolve_attempt(&mut runtime, &app, work);
        runtime.multiview.backend_start_completed(ticket, AdmissionOutcome::Admitted).unwrap();
        let index = ticket.slot().index();
        let output = &mut runtime.multiview_local_present;
        assert!(output.update_frame_ticket(index, Some(ticket)));
        output.frames[index].pending_pts_us = Some(41000);
        output.frames[index].submitted(0);
        output.frames[index].pending_pts_us = Some(42000);
        output.frames[index].submitted(300);
        output.frames[index].pending_pts_us = Some(43000);
        assert!(!output.update_frame_ticket(index, None));
        output.frames[index].submitted(900);
        runtime.multiview.playback_failed(ticket).unwrap();
        runtime.multiview_local = driver;
        let failed = &video_evidence(&runtime).unwrap()["slots"][index];
        assert_eq!(failed["drawn_frames"], 2);
        assert_eq!(failed["last_pts_us"], 42000);
        assert_eq!(failed["draw_intervals_ge_250ms_count"], 1);
        assert_eq!(failed["draw_interval_lifetime_max_ms"], 300);
        driver = std::mem::take(&mut runtime.multiview_local);
        driver.stop_slot(&mut runtime, ticket.slot()).unwrap();
        runtime.multiview_local = driver;
        let stopped = &video_evidence(&runtime).unwrap()["slots"][index];
        assert_eq!(stopped["drawn_frames"], 0);
        assert!(stopped["last_pts_us"].is_null());
        assert_eq!(stopped["draw_intervals_ge_250ms_count"], 0);
        assert_eq!(stopped.get("draw_interval_lifetime_max_ms"), Some(&Value::Null));
    }

    fn previous_favorite_fixture() -> (Runtime, Playing, Favorite) {
        use plx_data::ott::favorites::{Locator, Origin};
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.loaded = true;
        runtime.state_load = StateLoad::Idle;
        runtime.entropy = Some([7; 16]);
        runtime.resume_source_work();
        for (id, name) in [("active", "Active"), ("other", "Other")] {
            runtime.saved.profiles.profiles.push(ProviderProfile {
                id: id.into(),
                name: name.into(),
                kind: ProviderKind::M3u,
                source: format!("https://source.invalid/{id}.m3u"),
                source_alias: name.into(),
                enabled: true,
            });
        }
        runtime.saved.profiles.active_profile = Some("active".into());
        runtime.provider = ProviderKind::M3u;
        let favorite = Favorite {
            title: "Original movie".into(),
            origin: Origin {
                provider: "m3u".into(),
                profile: "Other".into(),
                subprofile: None,
                source_alias: "Other".into(),
            },
            locator: Locator::ChannelName {
                name: "Original movie".into(),
                group: Some("Movies".into()),
            },
        };
        runtime.saved.favorites.toggle(favorite.clone()).unwrap();
        let entry = CatalogEntry {
            id: "resolved-movie".into(),
            title: "Original movie".into(),
            category: "Movies".into(),
            description: String::new(),
            poster: String::new(),
            synopsis: None,
            protection: Some(false),
            media: MediaLocator::Http {
                url: "https://source.invalid/movie.mp4".into(),
            },
            duration_ms: Some(120_000),
            provider_id: None,
            provider_id_field: None,
            live: None,
        };
        let playing = Playing {
            channel_capture: None,
            kiosk_selection: None,
            archive: None,
            transient_owner: None,
            local_selection: None,
            identity: None,
            profile: runtime.saved.profiles.profiles[1].clone(),
            entry,
            selection_id: format!("favorite:{}", favorite.identity()),
            source: None,
            advance_on_failure: false,
            requires_server_lease: false,
        };
        (runtime, playing, favorite)
    }
    #[test]
    fn previous_favorite_from_inactive_profile_uses_fresh_ordinary_binding_without_provider_mutation(
    ) {
        let _serial = plx_base::testlock::serial();
        let (runtime, playing, favorite) = previous_favorite_fixture();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let previous =
            Previous::capture(&runtime, &playing, 12_345).expect("ordinary Favorite capture");
        let prepared = previous
            .prepared(&runtime)
            .expect("inactive Favorite remains bound");
        assert_eq!(prepared.0.id, format!("favorite:{}", favorite.identity()));
        assert_eq!(prepared.0.position_ms, 12_345);
        assert!(prepared.1 == playing.profile);
        assert!(prepared.2.is_none());
        assert_eq!(prepared.3, Some(favorite));
        assert!(runtime.effective_profile() != Some(&prepared.1));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(
            runtime.play_target.is_none()
                && runtime.resolver_next.is_none()
                && runtime.playing.is_none()
        );
    }
    #[test]
    fn previous_favorite_rejects_changed_definition_removed_favorite_and_revoked_profile() {
        let _serial = plx_base::testlock::serial();
        for scenario in 0..3 {
            let (mut runtime, playing, favorite) = previous_favorite_fixture();
            let previous = Previous::capture(&runtime, &playing, 5_000).unwrap();
            match scenario {
                0 => {
                    runtime.saved.favorites.remove(&favorite.identity());
                }
                1 => {
                    runtime.saved.favorites.remove(&favorite.identity());
                    let mut changed = favorite.clone();
                    changed.title = "Changed display identity".into();
                    runtime.saved.favorites.toggle(changed).unwrap();
                }
                _ => runtime.saved.profiles.profiles[1].enabled = false,
            }
            assert!(
                previous.prepared(&runtime).is_err(),
                "changed/revoked Favorite must not restore"
            );
            assert_eq!(
                runtime.saved.profiles.active_profile.as_deref(),
                Some("active")
            );
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        }
    }

    #[test]
    fn retired_slots_cannot_release_presenter_custody_to_a_new_fullscreen_source() {
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        assert!(!runtime.multiview_local.active());
        assert!(!runtime.multiview_local.has_work());
        runtime.multiview_local_present.taken = true;
        runtime.multiview_local_present.previous = Some(Previous {
            favorite: None,
            request: PlaybackRequest::ordinary("old-source", 0),
            profile: ProviderProfile {
                id: "old".into(),
                name: "Old".into(),
                kind: ProviderKind::M3u,
                source: "https://source.invalid/old.m3u".into(),
                source_alias: "Old".into(),
                enabled: true,
            },
            entry: CatalogEntry {
                id: "old-source".into(),
                title: "Old".into(),
                category: String::new(),
                description: String::new(),
                poster: String::new(),
                synopsis: None,
                protection: Some(false),
                media: MediaLocator::Http {
                    url: "https://source.invalid/old.m3u8".into(),
                },
                duration_ms: None,
                provider_id: None,
                provider_id_field: None,
                live: None,
            },
        });
        assert!(has_owned_work(&runtime));
        let result = runtime.play_prepared_request_with_quality_policy(
            &mut app,
            (
                PlaybackRequest::ordinary("new-source", 0),
                ProviderProfile {
                    id: "fresh".into(),
                    name: "Fresh".into(),
                    kind: ProviderKind::M3u,
                    source: "https://source.invalid/list.m3u".into(),
                    source_alias: "Fresh".into(),
                    enabled: true,
                },
                None,
                None,
            ),
            false,
        );
        assert_eq!(
            result,
            Err("Close multiview before starting fullscreen playback")
        );
        assert!(runtime.play_target.is_none());
        // Complete the old output retirement through the real presenter pump.
        // Cancellation/background retirement is not an explicit Off restoration.
        tick(&mut runtime, &mut app);
        assert!(!has_owned_work(&runtime));
        assert!(runtime.play_target.is_none());
        runtime.play_target = Some(PlaybackRequest::ordinary("new-source", 0));
        tick(&mut runtime, &mut app);
        assert_eq!(
            runtime.play_target.as_ref().map(|r| r.id.as_str()),
            Some("new-source")
        );
        runtime.cancel_play_request();
        tick(&mut runtime, &mut app);
        assert!(runtime.play_target.is_none());
    }
}

/// Callers update app.player.volume first. No native fallback or second preference copy.
pub(super) fn apply_volume(runtime: &mut Runtime, app: &App) -> bool {
    let mut state = std::mem::take(&mut runtime.multiview_local_present);
    let applied = state.apply_volume(runtime, app);
    runtime.multiview_local_present = state;
    applied
}

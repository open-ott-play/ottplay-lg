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
pub(super) struct State {
    taken: bool,
    graphics_ready: bool,
    previous: Option<Previous>,
    lease: Option<LeaseClose>,
    lease_failed: bool,
    textures: [Option<Texture>; 9],
    tickets: [Option<ResolveTicket>; 9],
    sizes: [(u32, u32); 9],
    plan: Option<RasterPlan>,
    sink: Option<Sink>,
    audio: Option<(SlotId, ResolveTicket, OwnerToken)>,
    serial: u64,
    error: Option<&'static str>,
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
            tickets: [None; 9],
            sizes: [(0, 0); 9],
            plan: None,
            sink: None,
            audio: None,
            serial: 0,
            error: None,
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
        if runtime.multiview_local.audio_owner() != Some(slot)
            || runtime.multiview_local.slot_ticket(slot) != Some(ticket)
        {
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
        self.tickets = [None; 9];
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
            .map(|p| Previous {
                request: PlaybackRequest {
                    kiosk_selection: p.kiosk_selection.clone(),
                    archive: p.archive.clone(),
                    transient_owner: p.transient_owner,
                    local_selection: p.local_selection.clone(),
                    continuation: None,
                    id: p.selection_id.clone(),
                    position_ms: if guide_ui::is_live(&p.profile, &p.entry) {
                        0
                    } else {
                        (playback::intended_pos(&app.player.session).max(0) as u64) / 1_000_000
                    },
                    advance_on_failure: false,
                },
                profile: p.profile.clone(),
                entry: p.entry.clone(),
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
                        // Re-enter the ordinary policy/resolver path; never replay a captured URL.
                        // A changed profile/catalogue cannot accidentally reinterpret its old id.
                        let current = runtime.effective_profile() == Some(&previous.profile)
                            && runtime
                                .catalog
                                .entries
                                .iter()
                                .any(|entry| entry == &previous.entry);
                        if !current
                            || runtime
                                .play_request_with_policy(app, previous.request)
                                .is_err()
                        {
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
            if ticket != self.tickets[index] {
                self.textures[index] = None;
                self.tickets[index] = ticket;
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
                if let Ok(t) = runtime.multiview.close_slot(slot) {
                    runtime.multiview_effects.extend(t.effects);
                }
                runtime.changed();
                self.textures[index] = None;
            } else {
                self.sizes[index] = (frame.width, frame.height);
            }
        }
        if app.adapters.player.release_busy() || self.lease.is_some() || self.lease_failed {
            if let Some(sink) = self.sink.as_mut() {
                sink.clear(app.adapters.player.mt());
            }
            return;
        }
        if let Some((_scene, _revision, owner)) = runtime.multiview_local.audio_request() {
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
            let next = owner.and_then(|slot| {
                runtime.multiview_local.slot_ticket(slot).map(|ticket| {
                    (
                        slot,
                        ticket,
                        OwnerToken {
                            scene: self.serial,
                            slot: slot.index() as u8,
                            generation: self.serial,
                        },
                    )
                })
            });
            let result = if owner.is_none() {
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
            driver.confirm_audio(runtime, accepted);
            runtime.multiview_local = driver;
        }
        if self.audio.is_some_and(|(slot, ticket, _)| {
            runtime.multiview_local.audio_owner() != Some(slot)
                || runtime.multiview_local.slot_ticket(slot) != Some(ticket)
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
            let Some((slot, ticket, chunk)) = self
                .pending_pcm
                .take()
                .or_else(|| runtime.multiview_local.take_audio())
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
    fn draw(&self, app: &App) {
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
            let (w, h) = self.sizes[index];
            if w == 0 || h == 0 {
                continue;
            }
            let bounds = slot.bounds();
            let scale = (bounds.w / w as f32).min(bounds.h / h as f32);
            let rect = plx_ui::Rect::new(
                bounds.x + (bounds.w - w as f32 * scale) / 2.0,
                bounds.y + (bounds.h - h as f32 * scale) / 2.0,
                w as f32 * scale,
                h as f32 * scale,
            );
            let _ = texture.draw(app.adapters.player.mt(), rect, 1.0);
        }
    }
}
pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    let mut state = std::mem::take(&mut runtime.multiview_local_present);
    state.tick(runtime, app);
    runtime.multiview_local_present = state;
}
pub(super) fn draw(app: &mut App) {
    let Some(runtime) = app.ott.take() else {
        return;
    };
    runtime.multiview_local_present.draw(app);
    app.ott = Some(runtime);
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
    fn retired_slots_cannot_release_presenter_custody_to_a_new_fullscreen_source() {
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        assert!(!runtime.multiview_local.active());
        assert!(!runtime.multiview_local.has_work());
        runtime.multiview_local_present.taken = true;
        runtime.multiview_local_present.previous = Some(Previous {
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

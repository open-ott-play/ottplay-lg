//! Existing closed scene/slot CAS wire, backed only by this device's slots.
use super::*;
use crate::app::ott_remote::multiview as wire;
use plx_data::ott::multiview::AudioPolicy;

pub(super) struct SourceReceipt {
    token: String,
    context: wire::Context,
    slot: wire::SlotBinding,
    handle: multiview_sources::SourceHandle,
    expires: u64,
}
fn context(runtime: &Runtime) -> wire::Context {
    runtime.multiview.scene_status(runtime).map_or(
        wire::Context {
            scene: "off".into(),
            layout_revision: "off".into(),
            focus_revision: "off".into(),
            audio_revision: "off".into(),
        },
        |s| wire::Context {
            scene: s.scene,
            layout_revision: s.layout_revision,
            focus_revision: s.focus_revision,
            audio_revision: s.audio_revision,
        },
    )
}
fn slot_current(runtime: &Runtime, slot: &wire::SlotBinding) -> bool {
    runtime.multiview.scene_status(runtime).is_some_and(|s| {
        s.slots.iter().any(|r| {
            r.slot.index() == usize::from(slot.slot_id)
                && r.slot_generation == slot.slot_generation
                && r.source_revision == slot.source_revision
        })
    })
}
fn map_layout(layout: wire::Layout) -> Layout {
    match layout {
        wire::Layout::Single => Layout::Single,
        wire::Layout::Pip => Layout::PictureInPicture,
        wire::Layout::Grid4 => Layout::Grid4,
        wire::Layout::Grid9 => Layout::Grid9,
        wire::Layout::Perimeter { satellites } => Layout::Perimeter { satellites },
    }
}
fn reason(error: multiview::Error) -> wire::Reason {
    match error {
        multiview::Error::BackendPending | multiview::Error::SceneClosing => wire::Reason::Busy,
        multiview::Error::SourceStale | multiview::Error::SourceMissing => {
            wire::Reason::SourceChanged
        }
        multiview::Error::KioskRestricted | multiview::Error::NotReady => {
            wire::Reason::PolicyRestricted
        }
        _ => wire::Reason::Unavailable,
    }
}
pub(super) fn eligible(runtime: &Runtime, app: &App) -> bool {
    app.running
        && runtime.multiview_driver.shutdown_confirmed()
        && !super::super::multiview_output::has_owned_work(runtime)
        && runtime.multiview_local_profile_epoch != 0
        && runtime.multiview_authority_epoch != 0
        && playback_allowed(app)
        && runtime.loaded
        && !runtime.persistence_uncertain
        && runtime.play_work_allowed()
        && input_policy::navigation(runtime).is_ok()
        && matches!(
            runtime.track_selection_kiosk(),
            Ok(crate::app::ott_kiosk_policy::State::Off)
        )
}
impl Driver {
    pub fn status(&self, runtime: &Runtime) -> Value {
        let c = context(runtime);
        let cx = json!({"scene":c.scene,"layout_revision":c.layout_revision,"focus_revision":c.focus_revision,"audio_revision":c.audio_revision});
        let Some(s) = runtime.multiview.scene_status(runtime) else {
            return json!({"context":cx,"layout":"off","satellites":0,"focus_slot":null,"audio_policy":"muted","audio_slot":null,"slots":[]});
        };
        let (layout, satellites) = match runtime.multiview.layout().unwrap_or(Layout::Single) {
            Layout::Single => ("single", 0),
            Layout::PictureInPicture => ("pip", 0),
            Layout::Grid4 => ("grid4", 0),
            Layout::Grid9 => ("grid9", 0),
            Layout::Perimeter { satellites } => ("perimeter", satellites),
        };
        let audio = match runtime.multiview.audio_policy() {
            Some(AudioPolicy::FollowsFocus) => "follows_focus",
            Some(AudioPolicy::Explicit(Some(_))) if self.audio_applied.is_some() => "slot",
            _ => "muted",
        };
        json!({"context":cx,"layout":layout,"satellites":satellites,"focus_slot":runtime.multiview.active_slot().map(|s|s.index()),
            "audio_policy":audio,"audio_slot":self.audio_applied.map(|s|s.index()),
            "slots":s.slots.iter().map(|s|json!({"slot_id":s.slot.index(),"slot_generation":s.slot_generation,
                "source_revision":s.source_revision,"state":s.status,"error":s.error,
                "source":s.source.as_ref().map(|v|json!({"source_id":v.source_id,"label":v.label,"provider":v.provider,"profile":v.profile,"kind":v.kind}))})).collect::<Vec<_>>()})
    }
    pub fn capability_markers(&self, runtime: &Runtime, app: &App) -> (Value, Value) {
        let supported = eligible(runtime, app)
            && (self.active() || !super::super::multiview_local_present::has_owned_work(runtime))
            && !self.closing
            && (!self.cleanup_failed || !self.active())
            && plx_media::local_multiview::available();
        let admission = if supported {
            json!({"version":1,"operations":["status","sources","layout","play","focus","audio","stop","off"],
                "layouts":["single","pip","grid4","grid9","perimeter"],"max_slots":9,
                "source_kinds":["live","vod","file"],"audio_policies":["follows_focus","muted","slot"],"off_modes":["restore_previous"]})
        } else {
            json!({"version":1,"operations":["status"],"layouts":[],"max_slots":0,"source_kinds":[],"audio_policies":[],"off_modes":[]})
        };
        (
            admission,
            json!({"version":1,"runtime":runtime.remote.runtime(),"operations":["status"],"off_modes":[]}),
        )
    }
    pub fn command(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        invocation: &Invocation,
    ) -> Option<Value> {
        if invocation.action != "multiview" {
            return None;
        }
        let request = wire::Request::decode(invocation.multiview_projection()?).ok()?;
        let reply_runtime = runtime.remote.runtime().to_owned();
        let rejected = |reason| request.rejected(&reply_runtime, reason).ok();
        if !runtime.remote.invocation_is_current(invocation)
            || runtime.now_ms() >= invocation.deadline_ms
        {
            return rejected(wire::Reason::Expired);
        }
        if input_policy::remote(runtime, invocation).is_err() {
            return rejected(wire::Reason::PolicyRestricted);
        }
        if request
            .runtime()
            .is_some_and(|r| r != runtime.remote.runtime())
        {
            return rejected(wire::Reason::RuntimeChanged);
        }
        if matches!(request.operation(), wire::Operation::Status) {
            return request
                .status_response(runtime.remote.runtime(), self.status(runtime))
                .ok();
        }
        if request.recovery() || !plx_media::local_multiview::available() {
            return rejected(wire::Reason::Unsupported);
        }
        if !eligible(runtime, app) {
            return rejected(wire::Reason::PolicyRestricted);
        }
        if self.closing || runtime.multiview_effects.len() > 56 {
            return rejected(wire::Reason::Busy);
        }
        let current = context(runtime);
        if let Some(want) = request.expected() {
            let mismatch = if want.scene != current.scene {
                Some(wire::Reason::SceneChanged)
            } else if want.layout_revision != current.layout_revision {
                Some(wire::Reason::LayoutChanged)
            } else if want.focus_revision != current.focus_revision {
                Some(wire::Reason::FocusChanged)
            } else if want.audio_revision != current.audio_revision {
                Some(wire::Reason::AudioChanged)
            } else {
                None
            };
            if let Some(reason) = mismatch {
                return rejected(reason);
            }
        }
        if request
            .slot()
            .is_some_and(|slot| !slot_current(runtime, slot))
        {
            return rejected(wire::Reason::SlotChanged);
        }
        let mut source_reply = None;
        let result: Result<(), wire::Reason> = (|| {
            match request.operation() {
                wire::Operation::Sources { slot, filters } => {
                    let mut coordinator = std::mem::take(&mut runtime.multiview);
                    let refreshed = coordinator.refresh_local_sources(runtime);
                    runtime.multiview = coordinator;
                    refreshed.map_err(reason)?;
                    let offset = match filters.cursor.as_deref() {
                        Some(cursor) => cursor
                            .strip_prefix("localpage_")
                            .and_then(|p| p.parse::<usize>().ok())
                            .filter(|n| *n <= 1152)
                            .ok_or(wire::Reason::InvalidRequest)?,
                        None => 0,
                    };
                    self.sources
                        .retain(|s| s.expires > runtime.now_ms() && s.context == current);
                    let mut items = Vec::new();
                    let options = runtime
                        .multiview
                        .source_options()
                        .into_iter()
                        .filter(|s| {
                            filters.provider.as_ref().is_none_or(|p| p == s.provider)
                                && filters.profile.as_ref().is_none_or(|p| p == &s.profile)
                                && filters.query.as_ref().is_none_or(|q| {
                                    s.label.to_lowercase().contains(&q.to_lowercase())
                                })
                        })
                        .collect::<Vec<_>>();
                    let next = (offset.saturating_add(128) < options.len())
                        .then(|| format!("localpage_{}", offset + 128));
                    for source in options.into_iter().skip(offset).take(128) {
                        let Some(handle) = runtime.multiview.source_handle(&source.source_id)
                        else {
                            continue;
                        };
                        self.serial = self
                            .serial
                            .checked_add(1)
                            .ok_or(wire::Reason::LimitExceeded)?;
                        let token = format!("local_source_{}", self.serial);
                        items.push(json!({"source_id":source.source_id,"label":source.label,"provider":source.provider,"profile":source.profile,"kind":source.kind,"ticket":token}));
                        self.sources.push(SourceReceipt {
                            token,
                            context: current.clone(),
                            slot: slot.clone(),
                            handle,
                            expires: invocation
                                .deadline_ms
                                .min(runtime.now_ms().saturating_add(30_000)),
                        });
                    }
                    while self.sources.len() > 256 {
                        self.sources.remove(0);
                    }
                    return request
                        .sources_response(runtime.remote.runtime(), items, next.as_deref())
                        .map(|value| {
                            // This command has no queued operation; the outer reply below
                            // is replaced by the exact validated sources response.
                            value
                        })
                        .map(|value| {
                            source_reply = Some(value);
                        })
                        .map_err(|_| wire::Reason::Unavailable);
                }
                wire::Operation::Layout(layout) => {
                    let layout = map_layout(*layout);
                    self.set_layout(runtime, app, layout).map_err(reason)?;
                }
                wire::Operation::Play {
                    slot,
                    source_ticket,
                } => {
                    let index = self
                        .sources
                        .iter()
                        .position(|s| {
                            &s.token == source_ticket
                                && s.context == current
                                && s.slot == *slot
                                && s.expires > runtime.now_ms()
                        })
                        .ok_or(wire::Reason::SourceExpired)?;
                    let receipt = self.sources.remove(index);
                    self.play_source(
                        runtime,
                        app,
                        SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?,
                        receipt.handle,
                    )
                    .map_err(reason)?;
                }
                wire::Operation::Focus(slot) => {
                    let t = runtime
                        .multiview
                        .focus(SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?)
                        .map_err(reason)?;
                    runtime.multiview_effects.extend(t.effects);
                }
                wire::Operation::Audio(policy) => {
                    let policy = match policy {
                        wire::AudioPolicy::FollowsFocus => AudioPolicy::FollowsFocus,
                        wire::AudioPolicy::Muted => AudioPolicy::Explicit(None),
                        wire::AudioPolicy::Slot(slot) => {
                            AudioPolicy::Explicit(SlotId::new(slot.slot_id))
                        }
                    };
                    let t = runtime.multiview.set_audio_policy(policy).map_err(reason)?;
                    runtime.multiview_effects.extend(t.effects);
                }
                wire::Operation::Stop(slot) => {
                    let t = runtime
                        .multiview
                        .close_slot(SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?)
                        .map_err(reason)?;
                    runtime.multiview_effects.extend(t.effects);
                }
                wire::Operation::Off(wire::OffMode::RestorePrevious) => {
                    self.close_restore_previous(runtime, app).map_err(reason)?;
                }
                _ => return Err(wire::Reason::Unsupported),
            }
            Ok(())
        })();
        match result {
            Ok(()) => source_reply.or_else(|| {
                request
                    .accepted(runtime.remote.runtime(), wire::Outcome::Requested)
                    .ok()
            }),
            Err(reason) => request.rejected(runtime.remote.runtime(), reason).ok(),
        }
    }
}

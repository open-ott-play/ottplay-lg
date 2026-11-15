//! The app owns scene selection. Player screens can only emit an offer-bound
//! event; they cannot choose a source, claim a decoder or change audio directly.
use super::*;

const EFFECT_LIMIT: usize = 64;
// Highlight emits no backend command. Activation emits at most Focus+AudioOwner.
const UI_EFFECT_RESERVE: usize = 2;

pub(super) fn player_action(app: &mut App, event: plx_appkit::multiview_chrome::Event) {
    let Some(mut runtime) = app.ott.take() else {
        app.bridge.publish_multiview_offer(None);
        return;
    };
    // Consuming any offer edge invalidates its issuance, including a busy or
    // stale refusal. A later fresh publication must not be hidden by the cache.
    runtime.multiview_ui.published = None;
    let local = runtime.multiview_local.active();
    let owned = if local {
        runtime.multiview_local.owns_ui(&runtime, app)
    } else {
        runtime.multiview_picture.as_ref().is_some_and(|identity| {
            plx_media::route::playback_identity_matches(&app.player.session, identity)
        })
    };
    if !owned {
        app.bridge.publish_multiview_offer(None);
        app.ott = Some(runtime);
        return;
    }
    if runtime.multiview_effects.len() > EFFECT_LIMIT - UI_EFFECT_RESERVE {
        app.bridge.publish_multiview_offer(None);
        runtime.status = "Multiview control is busy".into();
        app.ott = Some(runtime);
        return;
    }
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    let result = coordinator.accept_ui_event(&runtime, app, event);
    runtime.multiview = coordinator;
    match result {
        Ok(transition) => {
            // Only typed, owner-validated effects enter the bounded driver queue.
            // The driver still checks source and backend authority at execution.
            if transition.effects.len() <= UI_EFFECT_RESERVE {
                runtime.multiview_effects.extend(transition.effects);
                runtime.changed();
            } else {
                runtime.status = "Multiview control is unavailable".into();
            }
        }
        Err(_) => {
            runtime.status = "Multiview selection is no longer current".into();
        }
    }
    // One-shot offers are replaced only from a fresh owner publication.
    app.bridge.publish_multiview_offer(None);
    app.ott = Some(runtime);
}

#[derive(Default)]
pub(super) struct State {
    published: Option<Publication>,
}
#[derive(PartialEq, Eq)]
struct Publication {
    scene: multiview::SceneStatus,
    geometry: plx_appkit::multiview_layout::RasterPlan,
    focused: Option<plx_data::ott::multiview::SlotId>,
    audio: Option<plx_data::ott::multiview::SlotId>,
}

/// Publish controls only for the current local scene or an admitted service
/// picture. Both paths use the same raster plan as their respective renderer.
pub(super) fn publish(runtime: &mut Runtime, app: &mut App) {
    let publication = (|| {
        let geometry = if runtime.multiview_local.active() {
            if !runtime.multiview_local.owns_ui(runtime, app) {
                return None;
            }
            runtime.multiview_local.geometry(
                plx_base::surface::LOGICAL_W as u32,
                plx_base::surface::LOGICAL_H as u32,
            )?
        } else {
            if !multiview_output::owns_picture(runtime, app) {
                return None;
            }
            backend_plan(runtime)?
        };
        Some(Publication {
            scene: runtime.multiview.scene_status(runtime)?,
            geometry,
            focused: runtime.multiview.focused_slot(),
            audio: runtime.multiview.audio_route_state().applied,
        })
    })();
    let Some(publication) = publication else {
        runtime.multiview_ui.published = None;
        app.bridge.publish_multiview_offer(None);
        return;
    };
    if runtime.multiview_ui.published.as_ref() == Some(&publication)
        && app.bridge.multiview_offer().is_some()
    {
        return;
    }
    use plx_appkit::multiview_chrome as chrome;
    let plan = &publication.geometry;
    let Ok(offer) = runtime.multiview.publish_ui_offer() else {
        app.bridge.publish_multiview_offer(None);
        return;
    };
    let views: Vec<_> = publication
        .scene
        .slots
        .iter()
        .map(|slot| chrome::SlotView {
            id: slot.slot.index() as u8,
            title: slot
                .source
                .as_ref()
                .map(|s| s.label.clone())
                .unwrap_or_default(),
            state: match slot.status {
                "playing" | "paused" | "stalled" => chrome::SlotState::Playing,
                "starting" => chrome::SlotState::Loading,
                "failed" => chrome::SlotState::Failed,
                _ => chrome::SlotState::Empty,
            },
        })
        .collect();
    let state = chrome::ViewState {
        revision: offer.ui_serial,
        geometry_revision: plan.revision(),
        active: runtime.multiview.active_slot().map(|s| s.index() as u8),
        audio: publication.audio.map(|s| s.index() as u8),
        focused: publication.focused.map(|s| s.index() as u8),
        input_owned: true,
    };
    match project_frame(plan, &views, state) {
        Ok(frame) => {
            app.bridge.publish_multiview_offer(Some(frame));
            runtime.multiview_ui.published = Some(publication);
        }
        Err(_) => app.bridge.publish_multiview_offer(None),
    }
}

fn backend_plan(runtime: &Runtime) -> Option<plx_appkit::multiview_layout::RasterPlan> {
    use plx_appkit::multiview_layout as geometry;
    use plx_data::ott::multiview::Layout;
    let mode = match runtime.multiview.layout() {
        Some(Layout::Single) => geometry::Mode::Perimeter { satellites: 0 },
        Some(Layout::PictureInPicture) => geometry::Mode::Pip,
        Some(Layout::Grid4) => geometry::Mode::Grid4,
        Some(Layout::Grid9) => geometry::Mode::Grid9,
        Some(Layout::Perimeter { satellites }) => geometry::Mode::Perimeter { satellites },
        None => {
            return None;
        }
    };
    let g = runtime.multiview_driver.admitted_geometry(runtime)?;
    let plan = geometry::RasterPlan::checked(
        g.revision,
        g.canvas.width,
        g.canvas.height,
        mode,
        g.rects
            .iter()
            .map(|r| geometry::RasterSlot {
                id: r.slot_id,
                layer: r.z,
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
            .collect(),
    );
    plan.ok()
}

fn project_frame(
    plan: &plx_appkit::multiview_layout::RasterPlan,
    views: &[plx_appkit::multiview_chrome::SlotView],
    state: plx_appkit::multiview_chrome::ViewState,
) -> Result<plx_appkit::multiview_chrome::ChromeFrame, plx_appkit::multiview_chrome::ChromeError> {
    plx_appkit::multiview_chrome::ChromeFrame::from_raster_in(
        plan,
        views,
        state,
        1.0,
        plx_ui::Rect::FULL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_appkit::{multiview_chrome as chrome, multiview_layout as geometry};
    #[test]
    fn local_common_geometry_keeps_grid_video_and_pointer_slots_aligned() {
        for mode in [geometry::Mode::Grid4, geometry::Mode::Grid9] {
            let plan = geometry::Layout::new(plx_ui::Rect::FULL, mode, 1.0)
                .unwrap()
                .rasterize_even(7)
                .unwrap();
            let state = chrome::ViewState {
                revision: 11,
                geometry_revision: 7,
                input_owned: true,
                ..Default::default()
            };
            let frame = project_frame(&plan, &[], state).unwrap();
            let pointer =
                geometry::PointerMap::contain(plx_ui::Rect::FULL, plx_ui::Rect::FULL).unwrap();
            for slot in frame.slots() {
                let r = slot.bounds;
                assert_eq!(
                    frame.pointer(
                        &pointer,
                        geometry::PointerAction::PrimaryClick,
                        r.x + r.w / 2.0,
                        r.y + r.h / 2.0
                    ),
                    Some(chrome::Event::ActivateSlot {
                        revision: 11,
                        slot: slot.id
                    })
                );
            }
            assert_eq!(frame.slots().len(), plan.slots().len());
        }
    }
    #[test]
    fn changed_geometry_cannot_publish_stale_pointer_offer() {
        let plan = geometry::Layout::new(plx_ui::Rect::FULL, geometry::Mode::Grid4, 1.0)
            .unwrap()
            .rasterize_even(8)
            .unwrap();
        assert!(project_frame(
            &plan,
            &[],
            chrome::ViewState {
                revision: 11,
                geometry_revision: 7,
                input_owned: true,
                ..Default::default()
            }
        )
        .is_err());
    }
}

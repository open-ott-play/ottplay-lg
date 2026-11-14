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
    if !runtime.multiview_picture.as_ref().is_some_and(|identity| {
        plx_media::route::playback_identity_matches(&app.player.session, identity)
    }) {
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
    geometry: multiview_service::Geometry,
    focused: Option<plx_data::ott::multiview::SlotId>,
    audio: Option<plx_data::ott::multiview::SlotId>,
}

/// Only an admitted native picture can publish controls. Rectangles come from
/// the verified backend receipt, then project once onto the actual UI surface.
pub(super) fn publish(runtime: &mut Runtime, app: &mut App) {
    let publication = (|| {
        if !multiview_output::owns_picture(runtime, app) {
            return None;
        }
        Some(Publication {
            scene: runtime.multiview.scene_status(runtime)?,
            geometry: runtime.multiview_driver.admitted_geometry(runtime)?.clone(),
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
    use plx_appkit::{multiview_chrome as chrome, multiview_layout as geometry};
    use plx_data::ott::multiview::Layout;
    let mode = match runtime.multiview.layout() {
        Some(Layout::Single) => geometry::Mode::Perimeter { satellites: 0 },
        Some(Layout::PictureInPicture) => geometry::Mode::Pip,
        Some(Layout::Grid4) => geometry::Mode::Grid4,
        Some(Layout::Grid9) => geometry::Mode::Grid9,
        Some(Layout::Perimeter { satellites }) => geometry::Mode::Perimeter { satellites },
        None => {
            app.bridge.publish_multiview_offer(None);
            return;
        }
    };
    let g = &publication.geometry;
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
    let Ok(plan) = plan else {
        app.bridge.publish_multiview_offer(None);
        return;
    };
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
        geometry_revision: g.revision,
        active: runtime.multiview.active_slot().map(|s| s.index() as u8),
        audio: publication.audio.map(|s| s.index() as u8),
        focused: publication.focused.map(|s| s.index() as u8),
        input_owned: true,
    };
    match chrome::ChromeFrame::from_raster_in(&plan, &views, state, 1.0, plx_ui::Rect::FULL) {
        Ok(frame) => {
            app.bridge.publish_multiview_offer(Some(frame));
            runtime.multiview_ui.published = Some(publication);
        }
        Err(_) => app.bridge.publish_multiview_offer(None),
    }
}

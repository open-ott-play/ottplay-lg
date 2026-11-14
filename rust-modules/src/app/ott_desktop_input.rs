//! Physical desktop Quit plus Mac playback keys; TV and injected keys keep their map.
use super::*;
use crate::app::events;
use plx_ui::consts::{SDLK_DOWN, SDLK_LEFT, SDLK_RIGHT, SDLK_UP};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Volume(i32),
    Mute,
    Seek(i64),
    Step(i64),
    Favorite,
    Random,
    Menu,
}

fn global_decoder_transport(action: Action) -> bool {
    matches!(action, Action::Seek(_) | Action::Step(_))
}

const QUIT_KEY: u32 = b'q' as u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuitEdge { Down { plain: bool }, Repeat, Up }

/// Read actual stock SDL identity before generic q→Back classification loses its origin.
/// Modifiers never identify remote injection; only the dedicated carrier marker does.
fn physical_quit(bytes: &[u8]) -> Option<QuitEdge> {
    if !cfg!(feature = "hostsim") || bytes.len() < 28
        || events::rd_u32(bytes, 8) == events::SYNTH_WINDOW
        || events::rd_u32(bytes, 20) != QUIT_KEY {
        return None;
    }
    let modifiers = u16::from_ne_bytes([bytes[24], bytes[25]]);
    // Caps/Num lock change text case, not the shortcut. Shift/Ctrl/Alt/GUI (and
    // other active modifiers) do not become an application-quit gesture.
    let plain = modifiers & !0x3000 == 0;
    match (events::rd_u32(bytes, 0), bytes[12], bytes[13]) {
        (0x300, 1, 0) => Some(QuitEdge::Down { plain }),
        (0x300, 1, _) => Some(QuitEdge::Repeat),
        (0x301, 0, _) => Some(QuitEdge::Up),
        _ => None,
    }
}

/// Release retirement must run before standby/replay/kiosk fences. Ownership is
/// App-local: changing/removing the optional OTT runtime cannot lose a consumed Q.
fn quit_boundary(app: &mut App) -> bool {
    match physical_quit(&app.ev) {
        Some(QuitEdge::Up) => {
            let owned = std::mem::take(&mut app.desktop_quit_held);
            if owned && app.down_sym == QUIT_KEY { app.down_sym = 0; }
            owned
        },
        Some(QuitEdge::Repeat) => app.desktop_quit_held,
        Some(QuitEdge::Down { .. }) => {
            app.desktop_quit_held = false; // a genuine new gesture retires a lost key-up
            false
        },
        None => false,
    }
}

fn consume_quit(app: &mut App, edge: QuitEdge, action: DesktopQuit) -> DesktopQuit {
    if matches!(edge, QuitEdge::Down { .. }) { app.desktop_quit_held = true; }
    app.last_input = clock::now();
    app.rec.refuse("Physical desktop Quit is not bound in this recording format");
    action
}

/// Called after existing kiosk, boot, replay and standby admission. Text edges
/// are consumed while SDL_TEXTINPUT follows its existing single insertion path.
/// Search remains text-capable across its field activation boundary: the shared
/// text-owner predicate intentionally reserves the whole Search page.
pub(super) fn quit(app: &mut App) -> DesktopQuit {
    let Some(edge) = physical_quit(&app.ev) else { return DesktopQuit::Pass; };
    let text_owned = bridge::text_owns_input(&app.pages)
        || app.pages.input.keyboard || app.pages.input.keyboard_owner.is_some()
        || app.ott.as_ref().is_some_and(|runtime| runtime.edit_session.is_some());
    if text_owned {
        // Shift+Q is still SDL sym=q with a later TEXTINPUT("Q"). Checking
        // modifiers first would send Back and dismiss the field before insertion.
        return consume_quit(app, edge, DesktopQuit::Consume);
    }
    match edge {
        QuitEdge::Up => return DesktopQuit::Pass,
        QuitEdge::Repeat => return consume_quit(app, edge, DesktopQuit::Consume),
        QuitEdge::Down { plain: false } => return DesktopQuit::Pass,
        QuitEdge::Down { plain: true } => {},
    }
    if !app.inputs.is_empty() {
        // Earlier input in this SDL batch may mount a field/modal next frame.
        // Do not overtake it with Quit or enqueue a Back for its future owner.
        return consume_quit(app, edge, DesktopQuit::Consume);
    }
    if app.pages.surface_up() || app.pages.has_pending_navigation()
        || app.pages.input.arm.is_some() || app.pages.input.press.is_active()
        || app.input.press.is_active() || app.ok_armed
        || app.ott.as_ref().is_some_and(|runtime| runtime.setup.is_some()) {
        return DesktopQuit::Pass;
    }
    let action = if app.running { DesktopQuit::Exit } else { DesktopQuit::Consume };
    consume_quit(app, edge, action)
}

const FAVORITE_KEY: u32 = b'f' as u32;
const MUTE_KEY: u32 = b'm' as u32;
const RANDOM_KEY: u32 = b'r' as u32;
const MENU_KEY: u32 = b't' as u32;

struct Held {
    action: Action,
    identity: plx_media::route::PlaybackIdentity,
    ticket: plx_media::route::PlayRequestTicket,
    page: Option<InstanceId>,
    last_ms: u32,
}
#[derive(Default)]
pub(super) struct State {
    held: BTreeMap<u32, Held>,
}

fn physical_key(bytes: &[u8]) -> Option<(u32, u32, Option<Action>)> {
    if !cfg!(feature = "hostsim")
        || bytes.len() < 28
        || events::rd_u32(bytes, 8) == events::SYNTH_WINDOW
    {
        return None;
    }
    let event = events::rd_u32(bytes, 0);
    if event != 0x300 && event != 0x301 {
        return None;
    }
    let (state, _, sym) = events::decode_key(bytes);
    // FOSS PC/PC2 maps T (Tools) to Menu. Other physical playback
    // shortcuts retain their existing Mac-only contract.
    if sym != MENU_KEY && !cfg!(target_os = "macos") { return None; }
    if !matches!(
        sym,
        SDLK_LEFT | SDLK_RIGHT | SDLK_UP | SDLK_DOWN | FAVORITE_KEY | MUTE_KEY | RANDOM_KEY | MENU_KEY
    ) {
        return None;
    }
    // Stock SDL_Keysym.mod. The marked synthetic carrier above is NOT a modifier.
    let modifiers = u16::from_ne_bytes([bytes[24], bytes[25]]);
    let command = modifiers & 0x0c00 != 0;
    let control = modifiers & 0x00c0 != 0;
    let alt = modifiers & 0x0300 != 0;
    // Exactly one modifier family seeks, as in the desktop Classic handler.
    // Shift or combined families retain the page's ordinary input ownership.
    let seek = modifiers & 0x0003 == 0
        && u8::from(command) + u8::from(control) + u8::from(alt) == 1;
    let plain = modifiers & (0x0003 | 0x00c0 | 0x0300 | 0x0c00) == 0;
    let action = match sym {
        SDLK_LEFT if seek => Some(Action::Seek(-10_000_000_000)),
        SDLK_RIGHT if seek => Some(Action::Seek(10_000_000_000)),
        SDLK_LEFT if plain => Some(Action::Volume(-5)),
        SDLK_RIGHT if plain => Some(Action::Volume(5)),
        SDLK_UP if plain => Some(Action::Step(1)),
        SDLK_DOWN if plain => Some(Action::Step(-1)),
        FAVORITE_KEY if plain => Some(Action::Favorite),
        MUTE_KEY if plain => Some(Action::Mute),
        RANDOM_KEY if plain => Some(Action::Random),
        MENU_KEY if plain => Some(Action::Menu),
        _ => None,
    };
    Some((state, sym, action))
}

fn bare_video(runtime: &Runtime, app: &App, now: u32) -> bool {
    app.running
        && playback_allowed(app)
        && matches!(app.route(), AppArg::Player)
        && !app.pages.surface_up()
        && !app.pages.has_pending_navigation()
        && app.pages.input.arm.is_none()
        && !app.pages.input.press.is_active()
        && runtime.edit_session.is_none()
        && runtime.setup.is_none()
        && bridge::player(&app.pages).is_some_and(|page| {
            !page
                .hud
                .visible(&app.player.session, now, lifecycle::viewer_paused())
                && !page.scrub.is_live()
        })
}

fn input_context(runtime: &Runtime, app: &App, now: u32, action: Action) -> bool {
    if action != Action::Menu { return bare_video(runtime, app, now); }
    // Menu is available with or without the HUD. It never steals a field,
    // active press, queued input, or a surface that is about to mount.
    app.running
        && runtime.interface_mode() == plx_machine::machine::UiMode::Classic
        && app.bridge.ui_mode() == plx_machine::machine::UiMode::Classic
        && app.inputs.is_empty()
        && !app.pages.has_pending_surface_navigation()
        && app.pages.input.arm.is_none() && !app.pages.input.press.is_active()
        && !app.input.press.is_active() && !app.ok_armed
        && bridge::player(&app.pages).is_some_and(|page| !page.scrub.is_live())
        && player_remote::offer(runtime, app).is_some()
}


fn perform(
    runtime: &mut Runtime,
    app: &mut App,
    action: Action,
    now: u32,
) -> Result<(), &'static str> {
    input_policy::navigation(runtime)?;
    if global_decoder_transport(action) && super::multiview_output::owns_picture(runtime, app) {
        return Err(match action {
            Action::Seek(_) => "Per-window transport is unavailable for multiview output",
            Action::Step(_) => "Per-window channel stepping is unavailable for multiview output",
            _ => unreachable!("only seek and step are global decoder transport"),
        });
    }
    match action {
        Action::Menu => {
            let offer = player_remote::offer(runtime, app).ok_or("Playback changed before continuation")?;
            let owner = app.pages.top_page().ok_or("The player page is no longer active")?;
            // Use the ordinary dispatcher/content path. The captured source and
            // actual Player owner are checked again before More can be opened.
            app.pages.emit(MachineId::Instance(owner), Fx::App(plx_screens::registry::AppFx::Player(
                plx_screens::registry::PlayerReq::OttAction(plx_screens::registry::OttPlayerRequest {
                    owner, source: offer.source, action: plx_screens::registry::OttPlayerAction::OpenMenu,
                }))));
            return Ok(());
        }
        Action::Favorite => playing_favorite::toggle(runtime, app)?,
        Action::Random => {
            let context = runtime.raw_view().random_context
                .ok_or("Random playback is unavailable for this list")?;
            // Use the same admission and navigation bookkeeping as a clicked
            // Random action. The canonical resolver retains category/search and
            // mixed Favorites bindings; no second random selection algorithm.
            apply_ui_action(runtime, app, OttAction::Random(context), true, true, false);
            return Ok(());
        }
        Action::Volume(_) | Action::Mute => {
            if !commands::playback_ready(app) {
                return Err("No active foreground playback");
            }
            let current = app.player.volume.desired();
            let desired = match action {
                Action::Volume(step) => {
                    let step = step.signum() * runtime.volume_step();
                    let level = (i32::from(current.level()) + step).clamp(0, 100);
                    plx_media::player::volume::DesiredVolume::new(level, false)
                        .ok_or("Invalid volume")?
                }
                Action::Mute => current.with_muted(!current.muted()),
                _ => unreachable!(),
            };
            // This is the existing native volume door; a requested level is not
            // a claim that a TV reported or accepted its physical volume.
            plx_media::player::volume::set_volume(
                &mut app.adapters.player,
                &mut app.player.volume,
                desired,
            );
            if let Some(page) = bridge::player_mut(&mut app.pages) {
                // Feedback describes the application's desired level, not a hardware readback.
                page.volume_notice(desired, now);
            }
        }
        Action::Seek(delta) => {
            if !commands::playback_ready(app) {
                return Err("No active foreground playback");
            }
            let page =
                bridge::player_mut(&mut app.pages).ok_or("The player page is no longer active")?;
            let target = page
                .jump_by(&app.player.session, delta, false, now)
                .ok_or("Seeking requires a finite video timeline")?;
            playback::commit_seek(target);
            runtime.viewer_seek(app);
        }
        Action::Step(offset) => playback_step::perform(runtime, app, offset)?,
    }
    runtime.changed();
    Ok(())
}

#[cfg(test)]
mod multiview_transport_guard_tests {
    use super::{global_decoder_transport, Action};

    #[test]
    fn only_seek_and_list_step_are_decoder_transport() {
        assert!(global_decoder_transport(Action::Seek(10)));
        assert!(global_decoder_transport(Action::Step(1)));
        assert!(!global_decoder_transport(Action::Volume(5)));
        assert!(!global_decoder_transport(Action::Mute));
        assert!(!global_decoder_transport(Action::Favorite));
        assert!(!global_decoder_transport(Action::Random));
        assert!(!global_decoder_transport(Action::Menu));
    }
}

/// Catalogue arrows use the same desired-volume door as desktop playback.
/// The ordinary OTT action runner owns the input instance and policy admission.
pub(super) fn volume_step(runtime: &mut Runtime, app: &mut App, direction: i8) -> Result<(), &'static str> {
    if !matches!(direction, -1 | 1) { return Err("Invalid volume direction"); }
    if !app.running || !playback_allowed(app) || !runtime.loaded || runtime.persistence_uncertain
        || runtime.edit_session.is_some() || runtime.setup.is_some()
        || !runtime.playing.as_ref().and_then(|playing| playing.identity.as_ref()).is_some_and(|identity|
            plx_media::route::playback_identity_matches(&app.player.session, identity))
    { return Err("Volume is unavailable in this input context"); }
    perform(runtime, app, Action::Volume(i32::from(direction)), clock::now())
}

/// Retire physical ownership even when an earlier standby/kiosk guard consumes
/// the edge. A new down also retires a lost prior key-up, without owning the new press.
pub(super) fn boundary(app: &mut App) -> bool {
    if quit_boundary(app) { return true; }
    if numeric_input::boundary(app) { return true; }
    let Some((state, sym, _)) = physical_key(&app.ev) else {
        return false;
    };
    let Some(runtime) = app.ott.as_mut() else {
        return false;
    };
    if state & 0xff != 1 {
        let owned = runtime.desktop_input.held.remove(&sym).is_some();
        if owned && app.down_sym == sym {
            app.down_sym = 0;
        }
        return owned;
    }
    if state & 0x100 == 0 {
        runtime.desktop_input.held.remove(&sym);
    }
    false
}

pub(super) fn ingest(app: &mut App) -> bool {
    if numeric_input::ingest(app) { return true; }
    let Some((state, sym, action)) = physical_key(&app.ev) else {
        return false;
    };
    let Some(mut runtime) = app.ott.take() else {
        return false;
    };
    let now = clock::now();
    let consumed = (|| {
        if state & 0xff != 1 {
            // Modifiers, focus and even the source may change before key-up.
            // A press consumed here always owns its release, never the HUD scrubber.
            let owned = runtime.desktop_input.held.remove(&sym).is_some();
            if owned && app.down_sym == sym {
                app.down_sym = 0;
            }
            return owned;
        }
        if state & 0x100 != 0 {
            let Some(held) = runtime.desktop_input.held.get(&sym) else {
                return false;
            };
            let current = held.page == app.pages.top_page()
                && plx_media::route::playback_identity_matches(&app.player.session, &held.identity)
                && plx_media::route::play_request_is_current(held.ticket)
                && input_context(&runtime, app, now, held.action);
            if !current
                || matches!(held.action, Action::Step(_) | Action::Favorite | Action::Mute | Action::Random | Action::Menu)
                || now.saturating_sub(held.last_ms) < 110
            {
                return true;
            }
            let action = held.action;
            runtime.desktop_input.held.get_mut(&sym).unwrap().last_ms = now;
            app.rec
                .refuse("Physical desktop playback keys are not bound in this recording format");
            if let Err(error) = perform(&mut runtime, app, action, now) {
                runtime.status = error.into();
                runtime.changed();
            }
            return true;
        }
        let Some(action) = action else {
            return false;
        };
        if !input_context(&runtime, app, now, action) {
            return false;
        }
        let Some(identity) = plx_media::route::playback_identity(&app.player.session) else {
            return false;
        };
        runtime.desktop_input.held.insert(
            sym,
            Held {
                action,
                identity,
                ticket: plx_media::route::play_request_ticket(),
                page: app.pages.top_page(),
                last_ms: now,
            },
        );
        app.last_input = now;
        app.down_sym = sym;
        app.rec
            .refuse("Physical desktop playback keys are not bound in this recording format");
        if let Err(error) = perform(&mut runtime, app, action, now) {
            runtime.status = error.into();
            runtime.changed();
        }
        true
    })();
    app.ott = Some(runtime);
    consumed
}


#[cfg(test)]
mod quit_key_tests {
    use super::*;

    fn event(kind: u32, state: u8, repeat: u8, modifiers: u16) -> [u8; 28] {
        let mut bytes = [0; 28];
        bytes[..4].copy_from_slice(&kind.to_ne_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_ne_bytes());
        bytes[12] = state; bytes[13] = repeat;
        bytes[20..24].copy_from_slice(&QUIT_KEY.to_ne_bytes());
        bytes[24..26].copy_from_slice(&modifiers.to_ne_bytes());
        bytes
    }

    #[test]
    fn desktop_quit_parser_rejects_short_inconsistent_and_remote_carrier_input() {
        let bytes = event(0x300, 1, 0, 0);
        for n in 0..28 { assert_eq!(physical_quit(&bytes[..n]), None); }
        for (kind, state) in [(0x300, 0), (0x301, 1), (0x303, 1)] {
            assert_eq!(physical_quit(&event(kind, state, 0, 0)), None);
        }
        let mut remote = bytes;
        remote[8..12].copy_from_slice(&events::SYNTH_WINDOW.to_ne_bytes());
        assert_eq!(physical_quit(&remote), None);
    }

    #[test]
    fn desktop_quit_parser_preserves_repeat_release_and_modifier_identity() {
        if cfg!(feature = "hostsim") {
            assert_eq!(physical_quit(&event(0x300, 1, 0, 0)), Some(QuitEdge::Down { plain: true }));
            for modifiers in [0x1000, 0x2000, 0x3000] {
                assert_eq!(physical_quit(&event(0x300, 1, 0, modifiers)), Some(QuitEdge::Down { plain: true }));
            }
            for modifiers in [1, 2, 0x40, 0x80, 0x100, 0x200, 0x400, 0x800, 0x4000] {
                assert_eq!(physical_quit(&event(0x300, 1, 0, modifiers)), Some(QuitEdge::Down { plain: false }));
            }
            assert_eq!(physical_quit(&event(0x300, 1, 1, 0)), Some(QuitEdge::Repeat));
            assert_eq!(physical_quit(&event(0x301, 0, 0, 0x400)), Some(QuitEdge::Up));
        } else {
            assert_eq!(physical_quit(&event(0x300, 1, 0, 0)), None, "native TV builds never reinterpret Q");
        }
    }
}

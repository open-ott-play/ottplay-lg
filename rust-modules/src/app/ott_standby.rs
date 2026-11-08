//! Application Standby uses the player's existing foreground park and restore.
//! The caller retires pending OTT catalogue work before entering; this adapter
//! deliberately does not borrow `App::ott`, which may be owned by its dispatcher.

use super::{bridge, content, lifecycle, run, App};

/// A swallowed release must not leave either input implementation holding a
/// press. Use their normal cancellation paths so the visual spring can finish
/// without emitting a Hold/Commit on the covered screen.
fn cancel_interaction(
    shared: &mut plx_ui::input::InputMachine<u32>,
    legacy: &mut plx_ui::press::Press,
    down_sym: &mut u32,
    ok_armed: &mut bool,
    pointer: &mut super::input::Pointer,
    queued: &mut Vec<plx_machine::machine::InputEvent<u32>>,
) {
    shared.cancel_press();
    legacy.cancel();
    *down_sym = 0;
    *ok_armed = false;
    pointer.button_down = false;
    pointer.ok_from_click = false;
    pointer.edge.stop();
    queued.clear();
}

/// Set the logical Standby state; return whether that state changed.
///
/// Repeated Standby reasserts suspension if new playback work appeared, while
/// retaining an already parked session's position and viewer pause. Wake only
/// claims that session when the operating system grants foreground permission;
/// otherwise its ordinary DID-foreground edge performs the restore later.
/// The return value describes the state change, not completion of a native Load.
pub(super) fn set(app: &mut App, standby: bool) -> bool {
    let changed = app.ott_standby != standby;
    app.ott_standby = standby;

    if standby || changed {
        cancel_interaction(
            &mut app.pages.input,
            &mut app.input.press,
            &mut app.down_sym,
            &mut app.ok_armed,
            &mut app.ptr,
            &mut app.inputs,
        );
    }

    if standby {
        // Give mounted editors and screens the same suspension event as an OS
        // background, retaining the page stack for the eventual wake.
        bridge::background(&mut app.pages);
        // These paths must not independently restore a session held by Standby.
        app.foreground_retry = None;
        app.decoder_parked = false;

        // A preview is ended, never restored as ordinary playback.
        content::halt_preview_now(&mut app.player.session, &mut app.adapters.player);
        let flight_parked = lifecycle::park_pending_foreground_flight(
            &mut app.player.lifecycle,
            &mut app.player.session,
            &mut app.adapters.player,
        );

        // A resolve can own playback before the Player page mounts. Conversely,
        // an already suspended session must not capture the suspension's pause
        // or overwrite its saved position on another Standby command.
        let needs_park = !app.player.lifecycle.awaiting_load()
            && !plx_media::route::preview_request(&app.player.session)
            && (plx_media::route::play_pending()
                || app.adapters.player.is_live()
                || plx_media::route::has_url(&app.player.session));
        if needs_park {
            run::park_playback(app);
        }
        if changed || flight_parked || needs_park {
            plx_machine::idle::invalidate();
        }
    } else if changed {
        if app.window_activity.allow_present(true) {
            bridge::foreground(&mut app.pages);
            if app.player.lifecycle.claimable_clock().is_some() {
                run::claim_parked_foreground(app, lifecycle::ForegroundInput::DidForeground);
            }
        }
        plx_machine::idle::invalidate();
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_machine::machine::{
        Edge, EntryId, FocusKey, InputEvent, InputKind, InstanceId, Key, MachineId, PressArm,
        PressFrom, Source, Tick,
    };

    #[test]
    fn standby_cancels_held_and_click_presses_even_when_the_release_is_swallowed() {
        for (from, holdable) in [(PressFrom::Key, true), (PressFrom::Pointer, false)] {
            let mut shared = plx_ui::input::InputMachine::new();
            shared.arm(
                PressArm {
                    key: FocusKey {
                        entry: EntryId(1),
                        elem: 7_u32,
                    },
                    from,
                    holdable,
                },
                MachineId::Instance(InstanceId(1)),
                1_000,
            );
            let mut legacy = plx_ui::press::Press::new();
            legacy.begin(1_000);
            let mut down_sym = 13;
            let mut ok_armed = true;
            let mut pointer = super::super::input::Pointer::IDLE;
            pointer.button_down = true;
            pointer.ok_from_click = true;
            let mut queued = vec![InputEvent {
                at: Tick {
                    ms: 1_000,
                    dt_us: 0,
                },
                source: Source::Script,
                kind: InputKind::Key {
                    key: Key::Ok,
                    sym: 13,
                    wcode: 0,
                    edge: Edge::Down,
                    at_edge: false,
                },
            }];

            cancel_interaction(
                &mut shared,
                &mut legacy,
                &mut down_sym,
                &mut ok_armed,
                &mut pointer,
                &mut queued,
            );
            // The physical key/button-up is swallowed by Standby. Continue the
            // normal frame clock beyond both long-press and lost-release delays.
            for frame in 1..=160 {
                let now = 1_000 + frame * 16;
                assert!(
                    shared.tick(now, 0.016).is_empty(),
                    "covered control activated"
                );
                legacy.tick(now, 0.016);
                assert!(!legacy.take_commit(now), "legacy control activated");
            }
            assert!(shared.arm.is_none());
            assert!(queued.is_empty(), "queued pre-Standby input survived");
            assert_eq!(down_sym, 0);
            assert!(!ok_armed && !pointer.button_down && !pointer.ok_from_click);
        }
    }
}

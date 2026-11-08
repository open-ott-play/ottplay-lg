//! Live executor for Settings preference effects. The bridge's controlled-IO guard must admit
//! the effect before this entrypoint: profile capture, thread admission and persistence all live
//! here, never in a screen constructor or step.
use plx_media::route::{choose, Choice, How};
use plx_plex::plex::account::{PreferenceError, PreferenceRequest};
use plx_plex::plex::session::{InstantDpadSeekingPref, DeckPressPref, DirectPlayModePref, HdrOverlayBrightnessPref, NextEpisodeModePref, QualityPref,
    SkipIntervalPref, SubtitlePositionPref, SubtitleSizePref, TrailerAutoplayPref, UnderPostersPref};
use plx_screens::registry::{AccountPreferenceReply, LanguageApplyResult, PreferenceCmd};
use std::sync::mpsc::Sender;
#[path = "preferences_language.rs"]
mod language;
pub(super) use language::State as LanguageState;

/// Called before the dispatcher frame, independent of the page that requested
/// the durable write. The following frame refreshes all materialized labels.
pub(super) fn poll_language(app: &mut super::App) {
    let live = app.running && matches!(&app.rec, super::recorder::Recplay::Off)
        && !app.bridge.is_controlled_replay();
    if let Some(epoch) = language::poll(&mut app.language_apply, live) {
        super::ott_runtime::locale_changed(app);
        app.pages.locale_changed(epoch);
        plx_machine::idle::invalidate();
    }
}

/// The one place a Settings pick meets the write door: each row's own `SETTINGS` policy
/// (`How::settings`) decides whether the pick is confirmed first or published at once.
fn choose_setting(choice: Choice, reply: Sender<bool>) {
    match choice {
        Choice::Quality(v) => choose::<QualityPref>(v, How::settings::<QualityPref>(reply)),
        Choice::DirectPlay(v) => choose::<DirectPlayModePref>(v, How::settings::<DirectPlayModePref>(reply)),
        Choice::NextEpisode(v) => choose::<NextEpisodeModePref>(v, How::settings::<NextEpisodeModePref>(reply)),
        Choice::InstantDpadSeeking(v) => choose::<InstantDpadSeekingPref>(v, How::settings::<InstantDpadSeekingPref>(reply)),
        Choice::SkipInterval(v) => choose::<SkipIntervalPref>(v, How::settings::<SkipIntervalPref>(reply)),
        Choice::DeckPress(v) => choose::<DeckPressPref>(v, How::settings::<DeckPressPref>(reply)),
        Choice::UnderPosters(v) => choose::<UnderPostersPref>(v, How::settings::<UnderPostersPref>(reply)),
        Choice::TrailerAutoplay(v) => choose::<TrailerAutoplayPref>(v, How::settings::<TrailerAutoplayPref>(reply)),
        Choice::SubtitleSize(v) => choose::<SubtitleSizePref>(v, How::settings::<SubtitleSizePref>(reply)),
        Choice::SubtitlePosition(v) => choose::<SubtitlePositionPref>(v, How::settings::<SubtitlePositionPref>(reply)),
        Choice::HdrOverlayBrightness(v) => choose::<HdrOverlayBrightnessPref>(v, How::settings::<HdrOverlayBrightnessPref>(reply)),
    }
}


/// A native Settings effect retains its original outer instance through the
/// bridge queue. Admission happens here, after this frame's Saved publication;
/// hiding a surface is not authority to execute a command it already emitted.
pub(super) fn drain(app: &mut super::App) {
    use plx_machine::machine::{InputOwner, MachineId};
    for (from, command) in app.bridge.take_preference_reqs() {
        // Retired inner-page ownership cannot be recovered by the outer surface
        // still being current. It is negative admission metadata, not a request.
        if matches!(&command, PreferenceCmd::Retired(_)) {
            refuse(command);
            continue;
        }
        let live = if let MachineId::Instance(instance) = from {
            app.pages.nav.entry_of_instance(instance).is_some_and(|entry|
                app.pages.nav.input_owner() == Some(InputOwner::Entry(entry)))
        } else { false };
        let unavailable = !app.running || app.pages.nav.suspended
            || app.pages.has_pending_navigation()
            || app.pages.nav.tabs.stack.is_pending()
            || matches!(&app.rec, super::recorder::Recplay::Replaying(_))
            || app.bridge.is_controlled_replay();
        let forbidden = !command.is_read_only() && super::ott_runtime::kiosk_user_keys_blocked(app);
        let controlled_language = matches!(&command, PreferenceCmd::Language { .. })
            && !matches!(&app.rec, super::recorder::Recplay::Off);
        if !live || unavailable || forbidden || controlled_language {
            refuse(command);
            continue;
        }
        execute(app, command);
    }
}

/// Settle refusal only at the App side of the effect boundary. Peel nested
/// negative metadata iteratively; no wrapper can regain execution authority.
fn refuse(mut command: PreferenceCmd) {
    loop {
        match command {
            PreferenceCmd::Retired(inner) => { command = *inner; continue; }
            PreferenceCmd::Load { reply } => { let _ = reply.send(AccountPreferenceReply {
                request: None, outcome: Err(PreferenceError::Refused) }); }
            PreferenceCmd::Save { request, reply, .. } => { let _ = reply.send(AccountPreferenceReply {
                request: Some(request), outcome: Err(PreferenceError::Refused) }); }
            PreferenceCmd::Language { reply, .. } => { let _ = reply.send(LanguageApplyResult::Failed); }
            PreferenceCmd::Choose(_, reply) | PreferenceCmd::UiResolution { reply, .. } => { let _ = reply.send(false); }
        }
        break;
    }
    plx_machine::idle::invalidate();
}

fn execute(app: &mut super::App, command: PreferenceCmd) {
    // Defensive boundary: even a future direct caller cannot execute a retired
    // command or expose it to the final-side-effect test observer.
    if matches!(&command, PreferenceCmd::Retired(_)) { refuse(command); return; }
    #[cfg(test)]
    if admission_probe::observe(&command) { admission_probe::complete(command); return; }
    match command {
        command @ PreferenceCmd::Retired(_) => refuse(command),
        PreferenceCmd::Load { reply } => {
            let Some(request) = PreferenceRequest::capture() else {
                let _ = reply.send(AccountPreferenceReply {
                    request: None, outcome: Err(PreferenceError::Unavailable),
                });
                plx_machine::idle::invalidate();
                return;
            };
            // A refused spawn drops the reply sender. The screen reports its disconnected
            // receipt as retryable without claiming that an account call ran.
            plx_base::task::spawn_small("acct-prefs", move || {
                let outcome = request.load();
                let _ = reply.send(AccountPreferenceReply { request: Some(request), outcome });
                plx_machine::idle::invalidate();
            });
        }
        PreferenceCmd::Save { request, base, update, reply } => {
            plx_base::task::spawn_small("acct-prefs", move || {
                let outcome = request.save(&base, update);
                let _ = reply.send(AccountPreferenceReply { request: Some(request), outcome });
                plx_machine::idle::invalidate();
            });
        }
        // Every live setting saves through the one write door, in `choose_setting`.
        PreferenceCmd::Choose(choice, reply) => choose_setting(choice, reply),
        PreferenceCmd::UiResolution { mode, reply } => {
            let _ = plx_base::storage_worker::submit_retained(move || {
                let _ = reply.send(plx_plex::plex::session::set_ui_resolution(mode));
                plx_machine::idle::invalidate();
            });
        }
        PreferenceCmd::Language { language, reply } => {
            language::begin(&mut app.language_apply, language, reply);
        }
    }
}

/// Test-only replacement of the final side-effect boundary. Dispatcher and policy
/// admission still run unchanged; no fixture writes private settings or starts IO.
#[cfg(test)]
pub(crate) mod admission_probe {
    use super::*;
    use std::cell::RefCell;
    thread_local! { static CALLS: RefCell<Option<Vec<&'static str>>> = const { RefCell::new(None) }; }
    #[cfg(feature = "hostsim")]
    pub(crate) struct Probe;
    #[cfg(feature = "hostsim")]
    impl Probe {
        pub(crate) fn new() -> Self {
            CALLS.with(|slot| { assert!(slot.borrow().is_none()); *slot.borrow_mut() = Some(Vec::new()); });
            Self
        }
        pub(crate) fn calls(&self) -> Vec<&'static str> { CALLS.with(|slot| slot.borrow().as_ref().unwrap().clone()) }
    }
    #[cfg(feature = "hostsim")]
    impl Drop for Probe { fn drop(&mut self) { CALLS.with(|slot| { slot.borrow_mut().take(); }); } }
    pub(super) fn observe(command: &PreferenceCmd) -> bool {
        CALLS.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(calls) = slot.as_mut() else { return false; };
            calls.push(match command { PreferenceCmd::Retired(_) => "retired",
                PreferenceCmd::Load {..} => "load", PreferenceCmd::Save {..} => "save",
                PreferenceCmd::Choose(..) => "choose", PreferenceCmd::Language {..} => "language",
                PreferenceCmd::UiResolution {..} => "resolution" });
            true
        })
    }
    pub(super) fn complete(command: PreferenceCmd) {
        match command {
            command @ PreferenceCmd::Retired(_) => refuse(command),
            PreferenceCmd::Load { reply } => { let _ = reply.send(AccountPreferenceReply {
                request: None, outcome: Err(PreferenceError::Unavailable) }); }
            PreferenceCmd::Save { request, reply, .. } => { let _ = reply.send(AccountPreferenceReply {
                request: Some(request), outcome: Err(PreferenceError::Unavailable) }); }
            PreferenceCmd::Language { reply, .. } => { let _ = reply.send(LanguageApplyResult::LiveApplied); }
            PreferenceCmd::Choose(_, reply) | PreferenceCmd::UiResolution { reply, .. } => { let _ = reply.send(true); }
        }
    }
}

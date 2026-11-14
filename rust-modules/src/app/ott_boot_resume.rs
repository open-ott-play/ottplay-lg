//! Paint-only cover for the first automatic source restore. Admission remains
//! entirely in the normal catalogue, resolver, parental and player boundaries.
use super::*;

#[derive(Default)]
pub(super) struct State {
    active: bool,
    source: Option<String>,
}

pub(super) fn arm(runtime: &mut Runtime) {
    runtime.boot_resume.active = true;
}

pub(super) fn pending(runtime: &Runtime) -> bool {
    runtime.boot_resume.active && runtime.navigation_epoch == 0
        && !runtime.persistence_uncertain
        && !matches!(runtime.state_load, StateLoad::Failed(_))
}

/// Display only the exact currently queued automatic target. The cover never
/// turns a catalogue selection, old decoder or remote request into admission.
pub(super) fn target(runtime: &Runtime) -> Option<String> {
    if !pending(runtime) || !runtime.loaded || !runtime.play_work_allowed()
        || runtime.cancel.load(Ordering::Acquire) || runtime.play_cancel.load(Ordering::Acquire)
        || !runtime.play_ticket.is_some_and(plx_media::route::play_request_is_current)
        || !runtime.pending_attachment_current() || !transient::target_current(runtime)
        || runtime.pending_play.is_some()
    { return None; }
    let request = runtime.play_target.as_ref()?;
    if request.continuation.is_some() { return None; }
    let (_, profile, entry, _) = runtime.prepare_play_request(request.clone()).ok()?;
    if !profile.enabled || runtime.effective_profile() != Some(&profile)
        || runtime.pending_play_profile() != Some(&profile)
        || runtime.boot_resume.source.as_deref() != Some(profile_fingerprint(&profile).as_str())
    { return None; }
    entry?;
    Some(request.id.clone())
}

/// Facts come from the owned resolver capture, never filtered screen rows.
pub(super) fn display(runtime: &Runtime) -> Option<plx_screens::ott::OttPendingDisplay> {
    let id = target(runtime).or_else(|| classic_play_handoff::selected(runtime))?;
    if !runtime.loaded || runtime.persistence_uncertain || !runtime.play_work_allowed()
        || runtime.cancel.load(Ordering::Acquire) || runtime.play_cancel.load(Ordering::Acquire)
        || !runtime.play_ticket.is_some_and(plx_media::route::play_request_is_current)
        || !runtime.pending_attachment_current() || !transient::target_current(runtime)
        || runtime.pending_play.is_some()
    { return None; }
    let request = runtime.play_target.as_ref().filter(|request| request.id == id)?;
    // Pending continuations require a matching native session. This runtime-only
    // snapshot cannot prove that session and must not guess from a selection ID.
    if request.continuation.is_some() { return None; }
    let (_, profile, entry, _) = runtime.prepare_play_request(request.clone()).ok()?;
    if runtime.pending_play_profile() != Some(&profile) || !profile.enabled { return None; }
    let entry = entry?;
    let live = request.archive.is_none() && guide_ui::is_live(&profile, &entry);
    let guide = live.then(|| runtime.selected_row_guide(&request.id, Some(&entry)).or_else(|| {
        let now_s = runtime.wall_now_s()?;
        if runtime.effective_profile() != Some(&profile) { return None; }
        let programme = runtime.current_row_programme(&entry)?;
        Some(plx_data::metadata::OttNowNext { now_s,
            current: Some(plx_data::metadata::OttGuideProgramme { title: programme.title,
                description: String::new(), start_s: programme.start_s, end_s: programme.end_s }),
            next: None })
    })).flatten();
    let description = guide.as_ref().and_then(|guide| guide.at(guide.now_s).0)
        .map(|programme| programme.description.clone()).filter(|description| !description.is_empty())
        .unwrap_or_else(|| entry.description.clone());
    Some(plx_screens::ott::OttPendingDisplay {
        title: entry.title.clone(), description, live,
        ordinal: live.then(|| player_startup::pending_ordinal(runtime)).flatten(),
        guide, poster: entry_poster(&entry),
    })
}

/// Already-authorized native Home may mount before protected OTT intent loads.
/// Hide only that initial root page, never authentication, consent or a modal.
/// The existing navigation coordinator remains the sole source/route owner.
pub(super) fn home_pending(app: &App) -> bool {
    let Some(runtime) = app.ott.as_ref() else { return false; };
    if !pending(runtime) || !app.running || app.ott_standby
        || !app.bridge.session_ready_current() || app.bridge.is_controlled_replay()
        || matches!(&app.rec, super::super::recorder::Recplay::Replaying(_))
        || app.last_input != app.t0 || !matches!(app.route(), AppArg::Home)
        || app.pages.nav.tabs.stack.depth() != 1 || app.pages.nav.suspended
        || app.pages.surface_up() || app.pages.has_pending_surface_navigation()
    { return false; }
    if !runtime.loaded { return matches!(runtime.state_load, StateLoad::Loading); }
    classic_navigation::startup_home_cover(runtime, app)
}

pub(super) fn retire(runtime: &mut Runtime) {
    if runtime.boot_resume.active {
        runtime.boot_resume = Default::default();
        runtime.changed();
    }
}

pub(super) fn reconcile(runtime: &mut Runtime, app: &App) {
    if !runtime.boot_resume.active { return; }
    if !pending(runtime) || runtime.setup.is_some() || runtime.edit_session.is_some() {
        retire(runtime);
        return;
    }
    // Do not infer either configuration or kiosk policy before the protected
    // document has actually loaded. A failed load instead reveals recovery.
    if !runtime.loaded { return; }
    let source = runtime.active().filter(|profile| profile.enabled && !profile.source.is_empty())
        .map(profile_fingerprint);
    let Some(source) = source else { retire(runtime); return; };
    if runtime.boot_resume.source.as_ref().is_some_and(|initial| initial != &source) {
        retire(runtime);
        return;
    }
    runtime.boot_resume.source = Some(source);
    // A queued navigation is not yet a mounted Player: keep the cover through
    // the commit, without creating a fake session or adopting an old decoder.
    let mounted_player = app.pages.nav.top_page().is_some_and(|entry|
        matches!(entry.arg, AppArg::Player)
            && entry.inst.as_ref().is_some_and(|body| !body.staged));
    if mounted_player || (!runtime.catalog_loading && runtime.catalog_complete
        && runtime.play_target.is_none() && runtime.resume_pending.is_none()
        && runtime.resolver_next.is_none() && runtime.playing.is_none())
    {
        retire(runtime);
    }
}

pub(super) fn back(runtime: &mut Runtime, app: &App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if !matches!(action, OttAction::Back) || !pending(runtime) { return None; }
    // Same reply ownership and semantic policy as normal catalogue navigation;
    // unknown or strict kiosk policy cannot be bypassed by this presentation.
    if runtime.reply.is_none() || runtime.reply != current_ui_owner(app) {
        return Some(Err("The player view changed. Try again."));
    }
    if let Err(error) = input_policy::ui(runtime, action) { return Some(Err(error)); }
    retire(runtime);
    runtime.cancel_play_request();
    runtime.resume_pending = None;
    runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
    runtime.continuity.stop(runtime.generation);
    // Preserve the catalogue and any useful progressive results. Cancelling
    // restore must not restart it when the next batch or a late resolver arrives.
    runtime.status.clear();
    runtime.changed();
    Some(Ok(()))
}

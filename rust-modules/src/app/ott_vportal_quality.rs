//! Addressed VPortal quality preparation. The picker holds resources, never a playable grant.
use super::*;
use super::super::ott_vportal_lease::Lease;

const SETTING_PREFIX: &str = "vportalQuality.";
#[derive(Default)]
pub(super) struct State {
    serial: u64,
    intent: Option<Context>,
    pending: Option<Pending>,
    pub(super) retired: Vec<Lease>,
}
struct Context {
    token: String,
    owner: InstanceId,
    navigation: Option<u64>,
    catalog: u64,
    entry: Option<(String, MediaLocator)>,
    favorite: Option<Favorite>,
    play: u64,
    request: PlaybackRequest,
    profile: ProviderProfile,
    return_page: &'static str,
}
struct Pending {
    context: Context,
    entry: CatalogEntry,
    choices: ott_catalog::VPortalChoices,
    selected: Option<usize>,
    saving: bool,
}
fn digest(value: &impl serde::Serialize) -> String {
    plx_data::ott::favorites_wire::hash(&serde_json::to_vec(value).expect("quality identity is serializable"))
}
fn setting(profile: &ProviderProfile) -> String { format!("{SETTING_PREFIX}{}", digest(&profile.id)) }
fn fingerprint(profile: &ProviderProfile) -> String { digest(&(profile.kind, &profile.source)) }
fn valid_key(key: &str) -> bool { ott_catalog::valid_vportal_quality_key(key) }
pub(super) fn preference(saved: &Saved, profile: &ProviderProfile) -> Option<String> {
    let value = saved.settings.get(&setting(profile))?;
    if value.get("source")?.as_str()? != fingerprint(profile) { return None; }
    let key = value.get("key")?.as_str()?;
    valid_key(key).then(|| key.to_owned())
}
fn preference_candidate(saved: &Saved, profile: &ProviderProfile, key: &str) -> Result<Saved, &'static str> {
    if !valid_key(key) { return Err("Invalid VPortal quality preference"); }
    let mut candidate = saved.clone();
    candidate.settings.insert(setting(profile), json!({"source":fingerprint(profile),"key":key}));
    Ok(candidate)
}
pub(super) fn may_offer(runtime: &Runtime, app: &App) -> bool {
    runtime.reply.is_some() && current_ui_owner(app) == runtime.reply && playback_allowed(app)
        && !runtime.saving && runtime.loaded && !runtime.persistence_uncertain
        && matches!(super::super::ott_kiosk_stored::read(&runtime.saved.settings), Ok(None))
}
pub(super) fn request(runtime: &mut Runtime, profile: &ProviderProfile) -> Result<(), &'static str> {
    let owner = runtime.reply.ok_or("VPortal quality owner is unavailable")?;
    let request = runtime.play_target.clone().ok_or("VPortal playback request is unavailable")?;
    let (bound, bound_profile, entry, favorite) = runtime.prepare_play_request(request.clone())?;
    if bound != request || &bound_profile != profile {
        return Err("VPortal playback source changed before quality preparation");
    }
    // Catalogue append/enrichment is not a new selection. Retain only the
    // selected source facts, matching native admission's id/locator comparison.
    let entry = entry.map(|entry| (entry.id, entry.media));
    runtime.quality.serial = runtime.quality.serial.checked_add(1).ok_or("Reopen the player before selecting quality")?;
    let token = digest(&(runtime.remote.runtime(), runtime.quality.serial, runtime.play_generation));
    runtime.quality.intent = Some(Context { token, owner, navigation: None, catalog: runtime.generation, entry, favorite,
        play: runtime.play_generation, request, profile: profile.clone(), return_page: runtime.page });
    Ok(())
}
/// Called only after the SAME accepted local Play action advances navigation.
pub(super) fn seal(runtime: &mut Runtime) {
    if let Some(context) = runtime.quality.intent.as_mut().filter(|context| context.navigation.is_none()
        && context.play == runtime.play_generation && Some(context.owner) == runtime.reply) {
        context.navigation = Some(runtime.navigation_epoch);
    }
}
pub(super) fn interactive(runtime: &Runtime, generation: u64) -> bool {
    runtime.quality.intent.as_ref().is_some_and(|context| context.play == generation)
}
fn current(runtime: &Runtime, app: &App, context: &Context) -> bool {
    runtime.loaded && !runtime.persistence_uncertain && playback_allowed(app) && runtime.play_work_allowed()
        && runtime.reply == Some(context.owner) && current_ui_owner(app) == Some(context.owner)
        && context.navigation == Some(runtime.navigation_epoch) && context.catalog == runtime.generation
        && !runtime.play_cancel.load(Ordering::Acquire)
        && runtime.play_ticket.is_some_and(plx_media::route::play_request_is_current)
        && context.play == runtime.play_generation && runtime.play_target.as_ref() == Some(&context.request)
        && matches!(super::super::ott_kiosk_stored::read(&runtime.saved.settings), Ok(None))
        && runtime.prepare_play_request(context.request.clone()).is_ok_and(|(request, profile, entry, favorite)|
            request == context.request && profile == context.profile
                && entry.as_ref().map(|entry| (&entry.id, &entry.media))
                    == context.entry.as_ref().map(|(id, media)| (id, media))
                && favorite == context.favorite)
        && runtime.pending_attachment_current() && commands::pending_catalog_current(runtime)
        && transient::target_current(runtime)
}
pub(super) fn cancel(runtime: &mut Runtime) {
    if let Some(pending) = runtime.quality.pending.take() {
        if runtime.page == "vportal-quality" { runtime.page = pending.context.return_page; }
        runtime.quality.retired.extend(pending.choices.retire());
    }
    runtime.quality.intent = None;
}
pub(super) fn observe(runtime: &mut Runtime, app: &App) {
    let stale = runtime.quality.pending.as_ref().is_some_and(|pending|
        runtime.page != "vportal-quality" || !current(runtime, app, &pending.context))
        || runtime.quality.intent.as_ref().is_some_and(|context|
            context.navigation.is_some() && !current(runtime, app, context));
    if stale {
        preplay::cancel(runtime, preplay::CancelReason::OwnerChanged);
        cancel(runtime); runtime.cancel_play_request(); runtime.changed();
    }
}
pub(super) fn received(runtime: &mut Runtime, app: &mut App, generation: u64,
    profile: ProviderProfile, entry: CatalogEntry, choices: ott_catalog::VPortalChoices) {
    // A retired worker must not consume the replacement request's UI intent.
    if !runtime.quality.intent.as_ref().is_some_and(|context| context.play == generation && context.profile == profile) {
        runtime.quality.retired.extend(choices.retire());
        return;
    }
    let context = runtime.quality.intent.take();
    let valid = context.as_ref().is_some_and(|context| context.play == generation
        && context.profile == profile && current(runtime, app, context));
    if !valid || runtime.rebind_native_admission(app, &profile, &entry).is_err() {
        runtime.quality.retired.extend(choices.retire());
        if generation == runtime.play_generation {
            preplay::cancel(runtime, preplay::CancelReason::OwnerChanged);
            runtime.cancel_play_request();
        }
        return;
    }
    let selected = choices.selected;
    runtime.quality.pending = Some(Pending { context: context.unwrap(), entry, choices, selected, saving: false });
    runtime.page = "vportal-quality";
    preplay::phase(runtime, generation, preplay::Phase::AwaitingQuality);
    runtime.status.clear(); runtime.changed();
}
fn variant_token(context: &Context, key: &str) -> String { digest(&(&context.token, key)) }
fn row_id(context: &Context, variant: &str) -> String { digest(&(&context.token, variant)) }
pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if runtime.page != "vportal-quality" { return None; }
    let pending = runtime.quality.pending.as_ref()?;
    let context = &pending.context;
    let rows = pending.choices.options.iter().map(|option| {
        let variant = variant_token(context, &option.key);
        let mut row = ui_projection::row(row_id(context, &variant), option.label.clone(), String::new(),
            OttAction::SelectVPortalQuality { context: context.token.clone(), variant }, false);
        row.enabled = !pending.saving;
        row
    }).collect();
    let selected_row = pending.selected.and_then(|index| pending.choices.options.get(index))
        .map(|option| row_id(context, &variant_token(context, &option.key)));
    Some(OttView { page_id: format!("vportal-quality:{}", context.token), revision: runtime.revision,
        title: plx_platform::i18n::msg::settings_playback_quality().into(), status: runtime.status.clone(),
        rows, selected_row, ..OttView::default() })
}
pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction) -> Option<Result<(), &'static str>> {
    if matches!(action, OttAction::Back) && runtime.page == "vportal-quality" {
        preplay::cancel(runtime, preplay::CancelReason::UserBack);
        cancel(runtime); runtime.cancel_play_request(); runtime.status.clear(); return Some(Ok(()));
    }
    let OttAction::SelectVPortalQuality { context, variant } = action else { return None; };
    Some(select(runtime, app, context, variant))
}
fn select(runtime: &mut Runtime, app: &mut App, token: &str, variant: &str) -> Result<(), &'static str> {
    let mut pending = runtime.quality.pending.take().ok_or("VPortal quality selection is stale")?;
    if runtime.page != "vportal-quality" || pending.context.token != token || pending.saving
        || !current(runtime, app, &pending.context) {
        runtime.quality.pending = Some(pending); return Err("VPortal quality selection is stale");
    }
    let Some(index) = pending.choices.options.iter().position(|option| variant_token(&pending.context, &option.key) == variant) else {
        runtime.quality.pending = Some(pending); return Err("VPortal quality selection is stale");
    };
    if let Err(error) = runtime.rebind_native_admission(app, &pending.context.profile, &pending.entry) {
        runtime.quality.retired.extend(pending.choices.retire());
        runtime.page = pending.context.return_page;
        preplay::cancel(runtime, preplay::CancelReason::OwnerChanged);
        runtime.cancel_play_request();
        return Err(error);
    }
    let candidate = match preference_candidate(&runtime.saved, &pending.context.profile, &pending.choices.options[index].key) {
        Ok(candidate) => candidate,
        Err(error) => { runtime.quality.pending = Some(pending); return Err(error); }
    };
    let outcome = runtime.save(candidate, None, false);
    if outcome.is_err() {
        preplay::failed(runtime, pending.context.play, preplay::Failure::QualitySaveFailed);
    }
    if outcome.is_ok() {
        pending.selected = Some(index); pending.saving = true;
        runtime.save_ui_owner = Some(pending.context.owner);
    }
    runtime.quality.pending = Some(pending);
    outcome
}
pub(super) fn settled(runtime: &mut Runtime, app: &mut App, success: bool) {
    if !runtime.quality.pending.as_ref().is_some_and(|pending| pending.saving) { return; }
    let mut pending = runtime.quality.pending.take().unwrap();
    if !current(runtime, app, &pending.context) || runtime.page != "vportal-quality" {
        runtime.quality.retired.extend(pending.choices.retire());
        if runtime.page == "vportal-quality" { runtime.page = pending.context.return_page; }
        preplay::cancel(runtime, preplay::CancelReason::OwnerChanged);
        runtime.cancel_play_request();
        return;
    }
    if !success {
        preplay::failed(runtime, pending.context.play, preplay::Failure::QualitySaveFailed);
        pending.saving = false; runtime.quality.pending = Some(pending); return;
    }
    let index = pending.selected.expect("admitted quality save owns an index");
    if preference(&runtime.saved, &pending.context.profile).as_deref() != Some(pending.choices.options[index].key.as_str()) {
        runtime.quality.retired.extend(pending.choices.retire());
        runtime.page = pending.context.return_page;
        preplay::failed(runtime, pending.context.play, preplay::Failure::AdmissionFailed);
        runtime.cancel_play_request();
        runtime.status = "Quality preference was not confirmed".into();
        return;
    }
    let Context { play: generation, profile, return_page, .. } = pending.context;
    runtime.page = return_page;
    let sender = runtime.tx.clone(); let cancel = runtime.play_cancel.clone();
    preplay::phase(runtime, generation, preplay::Phase::Resolving);
    let mut observation = preplay::worker(runtime, generation, &profile);
    // Retain ownership if spawning fails, exactly like the ordinary resolver's
    // previous-session barrier. No lock is held during probe or cleanup IO.
    let prepared = Arc::new(std::sync::Mutex::new(Some((pending.entry, pending.choices))));
    let work = prepared.clone();
    let started = plx_base::task::spawn_off_frame("ott-media", move |worker| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (entry, choices) = work.lock().unwrap_or_else(|poison| poison.into_inner())
                .take().ok_or("VPortal quality selection is stale")?;
            let resolved = choices.choose(worker, Some(index), &cancel)?;
            let ready = observation.inspect(||
                inspect(plx_media::ff::ExternalSource::Http(resolved.url), resolved.lease, &cancel))?;
            Ok((entry, ready))
        })).unwrap_or(Err("Media worker failed"));
        if result.is_err() { observation.failed(); }
        let _ = sender.send(Job::Play(generation, profile, result));
        plx_machine::idle::invalidate();
    });
    runtime.resolver_busy = started;
    if !started {
        if let Some((_, choices)) = prepared.lock().unwrap_or_else(|poison| poison.into_inner()).take() {
            runtime.quality.retired.extend(choices.retire());
        }
        preplay::failed(runtime, generation, preplay::Failure::WorkerUnavailable);
        runtime.cancel_play_request(); runtime.status = "Could not open media worker".into();
    }
}
pub(super) fn inspect(source: plx_media::ff::ExternalSource, lease: Option<Lease>, cancel: &Arc<AtomicBool>)
    -> Result<ReadySource, &'static str> {
    let result = plx_media::ff::inspect::source(&source, cancel.clone());
    if cancel.load(Ordering::Acquire) || result.is_err() {
        if let Some(lease) = lease { lease.finish_off_frame()?; }
        return Err(result.err().unwrap_or("Playback request cancelled"));
    }
    Ok(ReadySource::External(source, result.unwrap(), lease))
}

pub(super) fn completed_resolution(runtime: &mut Runtime, generation: u64) {
    if runtime.quality.intent.as_ref().is_some_and(|context| context.play == generation) {
        runtime.quality.intent = None;
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_runtime_tests/vportal_quality.rs"]
mod tests;

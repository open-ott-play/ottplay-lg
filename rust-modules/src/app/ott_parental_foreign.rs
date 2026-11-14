//! Alternate native screens use the same source-owned membership and PIN gate.
//! A page label, requested rating key or retained card is not catalogue authority.
use super::*;

/// A configured target is independent of the currently playing provider. Its
/// credentials must still attest the registered client before it can prompt.
fn configured_target(runtime: &Runtime, app: &App, sid: plx_plex::plex::ServerId)
    -> Result<ProviderProfile, &'static str>
{
    if !app.bridge.session_ready_current() { return Err("The selected source session is unavailable"); }
    let mut profiles = runtime.saved.profiles.profiles.iter().filter(|profile|
        profile.enabled && profile.kind == ProviderKind::Plex
            && plx_plex::plex::id_of_machine(&profile.source) == Some(sid));
    let profile = profiles.next().cloned().ok_or("The target Plex profile is unavailable")?;
    if profiles.next().is_some() { return Err("The target Plex profile is ambiguous"); }
    profile.validate().map_err(|_| "The target Plex profile is unavailable")?;
    transient::parental_profile_authority(runtime, &profile)?
        .ok_or("The target Plex source authority is unavailable")?;
    Ok(profile)
}

pub(super) fn authorize(runtime: &mut Runtime, app: &mut App,
    sid: plx_plex::plex::ServerId, rating_key: &str) -> Result<(), &'static str>
{
    parental_runtime::observe_session(runtime, app);
    input_policy::replace_source(runtime)?;
    let policy = parental::Policy::read(&runtime.saved.settings)
        .map_err(|_| "Parental settings are unavailable")?;
    // A stale retained binding must not disappear behind its filtered profile
    // accessor, even when parental protection was explicitly disabled.
    let _ = classic_source::parental_authority(runtime)?;
    let existing = classic_source::profile(runtime).or_else(|| managed::profile(runtime)).cloned();
    if let Some(profile) = existing.as_ref() {
        classic_source::guard_profile(runtime, app, profile)?;
        managed::guard_profile(runtime, app, profile)?;
    }
    if !policy.protects(parental::Target::Channel { marked: true }) { return Ok(()); }
    let already_bound = existing.as_ref().is_some_and(|profile| profile.kind == ProviderKind::Plex
        && plx_plex::plex::id_of_machine(&profile.source) == Some(sid));
    if !already_bound && (runtime.playing.is_some() || runtime.play_target.is_some()
        || runtime.pending_play.is_some() || app.adapters.player.is_live())
    {
        let target = configured_target(runtime, app, sid)?;
        // The old source remains intact until the target-specific PIN grant is
        // verified. Catalogue preparation and native stop are later effects.
        runtime.require_parental_entry(&target, None)?;
        let authority = plx_plex::plex::client_for(sid)
            .and_then(|client| crate::app::ott_plex_prepare::CapturedAuthority::capture(client).ok())
            .ok_or("The target Plex source authority is unavailable")?;
        if !classic_source::ensure_server_catalog(runtime, app, sid)? {
            return Err("The selected source is unavailable");
        }
        let bound = classic_source::profile(runtime).or_else(|| managed::profile(runtime))
            .ok_or("The selected source is unavailable")?;
        if bound != &target || !plx_plex::plex::client_for(sid)
            .is_some_and(|client| authority.matches(client))
        { return Err("The target source changed before playback"); }
        classic_source::guard_profile(runtime, app, bound)?;
        managed::guard_profile(runtime, app, bound)?;
        // Binding must preserve only the explicitly verified target grant.
        // The eventual native Load checks this gate and route identity again.
        return runtime.require_parental_entry(&target, None);
    }
    if !classic_source::ensure_server_catalog(runtime, app, sid)? {
        return Err("The selected source is unavailable");
    }
    let profile = classic_source::profile(runtime).or_else(|| managed::profile(runtime))
        .cloned().ok_or("The selected source is unavailable")?;
    if profile.kind != ProviderKind::Plex || runtime.effective_profile() != Some(&profile)
        || plx_plex::plex::id_of_machine(&profile.source) != Some(sid)
    { return Err("The selected source changed before playback"); }
    classic_source::guard_profile(runtime, app, &profile)?;
    managed::guard_profile(runtime, app, &profile)?;
    let mut matches = runtime.catalog.entries.iter().filter(|entry|
        matches!(&entry.media, MediaLocator::Plex { server_id, rating_key: key }
            if server_id == &profile.source && key == rating_key));
    let entry = matches.next().cloned();
    let entry = if matches.next().is_none() { entry } else { None };
    runtime.require_parental_entry(&profile, entry.as_ref())
}

pub(super) fn allowed(app: &mut App, sid: plx_plex::plex::ServerId, rating_key: &str) -> bool {
    allowed_with_ui(app, sid, rating_key, false)
}

pub(super) fn ui_allowed(app: &mut App, sid: plx_plex::plex::ServerId, rating_key: &str) -> bool {
    allowed_with_ui(app, sid, rating_key, true)
}

fn allowed_with_ui(app: &mut App, sid: plx_plex::plex::ServerId, rating_key: &str, interactive: bool) -> bool {
    let Some(mut runtime) = app.ott.take() else { return false; };
    let result = authorize(&mut runtime, app, sid, rating_key);
    if let Err(message) = result {
        if interactive && parental_runtime::is_pin_required(message) {
            let profile = classic_source::profile(&runtime).or_else(|| managed::profile(&runtime))
                .filter(|profile| profile.kind == ProviderKind::Plex
                    && plx_plex::plex::id_of_machine(&profile.source) == Some(sid)).cloned()
                .or_else(|| configured_target(&runtime, app, sid).ok());
            if let Some(profile) = profile
            {
                parental_runtime::request_foreign_pin(&mut runtime, app, &profile);
            }
        }
        runtime.status = message.into();
        runtime.changed();
    }
    app.ott = Some(runtime);
    result.is_ok()
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_parental_foreign_tests.rs"]
mod tests;

/// The app calls this again after a Plex worker lands, immediately before a
/// native Load (including a queued release). OTT-owned sources are rebound
/// from their retained owner before the independent legacy Plex fallback.
pub(super) fn native_start_allowed(app: &mut App) -> bool {
    if let Some(mut runtime) = app.ott.take() {
        let identity = plx_media::route::playback_identity(&app.player.session);
        let request = plx_media::route::playback_request_identity(&app.player.session)
            .map(|(sid, key)| (sid, key.to_owned()));
        let source = plx_media::route::external_source(&app.player.session);
        let ticket = plx_media::route::play_request_ticket();
        enum Owner { Catalogue, Multiview, Companion }
        let (owner, admitted) = if let Some(result) = native_start_admission::owned(&mut runtime, app) {
            (Owner::Catalogue, Some(result))
        } else if let Some(result) = multiview_output::native_start_admission(&mut runtime, app) {
            (Owner::Multiview, Some(result))
        } else {
            (Owner::Companion, companion_adapter::native_start_admission(&mut runtime, app))
        };
        if let Some(result) = admitted {
            if let Err(message) = result {
                if let Some(identity) = identity.as_ref() {
                    match owner {
                        Owner::Catalogue => native_start_admission::retire_denied(&mut runtime, app, identity, ticket),
                        Owner::Multiview => multiview_output::retire_native_start(&mut runtime, app, identity, ticket),
                        Owner::Companion => companion_adapter::retire_native_start(&mut runtime, app, identity, ticket),
                    }
                }
                runtime.status = message.into(); runtime.changed();
            }
            app.ott = Some(runtime);
            return result.is_ok()
                && plx_media::route::playback_identity(&app.player.session) == identity
                && plx_media::route::playback_request_identity(&app.player.session)
                    .map(|(sid, key)| (sid, key.to_owned())) == request
                && plx_media::route::external_source(&app.player.session) == source
                && plx_media::route::play_request_is_current(ticket);
        }
        app.ott = Some(runtime);
    }
    let Some((sid, key)) = plx_media::route::playback_request_identity(&app.player.session)
        .map(|(sid, key)| (sid, key.to_owned())) else {
            return false;
        };
    if !sid.is_set() || key.is_empty() { return false; }
    let identity = plx_media::route::playback_identity(&app.player.session);
    let ticket = plx_media::route::play_request_ticket();
    allowed(app, sid, &key)
        && plx_media::route::playback_request_identity(&app.player.session) == Some((sid, key.as_str()))
        && plx_media::route::playback_identity(&app.player.session) == identity
        && plx_media::route::play_request_is_current(ticket)
}

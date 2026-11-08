//! In-memory source identity for the existing parental grant. Queue ownership
//! and inventory receipts are not source identities; no new lifetime is minted.
use super::*;
use crate::app::ott_plex_prepare::CapturedAuthority;

#[derive(Clone)]
pub(in crate::app::ott_runtime) struct SourceAuthority(Authority);

#[derive(Clone)]
enum Authority {
    Local(LocalSourceRead),
    Plex(Arc<CapturedAuthority>),
    Classic(classic_source::ClassicAuthority),
}

impl SourceAuthority {
    /// Structural comparison only. Capture must succeed again before an old
    /// grant is reused; revocation never changes equality of retained values.
    pub(in crate::app::ott_runtime) fn same_authority(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Authority::Local(a), Authority::Local(b)) => {
                a.selection.source_id == b.selection.source_id
                    && a.selection.authority == b.selection.authority
                    && a.selection.generation == b.selection.generation
                    && a.source.descriptor().kind == b.source.descriptor().kind
                    && a.same_grant(b)
            }
            (Authority::Plex(a), Authority::Plex(b)) => {
                a.server == b.server
                    && a.machine_id == b.machine_id
                    && a.client_key == b.client_key
                    && a.grant_epoch == b.grant_epoch
            }
            (Authority::Classic(a), Authority::Classic(b)) => a.same_authority(b),
            _ => false,
        }
    }
}

fn local_authority(read: &LocalSourceRead) -> SourceAuthority {
    SourceAuthority(Authority::Local(read.clone()))
}

/// Runner-side snapshot of an already-authorized effective source. This opens
/// no file and performs no HTTP. Unknown or revoked Local/Plex is an error,
/// never equivalent to an ordinary provider with no extra source witness.
pub(in crate::app::ott_runtime) fn parental_source_authority(
    runtime: &Runtime,
) -> Result<Option<SourceAuthority>, &'static str> {
    if let Some(authority) = classic_source::parental_authority(runtime)? {
        return Ok(Some(SourceAuthority(Authority::Classic(authority))));
    }
    let Some(profile) = runtime.effective_profile() else {
        return if matches!(
            runtime.effective_provider(),
            ProviderKind::Local | ProviderKind::Plex
        ) {
            Err("Source profile is unavailable")
        } else {
            Ok(None)
        };
    };
    if !matches!(profile.kind, ProviderKind::Local | ProviderKind::Plex) {
        return Ok(None);
    }
    if !profile.enabled {
        return Err("Source profile is disabled");
    }
    if profile.kind == ProviderKind::Local {
        if let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_ref() {
            let source = runtime
                .current_local_source(&session.read.selection)
                .ok_or("Local source is unavailable")?;
            if source.canonical_path.as_deref() != Some(profile.source.as_str())
                || !runtime.local_source_read_is_current(&session.read)
            {
                return Err("Local source changed");
            }
            return Ok(Some(local_authority(&session.read)));
        }
        let selection = runtime
            .local_profile_selection(profile)
            .ok_or("Local source is unavailable or ambiguous")?;
        let read = runtime
            .capture_local_source(&selection)
            .ok_or("Local source changed")?;
        return Ok(Some(local_authority(&read)));
    }

    if !runtime
        .saved
        .profiles
        .profiles
        .iter()
        .any(|saved| saved == profile && saved.enabled)
    {
        return Err("Plex source profile changed");
    }
    let sources = runtime
        .saved
        .plex_sources()
        .map_err(|_| "Plex configuration is unavailable")?;
    let mut matching = sources
        .iter()
        .filter(|source| source.machine_id == profile.source);
    let source = matching.next().ok_or("Plex source is unavailable")?;
    if matching.next().is_some() {
        return Err("Plex source is ambiguous");
    }
    let client = plx_plex::plex::id_of_machine(&profile.source)
        .and_then(plx_plex::plex::client_for)
        .ok_or("Plex source is unavailable")?;
    if source.origin().as_ref() != Some(client.origin()) || !client.uses_token(&source.token) {
        return Err("Plex source credentials changed");
    }
    let authority = match runtime.transient.accepted.as_ref() {
        Some(TransientMedia::Plex(session)) => session.authority.clone(),
        _ => {
            Arc::new(CapturedAuthority::capture(client).map_err(|_| "Plex source is unavailable")?)
        }
    };
    if authority.machine_id != profile.source || !authority.matches(client) {
        return Err("Plex source authority changed");
    }
    Ok(Some(SourceAuthority(Authority::Plex(authority))))
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_transient_parental_tests.rs"]
mod tests;

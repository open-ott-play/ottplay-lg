//! Source-bound archive intent carried by the existing playback resolver.
use super::{profile_fingerprint, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};
use plx_data::ott::{archive, guide::ChannelMetadata};

#[derive(Clone, PartialEq, Eq)]
struct ProgrammeMetadata {
    channel: String,
    title: String,
    description: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Intent {
    profile_fingerprint: String,
    entry_id: String,
    live_url: String,
    media: String,
    channel: ChannelMetadata,
    fallback_seconds: u64,
    interval: archive::ArchiveInterval,
    programme: Option<ProgrammeMetadata>,
}

impl std::fmt::Debug for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveIntent")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

fn media_fingerprint(entry: &CatalogEntry) -> Result<String, &'static str> {
    let value = serde_json::to_vec(&entry.media).map_err(|_| "Invalid archive media binding")?;
    Ok(plx_data::ott::favorites_wire::hash(&value))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorArchive {
    operator: String,
    mode: String,
    url: String,
    first: String,
    second: String,
    streammode: u8,
}
fn operator_archive(profile: &ProviderProfile, source: &archive::CatchupSource) -> Option<OperatorArchive> {
    let ProviderKind::Operator(id) = profile.kind else { return None; };
    if source.mode != archive::CatchupMode::Provider || source.source.len() > 65536 || source.correction_seconds != 0 { return None; }
    let policy: OperatorArchive = serde_json::from_str(&source.source).ok()?;
    if policy.operator != id.canonical_id() || !matches!(policy.mode.as_str(),
        "utc"|"auto-utc-now"|"only4"|"itv"|"club"|"archive"|"antifriz"|"kb")
        || policy.streammode > 9 || plx_data::ott::http_url(&policy.url).is_none()
        || [&policy.url,&policy.first,&policy.second].iter().any(|s| s.len()>16384 || s.chars().any(char::is_control)) { return None; }
    Some(policy)
}

impl Intent {
    pub(super) fn new(
        profile: &ProviderProfile,
        entry: &CatalogEntry,
        interval: archive::ArchiveInterval,
        now: i64,
    ) -> Result<Self, &'static str> {
        Self::new_with_fallback(profile, entry, interval, now, 0)
    }

    /// Profile depth fills only a supported source whose depth was omitted.
    /// The original channel remains the binding; no catalog metadata is mutated.
    pub(super) fn new_with_fallback(
        profile: &ProviderProfile,
        entry: &CatalogEntry,
        interval: archive::ArchiveInterval,
        now: i64,
        fallback_hours: u64,
    ) -> Result<Self, &'static str> {
        if fallback_hours > 8760 {
            return Err("Use whole archive hours from 0 to 8760");
        }
        let url = match (&profile.kind, &entry.media) {
            (_, MediaLocator::Http { url }) => url.clone(),
            (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. }) if stream_kind == "live" => String::new(),
            (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. }) if stream_kind == "itv" => String::new(),
            _ => return Err("Archive requires a supported live source"),
        };
        let channel = entry.live.clone().ok_or("Channel has no archive")?;
        let fallback_seconds = if channel
            .catchup
            .as_ref()
            .is_some_and(|source| source.retention_seconds == 0)
        {
            fallback_hours * 3600
        } else {
            0
        };
        let intent = Self {
            profile_fingerprint: profile_fingerprint(profile),
            entry_id: entry.id.clone(),
            live_url: url,
            media: media_fingerprint(entry)?,
            channel,
            fallback_seconds,
            interval,
            programme: None,
        };
        intent.validate(profile, entry, now)?;
        Ok(intent)
    }

    /// Presentation only: the interval must already match the admitted intent.
    pub(super) fn with_programme(
        mut self,
        programme: &plx_data::ott::epg::Programme,
    ) -> Result<Self, &'static str> {
        if programme.start != self.interval.start || programme.end != self.interval.end {
            return Err("Archive programme does not match the selected interval");
        }
        self.programme = Some(ProgrammeMetadata {
            channel: programme.channel.clone(),
            title: programme.title.clone(),
            description: programme.description.clone(),
        });
        Ok(self)
    }

    pub(super) fn programme(&self) -> Option<plx_data::ott::epg::Programme> {
        self.programme
            .as_ref()
            .map(|programme| plx_data::ott::epg::Programme {
                channel: programme.channel.clone(),
                start: self.interval.start,
                end: self.interval.end,
                title: programme.title.clone(),
                description: programme.description.clone(),
            })
    }

    pub(super) fn uses_profile_fallback_for(&self, profile: &ProviderProfile) -> bool {
        self.fallback_seconds != 0 && self.profile_fingerprint == profile_fingerprint(profile)
    }

    pub(super) fn interval(&self) -> archive::ArchiveInterval {
        self.interval
    }

    pub(super) fn provider_route(&self) -> bool {
        self.channel.catchup.as_ref().is_some_and(|source| source.mode == archive::CatchupMode::Provider)
    }

    pub(super) fn validate_binding(
        &self,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
    ) -> Result<(), &'static str> {
        let provider_archive = self.channel.catchup.as_ref().is_some_and(|source|
            source.mode == archive::CatchupMode::Provider && source.correction_seconds == 0 &&
            matches!((&profile.kind, &entry.media, source.source.as_str()),
                (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. }, "xtream") if stream_kind == "live")
            || source.mode == archive::CatchupMode::Provider && source.correction_seconds == 0 &&
            matches!((&profile.kind, &entry.media, source.source.as_str()),
                (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. }, "stalker") if stream_kind == "itv"));
        let operator_archive = matches!(entry.media, MediaLocator::Http { .. })
            && self.channel.catchup.as_ref().is_some_and(|source| operator_archive(profile,source).is_some());
        if !(provider_archive || operator_archive || matches!(profile.kind, ProviderKind::M3u | ProviderKind::Edem | ProviderKind::Operator(_))
            && matches!(entry.media, MediaLocator::Http { .. })
            && self.channel.catchup.as_ref().is_some_and(|source| source.mode != archive::CatchupMode::Provider))
            || !profile.enabled
            || profile_fingerprint(profile) != self.profile_fingerprint
            || entry.id != self.entry_id
            || media_fingerprint(entry)? != self.media
            || entry.live.as_ref() != Some(&self.channel)
            || self.channel.catchup.is_none()
        {
            return Err("Archive source changed or is unavailable");
        }
        Ok(())
    }

    pub(super) fn validate(
        &self,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
        now: i64,
    ) -> Result<(), &'static str> {
        if self.channel.catchup.as_ref().is_some_and(|source| source.mode == archive::CatchupMode::Provider) {
            self.validate_binding(profile, entry)?;
            let retention = self.channel.catchup.as_ref().and_then(|source| i64::try_from(source.retention_seconds).ok())
                .filter(|retention| *retention > 0).ok_or("Archive retention is unavailable")?;
            if now < 0 || self.interval.start < 0 || self.interval.end <= self.interval.start
                || self.interval.end > now || self.interval.end - self.interval.start > 90_000
                || self.interval.start < now.saturating_sub(retention) { return Err("Archive interval is expired or unavailable"); }
            Ok(())
        } else { self.apply(profile, entry, &self.live_url, now).map(|_| ()) }
    }

    /// Resolve the live locator first, then expand only this source's archive rule.
    pub(super) fn apply(
        &self,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
        fresh_live_url: &str,
        now: i64,
    ) -> Result<String, &'static str> {
        self.validate_binding(profile, entry)?;
        let mut source = self
            .channel
            .catchup
            .clone()
            .ok_or("Channel has no archive")?;
        if source.retention_seconds == 0 {
            source.retention_seconds = self.fallback_seconds;
        }
        archive::plan(&source, fresh_live_url, self.interval, now)
            .map(|target| target.url)
            .map_err(|_| "Archive interval is expired or unavailable")
    }
}

pub(super) fn epoch_seconds(unix_ms: u64) -> Result<i64, &'static str> {
    i64::try_from(unix_ms / 1000).map_err(|_| "Archive clock is unavailable")
}

/// A provider's archive endpoint is independent of the current live stream.
/// Validate the captured source both before and after the bounded provider IO.
pub(super) fn resolve_provider(
    intent: &Intent, profile: &ProviderProfile, entry: &CatalogEntry,
    worker: &plx_base::task::OffFrame, cancel: &std::sync::atomic::AtomicBool,
) -> Result<crate::app::ott_catalog::Resolved, &'static str> {
    if !intent.provider_route() { return Err("Provider archive route unavailable"); }
    let now = || plx_base::task::time::ClockSample::capture(worker)
        .map_err(|_| "Archive clock is unavailable").and_then(|clock| epoch_seconds(clock.unix_ms()));
    if cancel.load(std::sync::atomic::Ordering::Acquire) { return Err("Provider request cancelled"); }
    let observed = now()?;
    intent.validate(profile, entry, observed)?;
    let url = if let Some(policy) = intent.channel.catchup.as_ref().and_then(|source| operator_archive(profile,source)) {
        plx_shared_core::operators::archive_url(&policy.mode,&policy.url,&policy.first,&policy.second,
            intent.interval.start,intent.interval.end,observed,policy.streammode)
            .map_err(|_| "Operator archive interval is unavailable")?
    } else {
        crate::app::ott_provider_http::resolve_archive_cancellable(worker, profile, entry, intent.interval, cancel)?
    };
    if cancel.load(std::sync::atomic::Ordering::Acquire) { return Err("Provider request cancelled"); }
    intent.validate(profile, entry, now()?)?;
    let url = plx_data::ott::http_url(&url).ok_or("Invalid archive transport")?;
    Ok(crate::app::ott_catalog::Resolved { url, lease: None })
}

pub(super) fn resolved(
    intent: &Intent,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    mut resolved: crate::app::ott_catalog::Resolved,
    worker: &plx_base::task::OffFrame,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<crate::app::ott_catalog::Resolved, &'static str> {
    if intent.provider_route() && resolved.lease.is_none() {
        return resolve_provider(intent, profile, entry, worker, cancel);
    }
    // Lease renewal would require a separate archive continuation contract.
    // This slice admits direct M3U sources only, and retires unsupported leases.
    let result = if resolved.lease.is_some() {
        Err("Archive requires a direct HTTP source")
    } else {
        plx_base::task::time::ClockSample::capture(worker)
            .map_err(|_| "Archive clock is unavailable")
            .and_then(|clock| epoch_seconds(clock.unix_ms()))
            .and_then(|now| intent.apply(profile, entry, &resolved.url, now))
    };
    match result {
        Ok(url) => {
            resolved.url = url;
            Ok(resolved)
        }
        Err(error) => {
            if let Some(lease) = resolved.lease {
                lease.finish_off_frame()?;
            }
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "ott_archive_play_tests.rs"]
mod tests;

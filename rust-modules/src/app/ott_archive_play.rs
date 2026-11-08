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
        let MediaLocator::Http { url } = &entry.media else {
            return Err("Archive requires an explicit M3U source");
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
            live_url: url.clone(),
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

    pub(super) fn validate_binding(
        &self,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
    ) -> Result<(), &'static str> {
        if profile.kind != ProviderKind::M3u
            || !profile.enabled
            || profile_fingerprint(profile) != self.profile_fingerprint
            || entry.id != self.entry_id
            || !matches!(&entry.media, MediaLocator::Http { url } if url == &self.live_url)
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
        self.apply(profile, entry, &self.live_url, now).map(|_| ())
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

pub(super) fn resolved(
    intent: &Intent,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    mut resolved: crate::app::ott_catalog::Resolved,
    worker: &plx_base::task::OffFrame,
) -> Result<crate::app::ott_catalog::Resolved, &'static str> {
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

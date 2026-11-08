//! Per-profile M3U archive fallback: the ordinary editor and durable Saved path.
use super::*;

const KEY: &str = "profile.history_hours";
const INVALID: &str = "Use whole archive hours from 0 to 8760";
const CHANGED: &str = "Archive settings changed; reopen the editor";

pub(super) struct Editor {
    profile: ProviderProfile,
    stored: Option<Value>,
    generation: u64,
}

fn storage_key(profile: &ProviderProfile) -> String { format!("history:{}", profile.id) }

fn hours(saved: &Saved, profile: &ProviderProfile) -> u64 {
    saved.settings.get(&storage_key(profile)).and_then(Value::as_u64)
        .filter(|hours| *hours <= 8760).unwrap_or(0)
}

impl Runtime {
    pub(super) fn archive_hours_profile(&self) -> Option<&ProviderProfile> {
        self.provider_profile().filter(|profile| profile.kind == ProviderKind::M3u
            && plx_data::ott::m3u_source_kind(&profile.source)
                == Ok(plx_data::ott::M3uSourceKind::Playlist))
    }

    pub(super) fn archive_hours_label(&self) -> String {
        self.archive_hours_profile().map(|profile| hours(&self.saved, profile).to_string())
            .unwrap_or_default()
    }

    pub(super) fn begin_archive_hours(&mut self) -> Result<String, &'static str> {
        let profile = self.archive_hours_profile().cloned().ok_or("Select an M3U television profile")?;
        let value = hours(&self.saved, &profile).to_string();
        self.archive_hours_editor = Some(Editor {
            stored: self.saved.settings.get(&storage_key(&profile)).cloned(),
            profile,
            generation: self.generation,
        });
        Ok(value)
    }

    pub(super) fn archive_hours_candidate(&self, value: &str) -> Result<Saved, &'static str> {
        let value = value.trim();
        let amount = value.parse::<u64>().ok().filter(|amount| *amount <= 8760)
            .filter(|_| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or(INVALID)?;
        let editor = self.archive_hours_editor.as_ref().ok_or(CHANGED)?;
        if self.generation != editor.generation
            || self.archive_hours_profile() != Some(&editor.profile)
            || self.saved.settings.get(&storage_key(&editor.profile)) != editor.stored.as_ref()
        { return Err(CHANGED); }
        let mut candidate = self.saved.clone();
        candidate.settings.insert(storage_key(&editor.profile), json!(amount));
        candidate.validate()?;
        Ok(candidate)
    }

    pub(super) fn complete_archive_hours(&mut self, session: &EditorSession) {
        if session.key != KEY { return; }
        if let Some(editor) = self.archive_hours_editor.as_mut() {
            editor.stored = self.saved.settings.get(&storage_key(&editor.profile)).cloned();
        }
    }

    /// A pending request that used the previous fallback must not outlive its setting.
    /// Installed playback remains owned by the ordinary playback lifecycle.
    pub(super) fn retire_changed_archive_fallback(&mut self, next: &Saved) {
        let changed = self.play_target.as_ref().and_then(|request| request.archive.as_ref())
            .is_some_and(|intent| self.saved.profiles.profiles.iter().any(|profile|
                intent.uses_profile_fallback_for(profile)
                    && hours(&self.saved, profile) != hours(next, profile)));
        if changed { self.cancel_play_request(); }
    }

    pub(super) fn archive_fallback_hours(&self, profile: &ProviderProfile) -> u64 {
        self.saved.profiles.profiles.iter().find(|saved| *saved == profile)
            .map_or(0, |profile| hours(&self.saved, profile))
    }

    pub(super) fn archive_intent(&self, profile: &ProviderProfile, entry: &CatalogEntry,
        interval: plx_data::ott::archive::ArchiveInterval, now: i64)
        -> Result<archive_play::Intent, &'static str>
    {
        let fallback = self.archive_fallback_hours(profile);
        if fallback == 0 {
            archive_play::Intent::new(profile, entry, interval, now)
        } else {
            archive_play::Intent::new_with_fallback(profile, entry, interval, now, fallback)
        }
    }
}

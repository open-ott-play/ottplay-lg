//! Runtime admission for private, lazy M3U media attachments. No IO is started
//! while observing settings; the existing Favorite workers own all requests.
use super::*;
use crate::app::ott_m3u_attachment::{LegacySlot, Receipt, Registry, SETTINGS_KEY};

impl Runtime {
    pub(super) fn attachment_for(&self, favorite: &Favorite) -> Result<Option<Receipt>, &'static str> {
        let bindings = self.favorite_bindings(favorite).map_err(selection::BindingError::message)?;
        match selection::bind_bound(&bindings, favorite).map_err(selection::BindingError::message)? {
            selection::SelectionOwner::M3uAttachment(receipt) => Ok(Some(receipt.clone())),
            _ => Ok(None),
        }
    }

    pub(super) fn attachment_policy_profile(&self, profile: &ProviderProfile, favorite: Option<&Favorite>)
        -> Result<ProviderProfile, &'static str>
    {
        if let Some(receipt) = favorite.map(|favorite| self.attachment_for(favorite)).transpose()?.flatten() {
            if !self.m3u_attachments.is_current(&receipt)
                || receipt.effective_profile().map_err(|error| error.message())? != *profile
            {
                return Err("M3U media attachment changed before playback");
            }
            Ok(receipt.parent().clone())
        } else {
            Ok(profile.clone())
        }
    }

    /// Call synchronously for each accepted Saved image, even if several images
    /// arrive in one tick. An A -> B -> A transition must retire A's receipt.
    pub(super) fn observe_m3u_attachments(&mut self) {
        let active_id = self.active().map(|profile|profile.id.clone());
        let before = active_id.as_deref().and_then(|id|self.m3u_attachments.capture(id));
        let observed = Registry::read(&self.saved.settings, &self.saved.profiles)
            .and_then(|registry| self.m3u_attachments.observe(&registry, &self.saved.profiles));
        match observed {
            Ok(_) => {},
            Err(error) => {
                self.m3u_attachments.invalidate_all();
                self.status = error.message().into();
            }
        }
        let after = active_id.as_deref().and_then(|id|self.m3u_attachments.capture(id));
        if before != after || self.m3u_attachments.context_revision() == 0 {
            self.retire_parental_source();
        }
        if !self.pending_attachment_current() {
            self.cancel_play_request();
        }
        if !self.metadata_attachment_current() {
            self.metadata_cancel.store(true, Ordering::Release);
            self.metadata_generation = self.metadata_generation.wrapping_add(1);
            self.metadata_target = None;
            self.metadata_attachment = None;
            self.metadata_next = None;
            self.favorite_preview = None;
        }
    }

    pub(super) fn take_completed_play_target(&mut self) -> Option<PlaybackRequest> {
        self.play_attachment = None;
        self.play_target.take()
    }

    pub(super) fn pending_attachment_current(&self) -> bool {
        self.play_attachment.as_ref().is_none_or(|receipt| self.play_target.as_ref()
            .is_some_and(|target| match target.kiosk_selection.as_ref() {
                Some(_) => kiosk_source::pinned_attachment_current(self, target, receipt),
                None => self.attachment_selection_current(receipt, &target.id),
            }))
    }

    pub(super) fn attachment_selection_current(&self, receipt: &Receipt, id: &str) -> bool {
        id.strip_prefix("favorite:").and_then(|identity|self.saved.favorites.entries().iter()
            .find(|favorite|favorite.identity()==identity)).is_some_and(|favorite|
                self.favorite_bindings(favorite).is_ok_and(|bindings|
                    bindings.attachment_is_current(receipt,favorite)))
    }

    pub(super) fn metadata_attachment_current(&self) -> bool {
        self.metadata_attachment.as_ref().is_none_or(|receipt|self.metadata_target.as_ref()
            .is_some_and(|(_,id)|self.attachment_selection_current(receipt,id)))
    }

    pub(super) fn reconcile_m3u_attachment(&mut self, app: &mut App) {
        let Some(receipt) = self.playing_attachment.as_ref() else { return; };
        let Some(playing) = self.playing.as_ref() else {
            self.playing_attachment = None;
            return;
        };
        if receipt.effective_profile().ok().as_ref() != Some(&playing.profile) {
            self.playing_attachment = None;
            return;
        }
        if self.m3u_attachments.is_current(receipt) { return; }
        // Do not stop an unrelated native route that has already replaced us.
        if playing_matches(playing, app) {
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
        }
        self.retire_catalog_playback(app);
        self.playing_attachment = None;
        self.status = "M3U media attachment changed; select the Favorite again".into();
    }

    pub(super) fn metadata_profile_matches(&self, playing: &Playing, profile_id: &str) -> bool {
        playing.profile.id == profile_id || self.metadata_attachment.as_ref().is_some_and(|receipt|
            self.m3u_attachments.is_current(receipt) && receipt.parent().id == profile_id
                && receipt.effective_profile().ok().as_ref() == Some(&playing.profile))
    }

    pub(super) fn attachment_candidate(&self, parent: &ProviderProfile, value: &str, new_id: Option<&str>)
        -> Result<Saved, &'static str>
    {
        if self.saved.profiles.profiles.iter().find(|profile| profile.id == parent.id) != Some(parent) {
            return Err(crate::app::ott_m3u_attachment::Error::StaleParent.message());
        }
        let registry = Registry::read(&self.saved.settings, &self.saved.profiles)
            .map_err(|error| error.message())?;
        let mut candidate = self.saved.clone();
        if parent.kind == ProviderKind::M3u
            && plx_data::ott::m3u_source_kind(&parent.source) == Ok(plx_data::ott::M3uSourceKind::PortalOnly)
        {
            if !parent.enabled || plx_data::ott::parse_vportal_link(value).is_err() {
                return Err(crate::app::ott_m3u_attachment::Error::InvalidLink.message());
            }
            // A media-only parent cannot survive clearing its only real source.
            // Both fields advance together in this candidate, never in live state.
            candidate.profiles.profiles.iter_mut().find(|profile| profile.id == parent.id)
                .expect("captured parent checked above").source = value.trim().into();
            let next = registry.reconcile_sources(&self.saved.profiles, &candidate.profiles,
                &mut || new_id.map(str::to_owned).ok_or(crate::app::ott_m3u_attachment::Error::MissingId))
                .map_err(|error| error.message())?;
            next.put_into(&mut candidate.settings, &candidate.profiles).map_err(|error| error.message())?;
        } else {
            let legacy_source_id = registry.for_parent(&parent.id).and_then(|attachment|attachment.legacy_source_id());
            let slot = LegacySlot { playlist: &parent.source, media_url: Some(value), media_source_id: legacy_source_id };
            let next = registry.migrate(&self.saved.profiles, parent, slot, new_id)
                .map_err(|error| error.message())?;
            next.put_into(&mut candidate.settings, &candidate.profiles).map_err(|error| error.message())?;
        }
        candidate.validate()?;
        Ok(candidate)
    }

    pub(super) fn prune_m3u_attachments(&mut self, candidate: &mut Saved) -> Result<(), &'static str> {
        let unchanged = candidate.settings.get(SETTINGS_KEY) == self.saved.settings.get(SETTINGS_KEY);
        if !unchanged {
            // An explicit imported/replaced registry must already describe its
            // candidate parents exactly. Never repair or drop its mismatches.
            Registry::read(&candidate.settings, &candidate.profiles).map_err(|error| error.message())?;
            return Ok(());
        }
        let registry = Registry::read(&self.saved.settings, &self.saved.profiles)
            .map_err(|error| error.message())?;
        let previous = self.saved.profiles.clone();
        let next = registry.reconcile_sources(&previous, &candidate.profiles, &mut ||
            super::favorites::nonce(self)
                .map_err(|_| crate::app::ott_m3u_attachment::Error::MissingId))
            .map_err(|error| error.message())?;
        // Preserve old documents which never had an attachment setting at all.
        if candidate.settings.contains_key(SETTINGS_KEY) || next != Registry::default() {
            next.put_into(&mut candidate.settings, &candidate.profiles).map_err(|error| error.message())?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "ott_m3u_attachment_runtime_tests.rs"]
mod tests;

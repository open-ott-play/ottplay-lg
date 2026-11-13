//! Multi-window admission reuses the ordinary source-owned gates. Geometry,
//! a saved profile name and a compositor response never confer playback rights.
use super::*;

impl Runtime {
    /// Run at capture and again after resolution, before any input is handed to
    /// the compositor. The source broker separately rebinds its exact snapshot.
    /// This method never queues playback or changes the selected provider.
    pub(super) fn authorize_multiview_source(
        &mut self,
        app: &App,
        request: &PlaybackRequest,
        profile: &ProviderProfile,
        entry: Option<&CatalogEntry>,
        favorite: Option<&Favorite>,
        resolved: Option<&CatalogEntry>,
    ) -> Result<(), &'static str> {
        input_policy::replace_source(self)?;
        // Existing kiosk pins describe a single source/queue, not permission to
        // add sibling inputs. Even an armed pin cannot be consumed by a scene.
        if !matches!(
            self.track_selection_kiosk(),
            Ok(crate::app::ott_kiosk_policy::State::Off)
        ) {
            return Err("Multiview is unavailable while kiosk policy is enabled");
        }
        if !app.running || !self.play_work_allowed() || !playback_allowed(app) {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        profile.validate().map_err(|_| "Invalid provider profile")?;
        profile_rename::guard(self, profile)?;
        managed::guard_profile(self, app, profile)?;
        classic_source::guard_profile(self, app, profile)?;
        if !transient::lifecycle_target_ready(self, profile, request.local_selection.as_ref()) {
            return Err("The local source is still being validated");
        }
        let policy = self.attachment_policy_profile(profile, favorite)?;
        self.require_parental_preparation(&policy, profile, entry, favorite)?;
        if let Some(resolved) = resolved {
            self.require_parental_resolved(&policy, profile, entry, favorite, resolved)?;
        }
        Ok(())
    }
}

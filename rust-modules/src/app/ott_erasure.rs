//! The UI revokes producers; the existing persistence FIFO performs erasure.
use super::*;

pub(crate) struct Erasure {
    companion: super::super::ott_companion::ErasureStop,
    retirement_failures: Vec<String>,
}

impl Erasure {
    pub(crate) fn without_runtime() -> Self {
        Self {
            companion: super::super::ott_companion::ErasureStop::owned_data_without_worker(
                private::root().with_file_name("ott-companion"),
            ),
            retirement_failures: revoke_imports(),
        }
    }

    /// Called only by the persistence worker, never while handling a frame.
    /// The owner retains this context after a partial result so a retry can
    /// finish waiting for the same helper instead of losing its stop evidence.
    pub(crate) fn clear(&mut self, session_cleared: bool) -> Vec<String> {
        let mut failures = self.retirement_failures.clone();
        failures.extend(private::erase_owned_pending());
        if let Err(error) = self.companion.finish(std::time::Duration::from_secs(15)) {
            failures.push(format!("OTT companion cleanup: {error:?}"));
        }
        if session_cleared && failures.is_empty() {
            if let Err(error) = private::finish_erasure() {
                failures.push(format!("OTT erasure completion: {error:?}"));
            }
        }
        failures
    }
}

fn revoke_imports() -> Vec<String> {
    super::super::ott_import::set_authorized_sources(&[])
        .err().map(|error| format!("OTT import revocation: {error}")).into_iter().collect()
}

/// This is deliberately separate from shutdown, which preserves checkpoints.
pub(crate) fn retire_for_erasure(app: &mut App) -> Option<Erasure> {
    let mut runtime = app.ott.take()?;
    runtime.write_permit.revoke();
    runtime.remote.retire_for_erasure();
    runtime.cancel_catalog();
    runtime.cancel_play_request();
    if let Some(lease) = runtime.active_lease.take() {
        lease.begin_release();
    }
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    plx_media::route::forget_external_source(&mut app.player.session);
    app.player.pending_exit = None;
    let companion = runtime.companion.retire_for_erasure();
    let retirement_failures = revoke_imports();
    // Dropping the runtime closes all result channels. A late save, resolver,
    // metadata response or Plex restore cannot regain main-thread authority.
    drop(runtime);
    Some(Erasure { companion, retirement_failures })
}

//! A browse return retains a terminal catalogue outcome or resumes cancelled
//! work. The marker is RAM-only and belongs to one exact source/generation.
use super::*;

#[derive(Clone)]
enum Outcome {
    Reload,
    Retained {
        complete: bool,
        status: String,
        warning: Option<&'static str>,
        failure: Option<catalog_diagnostics::Failure>,
    },
}

#[derive(Clone)]
pub(super) struct Pending {
    generation: u64,
    fingerprint: String,
    outcome: Outcome,
}

impl Pending {
    pub(super) fn rebase(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    fn current(&self, runtime: &Runtime, fingerprint: &str) -> bool {
        self.generation == runtime.generation && self.fingerprint == fingerprint
    }
}

impl Runtime {
    pub(super) fn rebase_catalog_return_for_lifecycle(&mut self, next_generation: u64) {
        let Some(pending) = self.catalog_return.take() else { return; };
        if self.transient.accepted.is_some() || self.transient.candidate.is_some() { return; }
        let Some(profile) = self.effective_profile().filter(|profile| profile.enabled && matches!(profile.kind,
            ProviderKind::M3u | ProviderKind::VPortal | ProviderKind::Plex)) else { return; };
        if !pending.current(self, &profile_fingerprint(profile)) { return; }
        // Suspend retires producers, not an exact source's browse return. Carry
        // either Reload or a Retained terminal outcome across this one lifecycle
        // epoch; returning still checks the source and generation before use.
        self.catalog_return = Some(pending.rebase(next_generation));
    }

    pub(super) fn catalog_return_after_back(&self) -> Option<Pending> {
        if self.transient.accepted.is_some() || self.transient.candidate.is_some() { return None; }
        let profile = self.effective_profile().filter(|profile| profile.enabled && matches!(profile.kind,
            ProviderKind::M3u | ProviderKind::VPortal | ProviderKind::Plex))?;
        let fingerprint = profile_fingerprint(profile);
        if self.page == "profiles" {
            if let Some(pending) = self.catalog_return.as_ref().filter(|pending| pending.current(self, &fingerprint)) {
                return Some(pending.clone());
            }
        }
        let outcome = if self.page != "providers" && self.catalog_loading && !self.catalog_complete {
            Outcome::Reload
        } else if matches!(self.page, "channels" | "favorites" | "settings") && !self.catalog_loading {
            Outcome::Retained {
                complete: self.catalog_complete,
                status: self.status.clone(),
                warning: self.catalog_warning.filter(|(generation, _)| *generation == self.generation)
                    .map(|(_, warning)| warning),
                failure: if self.catalog_complete { None }
                    else { self.catalog_diagnostics.current_failure(self.generation) },
            }
        } else { return None; };
        Some(Pending { generation: self.generation, fingerprint, outcome })
    }

    pub(super) fn resume_returned_catalog(&mut self) {
        let Some(pending) = self.catalog_return.take() else { return; };
        let Some(profile) = self.effective_profile() else { return; };
        if !pending.current(self, &profile_fingerprint(profile)) { return; }
        match pending.outcome {
            Outcome::Reload => self.refresh_catalog_preserving_playback(),
            Outcome::Retained { complete, status, warning, failure } => {
                // Back cancelled the old producer. A validated complete snapshot
                // gets a fresh read/action owner without reviving that producer
                // or scheduling another fetch; failed outcomes remain cancelled.
                if complete { self.cancel = Arc::new(AtomicBool::new(false)); }
                self.catalog_complete = complete;
                self.status = status;
                self.catalog_warning = warning.map(|warning| (self.generation, warning));
                self.catalog_diagnostics.clear();
                if let Some(failure) = failure {
                    self.catalog_diagnostics.failed(self.generation, failure);
                }
            }
        }
    }
}

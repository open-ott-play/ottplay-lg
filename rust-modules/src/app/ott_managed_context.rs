//! Bounded row-context bookkeeping, not candidate authority or an activation owner.
//!
//! The coordinator supplies monotonic time, an eligible exact candidate binding,
//! and receiver-local unpredictable tokens that are never reused. Candidate
//! generations must increase on replacement and never wrap; authority/scope tags
//! are opaque equality guards, not permission to repair an ineligible candidate.
//! Operation IDs must also be unique for this store's lifetime. This module does
//! no IO, verification, policy evaluation, publication, or wire parsing.

const CONTEXT_LIFETIME_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub candidate_generation: u64,
    pub authority_tag: u64,
    pub scope_tag: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RowContext {
    pub context: String,
    pub expires_at_ms: u64,
    pub expires_in_ms: u32,
}

impl std::fmt::Debug for RowContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowContext")
            .field("context", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("expires_in_ms", &self.expires_in_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Inflight {
    pub operation: u64,
    pub deadline_ms: u64,
    pub context: String,
    pub server_id: String,
    pub runtime: String,
    pub binding: Binding,
}

impl std::fmt::Debug for Inflight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inflight")
            .field("operation", &self.operation)
            .field("deadline_ms", &self.deadline_ms)
            .field("context", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContextError {
    Busy,
    Stale,
    SourceChanged,
    Expired,
    InvalidRequest,
    LimitExceeded,
}

struct Offered {
    binding: Binding,
    runtime: String,
    server_id: String,
    context: String,
    created_at_ms: u64,
    expires_at_ms: u64,
}

impl Offered {
    fn row(&self, now_ms: u64) -> RowContext {
        RowContext {
            context: self.context.clone(),
            expires_at_ms: self.expires_at_ms,
            // Called only for created_at <= now < expires_at; at most 120_000.
            expires_in_ms: (self.expires_at_ms - now_ms) as u32,
        }
    }
}

#[derive(Default)]
pub(super) struct OfferStore {
    candidate_generation: Option<u64>,
    consumed: bool,
    offered: Option<Offered>,
    last_context: Option<String>,
    inflight: Option<Inflight>,
}

impl OfferStore {
    /// The caller has already established candidate eligibility. A fresh token
    /// is needed only when no matching unexpired offer exists. Relisting never
    /// extends the absolute expiry and cannot clear a consumed-candidate marker.
    pub fn list(
        &mut self,
        now_ms: u64,
        binding: Binding,
        runtime: &str,
        server_id: &str,
        new_token: &str,
    ) -> Result<RowContext, ContextError> {
        if self.inflight.is_some() {
            return Err(ContextError::Busy);
        }
        match self.candidate_generation {
            Some(generation) if binding.candidate_generation < generation => {
                return Err(ContextError::SourceChanged);
            }
            Some(generation) if binding.candidate_generation == generation => {
                if self.consumed {
                    return Err(ContextError::Stale);
                }
            }
            _ => {
                self.candidate_generation = Some(binding.candidate_generation);
                self.consumed = false;
                self.offered = None;
            }
        }
        if let Some(offered) = &self.offered {
            if now_ms < offered.created_at_ms {
                return Err(ContextError::Stale);
            }
            if offered.binding == binding
                && offered.runtime == runtime
                && offered.server_id == server_id
                && now_ms < offered.expires_at_ms
            {
                return Ok(offered.row(now_ms));
            }
        }
        // A changed binding or expired offer cannot survive a failed renewal.
        self.offered = None;
        if new_token.is_empty()
            || new_token.len() > 256
            || !new_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            || self.last_context.as_deref() == Some(new_token)
        {
            return Err(ContextError::InvalidRequest);
        }
        let expires_at_ms = now_ms
            .checked_add(CONTEXT_LIFETIME_MS)
            .ok_or(ContextError::LimitExceeded)?;
        let offered = Offered {
            binding,
            runtime: runtime.to_owned(),
            server_id: server_id.to_owned(),
            context: new_token.to_owned(),
            created_at_ms: now_ms,
            expires_at_ms,
        };
        let row = offered.row(now_ms);
        self.last_context = Some(offered.context.clone());
        self.offered = Some(offered);
        Ok(row)
    }

    /// Non-consuming resolution before deferred work. This checks the original
    /// offered context, not candidate eligibility, policy or invocation deadline.
    /// Admission repeats these same checks after the caller's deferred work.
    pub fn check(
        &self,
        now_ms: u64,
        binding: Binding,
        runtime: &str,
        server_id: &str,
        context: &str,
    ) -> Result<(), ContextError> {
        if self.inflight.is_some() {
            return Err(ContextError::Busy);
        }
        if self.candidate_generation != Some(binding.candidate_generation) {
            return Err(ContextError::SourceChanged);
        }
        if self.consumed {
            return Err(ContextError::Stale);
        }
        let offered = self.offered.as_ref().ok_or(ContextError::Stale)?;
        if offered.binding != binding || offered.server_id != server_id {
            return Err(ContextError::SourceChanged);
        }
        if offered.runtime != runtime || offered.context != context {
            return Err(ContextError::Stale);
        }
        if now_ms < offered.created_at_ms {
            return Err(ContextError::Stale);
        }
        if now_ms >= offered.expires_at_ms {
            return Err(ContextError::Expired);
        }
        Ok(())
    }

    /// Absolute expiry to carry through queued final admission. Resolve the
    /// original context with `check` first; this accessor does not test freshness.
    pub fn offered_deadline(&self) -> Option<u64> {
        self.offered.as_ref().map(|offered| offered.expires_at_ms)
    }

    /// Final admission only: the caller must recheck the candidate, policy,
    /// invocation, proof and registry collision before this atomic consumption.
    #[allow(clippy::too_many_arguments)]
    pub fn admit(
        &mut self,
        now_ms: u64,
        binding: Binding,
        runtime: &str,
        server_id: &str,
        context: &str,
        operation: u64,
        deadline_ms: u64,
    ) -> Result<Inflight, ContextError> {
        self.check(now_ms, binding, runtime, server_id, context)?;
        if now_ms >= deadline_ms {
            return Err(ContextError::Expired);
        }
        let offered = self.offered.as_ref().expect("check resolved an offered context");
        let inflight = Inflight {
            operation,
            deadline_ms,
            context: offered.context.clone(),
            server_id: offered.server_id.clone(),
            runtime: offered.runtime.clone(),
            binding: offered.binding,
        };
        self.offered = None;
        self.consumed = true;
        self.inflight = Some(inflight.clone());
        Ok(inflight)
    }

    /// Revokes the offered row, never the admitted operation or consumption.
    /// Candidate eligibility and a subsequent generation remain caller-owned.
    pub fn invalidate(&mut self) {
        self.offered = None;
    }

    /// Remove exactly one matching operation after the coordinator has decided
    /// its outcome. This does not assert activation, check a deadline, or restore
    /// the prior session. Offered-context expiry is irrelevant after admission.
    pub fn finish(&mut self, operation: u64) -> Option<Inflight> {
        if self.inflight.as_ref()?.operation != operation {
            return None;
        }
        self.inflight.take()
    }

    #[cfg(test)]
    pub fn inflight(&self) -> Option<&Inflight> {
        self.inflight.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINDING: Binding = Binding {
        candidate_generation: 7,
        authority_tag: 11,
        scope_tag: 13,
    };
    const RUNTIME: &str = "native-qa-1";
    const SERVER: &str = "PMS-1";
    // Deterministic inputs exercise bookkeeping only, not token unpredictability.
    const TOKEN: &str = "context-1";

    fn offered_at(now_ms: u64) -> OfferStore {
        let mut store = OfferStore::default();
        store.list(now_ms, BINDING, RUNTIME, SERVER, TOKEN).unwrap();
        store
    }

    #[test]
    fn debug_output_redacts_context_values() {
        let mut store = offered_at(0);
        let row = store.list(1, BINDING, RUNTIME, SERVER, "unused").unwrap();
        let inflight = store.admit(2, BINDING, RUNTIME, SERVER, TOKEN, 1, 100).unwrap();
        for output in [format!("{row:?}"), format!("{inflight:?}")] {
            assert!(!output.contains(TOKEN));
            assert!(output.contains("<redacted>"));
        }
    }

    #[test]
    fn preflight_check_preserves_offer_and_admission_checks_it_again() {
        let mut store = offered_at(0);
        assert_eq!(store.check(1, BINDING, RUNTIME, SERVER, TOKEN), Ok(()));
        assert_eq!(store.offered_deadline(), Some(120_000));
        assert_eq!(store.inflight(), None);
        let row = store.list(2, BINDING, RUNTIME, SERVER, "unused").unwrap();
        assert_eq!(row.context, TOKEN);
        assert_eq!(row.expires_at_ms, 120_000);
        store.invalidate();
        assert_eq!(store.offered_deadline(), None);
        assert_eq!(
            store.check(3, BINDING, RUNTIME, SERVER, TOKEN),
            Err(ContextError::Stale)
        );
        assert_eq!(
            store.admit(3, BINDING, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        assert_eq!(store.inflight(), None);
    }

    #[test]
    fn preflight_check_cannot_extend_context_through_deferred_work() {
        let mut store = offered_at(0);
        assert_eq!(store.check(119_999, BINDING, RUNTIME, SERVER, TOKEN), Ok(()));
        assert_eq!(
            store.check(120_000, BINDING, RUNTIME, SERVER, TOKEN),
            Err(ContextError::Expired)
        );
        assert_eq!(
            store.admit(120_000, BINDING, RUNTIME, SERVER, TOKEN, 1, 200_000),
            Err(ContextError::Expired)
        );
        assert_eq!(store.inflight(), None);
    }

    #[test]
    fn relist_keeps_absolute_expiry_and_positive_remaining_lifetime() {
        let mut store = offered_at(10);
        let initial = store.list(10, BINDING, RUNTIME, SERVER, "unused").unwrap();
        assert_eq!(initial.expires_at_ms, 120_010);
        assert_eq!(initial.expires_in_ms, 120_000);
        let last_ms = store.list(120_009, BINDING, RUNTIME, SERVER, "unused").unwrap();
        assert_eq!(last_ms.context, TOKEN);
        assert_eq!(last_ms.expires_at_ms, initial.expires_at_ms);
        assert_eq!(last_ms.expires_in_ms, 1);
    }

    #[test]
    fn exact_expiry_rejects_admission_and_later_list_replaces_context() {
        let mut store = offered_at(10);
        assert_eq!(
            store.admit(120_010, BINDING, RUNTIME, SERVER, TOKEN, 1, 200_000),
            Err(ContextError::Expired)
        );
        let row = store.list(120_010, BINDING, RUNTIME, SERVER, "context-2").unwrap();
        assert_eq!(row.expires_at_ms, 240_010);
        assert_eq!(row.expires_in_ms, 120_000);
        assert_eq!(
            store.admit(120_011, BINDING, RUNTIME, SERVER, TOKEN, 1, 200_000),
            Err(ContextError::Stale)
        );
        assert!(store.admit(120_011, BINDING, RUNTIME, SERVER, &row.context, 1, 200_000).is_ok());
    }

    #[test]
    fn invalidation_revokes_old_context_without_changing_candidate_generation() {
        let mut store = offered_at(0);
        store.invalidate();
        assert_eq!(
            store.admit(1, BINDING, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        assert_eq!(
            store.list(1, BINDING, RUNTIME, SERVER, TOKEN),
            Err(ContextError::InvalidRequest)
        );
        let row = store.list(2, BINDING, RUNTIME, SERVER, "context-2").unwrap();
        assert_eq!(
            store.admit(3, BINDING, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        assert!(store.admit(3, BINDING, RUNTIME, SERVER, &row.context, 1, 100).is_ok());
    }

    #[test]
    fn mismatched_runtime_source_binding_and_context_do_not_consume_offer() {
        let mut store = offered_at(0);
        for binding in [
            Binding { candidate_generation: 8, ..BINDING },
            Binding { authority_tag: 12, ..BINDING },
            Binding { scope_tag: 14, ..BINDING },
        ] {
            assert_eq!(
                store.admit(1, binding, RUNTIME, SERVER, TOKEN, 1, 100),
                Err(ContextError::SourceChanged)
            );
        }
        assert_eq!(
            store.admit(1, BINDING, "native-qa-2", SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        assert_eq!(
            store.admit(1, BINDING, RUNTIME, "PMS-2", TOKEN, 1, 100),
            Err(ContextError::SourceChanged)
        );
        assert_eq!(
            store.admit(1, BINDING, RUNTIME, SERVER, "other-context", 1, 100),
            Err(ContextError::Stale)
        );
        assert!(store.admit(1, BINDING, RUNTIME, SERVER, TOKEN, 1, 100).is_ok());
    }

    #[test]
    fn caller_supplied_network_binding_change_revokes_old_offer() {
        let mut store = offered_at(0);
        let network_changed = Binding { scope_tag: 14, ..BINDING };
        let row = store.list(1, network_changed, RUNTIME, SERVER, "context-2").unwrap();
        assert_eq!(
            store.admit(2, BINDING, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::SourceChanged)
        );
        assert_eq!(
            store.admit(2, network_changed, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        assert!(store.admit(2, network_changed, RUNTIME, SERVER, &row.context, 1, 100).is_ok());
    }

    #[test]
    fn consumed_candidate_cannot_be_reoffered_after_finish_or_invalidation() {
        let mut store = offered_at(0);
        store.admit(1, BINDING, RUNTIME, SERVER, TOKEN, 1, 100).unwrap();
        assert_eq!(store.offered_deadline(), None);
        assert_eq!(
            store.admit(2, BINDING, RUNTIME, SERVER, TOKEN, 2, 100),
            Err(ContextError::Busy)
        );
        store.invalidate();
        assert!(store.finish(1).is_some());
        assert_eq!(
            store.admit(2, BINDING, RUNTIME, SERVER, TOKEN, 2, 100),
            Err(ContextError::Stale)
        );
        for binding in [BINDING, Binding { scope_tag: 14, ..BINDING }] {
            assert_eq!(
                store.list(2, binding, RUNTIME, SERVER, "context-2"),
                Err(ContextError::Stale)
            );
        }
        let replacement = Binding { candidate_generation: 8, ..BINDING };
        assert!(store.list(2, replacement, RUNTIME, SERVER, "context-2").is_ok());
        assert_eq!(
            store.list(3, BINDING, RUNTIME, SERVER, "context-3"),
            Err(ContextError::SourceChanged)
        );
    }

    #[test]
    fn inflight_survives_expiry_invalidation_and_attempted_candidate_replacement() {
        let mut store = offered_at(0);
        let operation = store.admit(1, BINDING, RUNTIME, SERVER, TOKEN, 9, 300_000).unwrap();
        store.invalidate();
        assert_eq!(
            store.list(
                120_000,
                Binding { candidate_generation: 8, ..BINDING },
                RUNTIME,
                SERVER,
                "context-2",
            ),
            Err(ContextError::Busy)
        );
        assert_eq!(store.candidate_generation, Some(BINDING.candidate_generation));
        assert_eq!(store.inflight(), Some(&operation));
        assert_eq!(store.finish(8), None);
        assert_eq!(store.inflight(), Some(&operation));
        assert_eq!(store.finish(9), Some(operation));
        assert_eq!(store.finish(9), None);
        assert_eq!(store.inflight(), None);
    }

    #[test]
    fn original_deadline_must_be_strictly_after_final_admission() {
        let mut store = offered_at(0);
        for deadline in [9, 10] {
            assert_eq!(
                store.admit(10, BINDING, RUNTIME, SERVER, TOKEN, 1, deadline),
                Err(ContextError::Expired)
            );
        }
        let operation = store.admit(10, BINDING, RUNTIME, SERVER, TOKEN, 1, 11).unwrap();
        assert_eq!(operation.deadline_ms, 11);
        // Finish preserves the original deadline for the caller's outcome decision;
        // it does not turn post-effects timeout into pre-admission expiry.
        assert_eq!(store.finish(1).unwrap().deadline_ms, 11);
    }

    #[test]
    fn invalid_token_and_time_overflow_never_produce_an_offer() {
        let mut store = OfferStore::default();
        for token in ["", "contains space", "non-ascii-\u{e9}"] {
            assert_eq!(
                store.list(0, BINDING, RUNTIME, SERVER, token),
                Err(ContextError::InvalidRequest)
            );
        }
        assert_eq!(
            store.list(0, BINDING, RUNTIME, SERVER, &"a".repeat(257)),
            Err(ContextError::InvalidRequest)
        );
        assert_eq!(
            store.list(u64::MAX, BINDING, RUNTIME, SERVER, TOKEN),
            Err(ContextError::LimitExceeded)
        );
        assert!(store.offered.is_none());
        assert!(store.inflight().is_none());
    }

    #[test]
    fn time_before_offer_creation_is_rejected_without_extending_lifetime() {
        let mut store = offered_at(10);
        assert_eq!(
            store.list(9, BINDING, RUNTIME, SERVER, "unused"),
            Err(ContextError::Stale)
        );
        assert_eq!(
            store.admit(9, BINDING, RUNTIME, SERVER, TOKEN, 1, 100),
            Err(ContextError::Stale)
        );
        let row = store.list(11, BINDING, RUNTIME, SERVER, "unused").unwrap();
        assert_eq!(row.expires_in_ms, 119_999);
    }
}

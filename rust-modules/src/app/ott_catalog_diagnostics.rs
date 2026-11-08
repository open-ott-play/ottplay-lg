//! Public source-catalogue diagnostics, separate from mutable UI copy.
//! Only producer-site codes are retained; no source, URL or error text enters here.
use serde::Serialize;
use std::cell::Cell;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Failure {
    CatalogFailed,
    SourceUnavailable,
    LoaderUnavailable,
    CatalogLimit,
}

impl Failure {
    pub(super) fn from_merge(error: plx_data::ott::CatalogError) -> Self {
        match error {
            plx_data::ott::CatalogError::LimitExceeded => Self::CatalogLimit,
            _ => Self::CatalogFailed,
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    failure: Option<(u64, Failure)>,
    performance: Cell<Option<PerformanceCounts>>,
}

/// Numeric work counts for one source generation, independent of failure state.
/// Accepted entries count Batch rows (including replacements), not unique entries.
/// Merge/rebuild counts include failed calls; publications count queued snapshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(super) struct PerformanceCounts {
    generation: u64,
    accepted_batches: u64,
    accepted_entries: u64,
    merge_count: u64,
    organization_rebuilds: u64,
    view_publications: u64,
}

#[derive(Serialize)]
pub(super) struct Snapshot {
    version: u8,
    scope: &'static str,
    generation: u64,
    loading: bool,
    complete: bool,
    count: usize,
    error_code: Option<Failure>,
}

impl State {
    /// Clear only the failure. Successful Finished must retain measured work.
    pub(super) fn clear(&mut self) {
        self.failure = None;
    }

    pub(super) fn reset_performance(&self, generation: u64) {
        self.performance.set(Some(PerformanceCounts { generation, ..Default::default() }));
    }

    /// Other lifecycle paths can advance generation directly. Reading never
    /// exposes retired counts or resets the current owner's accumulated work.
    pub(super) fn performance_counts(&self, generation: u64) -> PerformanceCounts {
        self.performance.get().filter(|counts| counts.generation == generation)
            .unwrap_or(PerformanceCounts { generation, ..Default::default() })
    }

    fn count(&self, generation: u64, update: impl FnOnce(&mut PerformanceCounts)) {
        let mut counts = self.performance_counts(generation);
        update(&mut counts);
        self.performance.set(Some(counts));
    }

    /// Call after source-owner admission; a retired Batch cannot change counts.
    pub(super) fn accepted_batch(&self, current_generation: u64, event_generation: u64, entries: usize) {
        if event_generation != current_generation { return; }
        self.count(current_generation, |counts| {
            counts.accepted_batches = counts.accepted_batches.saturating_add(1);
            counts.accepted_entries = counts.accepted_entries.saturating_add(entries as u64);
        });
    }

    pub(super) fn merge_attempted(&self, generation: u64) {
        self.count(generation, |counts| counts.merge_count = counts.merge_count.saturating_add(1));
    }

    pub(super) fn organization_rebuilt(&self, generation: u64) {
        self.count(generation, |counts| counts.organization_rebuilds = counts.organization_rebuilds.saturating_add(1));
    }

    pub(super) fn view_published(&self, generation: u64) {
        self.count(generation, |counts| counts.view_publications = counts.view_publications.saturating_add(1));
    }

    /// Call only for the current accepted producer or an owned launch refusal.
    pub(super) fn failed(&mut self, generation: u64, failure: Failure) {
        self.failure = Some((generation, failure));
    }

    pub(super) fn current_failure(&self, generation: u64) -> Option<Failure> {
        // Lifecycle transitions can advance generation directly. Neither a
        // snapshot nor a retained catalogue may reuse a retired failure.
        self.failure.filter(|(owner, _)| *owner == generation)
            .map(|(_, failure)| failure)
    }

    pub(super) fn snapshot(&self, generation: u64, loading: bool, complete: bool,
        count: usize) -> Snapshot {
        Snapshot {
            version: 1,
            // This is the source catalogue, not a filtered or Favorites view.
            scope: "source",
            generation,
            loading,
            complete,
            count,
            error_code: self.current_failure(generation),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_launch_receipt_does_not_poison_restored_generation() {
        let mut state = State::default();
        let launched = 7;
        let restored = 8;
        // Models cleanup retiring the attempted USB task before it reports
        // spawn failure. The receipt must retain the attempted generation.
        state.failed(launched, Failure::LoaderUnavailable);
        let old = serde_json::to_value(state.snapshot(launched, false, false, 0)).unwrap();
        assert_eq!(old["error_code"], "loader_unavailable");
        let current = serde_json::to_value(state.snapshot(restored, true, false, 0)).unwrap();
        assert!(current["error_code"].is_null());
        assert_eq!(current["loading"], true);
    }

    #[test]
    fn performance_counts_admitted_batches_separately_from_coalesced_work() {
        let state = State::default();
        state.accepted_batch(7, 7, 64);
        state.accepted_batch(7, 7, 3);
        state.merge_attempted(7);
        state.organization_rebuilt(7);
        state.view_published(7);
        assert_eq!(state.performance_counts(7), PerformanceCounts {
            generation: 7, accepted_batches: 2, accepted_entries: 67,
            merge_count: 1, organization_rebuilds: 1, view_publications: 1,
        });
    }

    #[test]
    fn retired_batch_and_snapshot_do_not_change_current_performance_counts() {
        let state = State::default();
        state.accepted_batch(7, 7, 64);
        state.accepted_batch(8, 8, 2);
        let current = state.performance_counts(8);
        state.accepted_batch(8, 7, 100);
        assert_eq!(state.performance_counts(7), PerformanceCounts { generation: 7, ..Default::default() });
        assert_eq!(state.performance_counts(8), current);
        assert_eq!(current.accepted_entries, 2);
    }

    #[test]
    fn finished_failure_clear_preserves_counts_and_generation_reset_retires_them() {
        let mut state = State::default();
        state.accepted_batch(7, 7, 64);
        state.merge_attempted(7);
        state.failed(7, Failure::CatalogFailed);
        let before = state.performance_counts(7);
        // The successful Finished path calls clear(), not a work-count reset.
        state.clear();
        assert!(state.current_failure(7).is_none());
        assert_eq!(state.performance_counts(7), before);
        state.reset_performance(8);
        assert_eq!(state.performance_counts(8), PerformanceCounts { generation: 8, ..Default::default() });
        assert_eq!(state.performance_counts(7), PerformanceCounts { generation: 7, ..Default::default() });
        state.view_published(8);
        assert_eq!(state.performance_counts(8).view_publications, 1);
    }

    #[test]
    fn performance_counts_saturate_and_export_only_fixed_numeric_fields() {
        let state = State::default();
        state.performance.set(Some(PerformanceCounts {
            generation: 7, accepted_batches: u64::MAX, accepted_entries: u64::MAX,
            merge_count: u64::MAX, organization_rebuilds: u64::MAX, view_publications: u64::MAX,
        }));
        let before = state.performance_counts(7);
        state.accepted_batch(7, 7, 1);
        state.merge_attempted(7);
        state.organization_rebuilt(7);
        state.view_published(7);
        assert_eq!(state.performance_counts(7), before);
        let value = serde_json::to_value(state.performance_counts(7)).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert!(value.as_object().unwrap().values().all(serde_json::Value::is_u64));
    }
}

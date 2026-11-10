//! Compact current-programme facts for one complete ordinary M3U catalogue.
//! Metadata admission owns publication; row lookup performs no work or catalogue scan.
use super::*;
use plx_data::ott::guide::{rows::{CoverageReport, CoverageState, CurrentProgramme, Outcome, Selector, Snapshot}, ChannelMetadata, Query};
use std::collections::HashMap;

pub(crate) struct Request {
    profile: ProviderProfile,
    generation: u64,
    catalog_revision: u64,
    now_s: i64,
    pub(crate) selectors: Vec<Selector>,
    bindings: HashMap<String, (usize, MediaLocator, Option<ChannelMetadata>)>,
}

pub(crate) struct Reply {
    pub(crate) request: Request,
    pub(crate) outcome: Outcome,
}

pub(super) struct Coverage {
    profile: ProviderProfile,
    generation: u64,
    catalog_revision: u64,
    report: CoverageReport,
}

pub(super) struct Retained {
    request: Request,
    snapshot: Snapshot,
    expiry_notified: bool,
}

pub(super) struct Attempt {
    profile: ProviderProfile,
    generation: u64,
    catalog_revision: u64,
    retry_after_ms: u64,
}

impl Attempt {
    fn current(&self, runtime: &Runtime) -> bool {
        self.generation == runtime.generation && self.catalog_revision == runtime.catalog_revision
            && runtime.effective_profile() == Some(&self.profile)
    }
}

fn live_entry(entry: &CatalogEntry) -> bool {
    entry.duration_ms.is_none() && matches!(entry.media, MediaLocator::Http { .. })
}

fn report_valid(report: &CoverageReport, request: &Request) -> bool {
    report.requested == request.selectors.len()
        && report.requested <= plx_data::ott::MAX_CATALOG_ENTRIES
        && report.matched <= report.requested && report.current <= report.matched
        && report.configured_sources <= plx_data::ott::MAX_CATALOG_ENTRIES
        && report.attempted_sources <= 16 && report.attempted_sources <= report.configured_sources
        && report.loaded_sources <= report.attempted_sources && report.reasons.len() <= 10
        && match report.state {
            CoverageState::Complete => report.requested > 0 && report.matched == report.requested
                && report.reasons.is_empty(),
            CoverageState::Partial => report.matched > 0 && !report.reasons.is_empty(),
            CoverageState::Unavailable => report.matched == 0 && !report.reasons.is_empty(),
        }
}

impl Request {
    fn matches_entry(&self, entry: &CatalogEntry) -> bool {
        let Some((index, media, live)) = self.bindings.get(&entry.id) else { return false; };
        let selector = &self.selectors[*index];
        live_entry(entry) && media == &entry.media && live == &entry.live
            && selector.entry_id == entry.id && selector.provider_id == entry.provider_id
            && selector.title == entry.title
            && selector.tvg_name == entry.live.as_ref().map_or("", |live| live.tvg_name.as_str())
            && selector.shift_seconds == entry.live.as_ref().map_or(0, |live| live.guide_shift_seconds)
    }

    fn current(&self, runtime: &Runtime) -> bool {
        runtime.row_guide_context()
            && self.generation == runtime.generation
            && self.catalog_revision == runtime.catalog_revision
            && runtime.effective_profile() == Some(&self.profile)
    }

    fn selectors_current(&self, runtime: &Runtime) -> bool {
        let mut count = 0;
        for entry in runtime.catalog.entries.iter().filter(|entry| live_entry(entry)) {
            if !self.matches_entry(entry)
                || self.bindings.get(&entry.id).is_none_or(|(index, _, _)| *index != count)
            { return false; }
            count += 1;
        }
        count == self.selectors.len()
    }
}

impl Retained {
    fn expires_at(&self) -> i64 {
        self.snapshot.next_transition_s.unwrap_or_else(|| self.snapshot.now_s.saturating_add(60))
    }

    fn fresh(&self, now_s: i64) -> bool {
        now_s >= self.snapshot.now_s && now_s < self.expires_at()
    }
}

impl Runtime {
    fn row_guide_context(&self) -> bool {
        self.loaded && !self.persistence_uncertain && self.source_work_allowed()
            && self.page == "channels" && self.catalog_complete && !self.catalog_loading
            && !self.cancel.load(Ordering::Acquire)
            && self.transient.accepted.is_none()
            && self.effective_profile().is_some_and(|profile| profile.enabled
                && profile.kind == ProviderKind::M3u
                && plx_data::ott::m3u_source_kind(&profile.source)
                    == Ok(plx_data::ott::M3uSourceKind::Playlist))
    }

    pub(super) fn request_row_guide(
        &self, profile: &ProviderProfile, entry: &CatalogEntry, query: Query,
    ) -> Option<Request> {
        let Query::CurrentNext { now } = query.validate().ok()? else { return None; };
        if !self.row_guide_context() || self.metadata_attachment.is_some()
            || self.effective_profile() != Some(profile) || !live_entry(entry)
            || self.row_guide.as_ref().is_some_and(|cache| cache.request.current(self) && cache.fresh(now))
            || !self.row_guide_retry_ready()
        { return None; }
        let mut request = Request {
            profile: profile.clone(), generation: self.generation,
            catalog_revision: self.catalog_revision, now_s: now,
            selectors: Vec::new(), bindings: HashMap::new(),
        };
        for row in self.catalog.entries.iter().filter(|row| live_entry(row)) {
            let index = request.selectors.len();
            if request.bindings.insert(row.id.clone(), (index, row.media.clone(), row.live.clone())).is_some() {
                return None;
            }
            request.selectors.push(Selector {
                entry_id: row.id.clone(), provider_id: row.provider_id.clone(), title: row.title.clone(),
                tvg_name: row.live.as_ref().map_or_else(String::new, |live| live.tvg_name.clone()),
                shift_seconds: row.live.as_ref().map_or(0, |live| live.guide_shift_seconds),
            });
        }
        request.matches_entry(entry).then_some(request)
    }

    /// Called after the ordinary metadata receiver has accepted every owner fence,
    /// but before its description/poster enrichment changes the catalogue revision.
    pub(super) fn accept_row_guide(&mut self, reply: Option<Reply>) {
        let Some(reply) = reply else { return; };
        if !reply.request.current(self) || !reply.request.selectors_current(self)
            || !report_valid(&reply.outcome.report, &reply.request)
        { return; }
        let Reply { request, outcome: Outcome { snapshot, report } } = reply;
        let current = report.current;
        let available = report.state != CoverageState::Unavailable;
        self.row_guide_coverage = Some(Coverage {
            profile: request.profile.clone(), generation: request.generation,
            catalog_revision: request.catalog_revision, report,
        });
        self.changed();
        let Some(snapshot) = snapshot else { return; };
        if !available || snapshot.programmes.len() != current
            || snapshot.now_s != request.now_s || snapshot.now_s < 0
            || snapshot.next_transition_s.is_some_and(|next| next <= snapshot.now_s)
            || snapshot.programmes.iter().any(|(id, programme)|
                !request.bindings.contains_key(id) || programme.start_s < 0
                    || programme.start_s > snapshot.now_s || programme.end_s <= snapshot.now_s
                    || snapshot.next_transition_s.is_none_or(|next| next > programme.end_s))
        { return; }
        self.row_guide_attempt = None;
        self.row_guide = Some(Retained {
            request, snapshot, expiry_notified: false,
        });
    }

    /// This is only for the receiver's existing description/poster-only merge.
    /// Arbitrary catalogue revisions never inherit the compact facts.
    pub(super) fn rebase_row_guide_enrichment(&mut self, previous_revision: u64) {
        if let Some(coverage) = self.row_guide_coverage.as_mut() {
            if coverage.generation == self.generation && coverage.catalog_revision == previous_revision
                && previous_revision.checked_add(1) == Some(self.catalog_revision)
            { coverage.catalog_revision = self.catalog_revision; }
        }
        if let Some(attempt) = self.row_guide_attempt.as_mut() {
            if attempt.generation == self.generation && attempt.catalog_revision == previous_revision
                && previous_revision.checked_add(1) == Some(self.catalog_revision)
            { attempt.catalog_revision = self.catalog_revision; }
        }
        let Some(cache) = self.row_guide.as_mut() else { return; };
        if cache.request.generation == self.generation
            && cache.request.catalog_revision == previous_revision
            && previous_revision.checked_add(1) == Some(self.catalog_revision)
        {
            cache.request.catalog_revision = self.catalog_revision;
        }
    }

    pub(super) fn current_row_programme(&self, entry: &CatalogEntry) -> Option<CurrentProgramme> {
        let cache = self.row_guide.as_ref()?;
        let now_s = self.wall_now_s()?;
        if !cache.request.current(self) || !cache.fresh(now_s) || !cache.request.matches_entry(entry) {
            return None;
        }
        let programme = cache.snapshot.programmes.get(&entry.id)?;
        (programme.start_s <= now_s && now_s < programme.end_s).then(|| programme.clone())
    }

    /// Only the authenticated state/status response includes this closed report.
    /// Source credentials, row selectors and programme text never enter it.
    pub(super) fn row_guide_diagnostics(&self) -> Value {
        let Some(coverage) = self.row_guide_coverage.as_ref().filter(|coverage|
            self.row_guide_context() && coverage.generation == self.generation
                && coverage.catalog_revision == self.catalog_revision
                && self.effective_profile() == Some(&coverage.profile))
        else { return Value::Null; };
        let fresh = self.wall_now_s().is_some_and(|now| self.row_guide.as_ref()
            .is_some_and(|cache| cache.request.current(self) && cache.fresh(now)));
        // Evaluated only for an authenticated status read, never per frame.
        // Closed counts distinguish accepted HTTP facts from row projection;
        // no channel IDs, programme text or source addresses leave this seam.
        let view = self.view();
        let selected = self.selected.as_ref().and_then(|id| view.rows.iter().find(|row| &row.id == id));
        let now = self.wall_now_s();
        json!({"coverage": coverage.report, "fresh": fresh,
            "projection": {
                "bound_current_rows": self.catalog.entries.iter().filter(|entry| self.current_row_programme(entry).is_some()).count(),
                "rows": view.rows.len(),
                "compact_current_rows": view.rows.iter().filter(|row| row.current_programme.is_some()).count(),
                "full_guide_rows": view.rows.iter().filter(|row| row.guide.is_some()).count(),
                "guide_available_rows": view.rows.iter().filter(|row| row.guide_available).count(),
                "selected_compact": selected.is_some_and(|row| row.current_programme.is_some()),
                "selected_full_current": selected.and_then(|row| row.guide.as_ref()).is_some_and(|guide| guide.at(now.unwrap_or(guide.now_s)).0.is_some()),
                "show_program": view.classic_preferences.show_program(),
                "revision": view.revision
            }})
    }

    fn row_guide_retry_ready(&self) -> bool {
        self.row_guide_attempt.as_ref().is_none_or(|attempt|
            !attempt.current(self) || self.now_ms() >= attempt.retry_after_ms)
    }

    pub(super) fn note_row_guide_attempt(&mut self, request: &Request) {
        self.row_guide_attempt = Some(Attempt {
            profile: request.profile.clone(), generation: request.generation,
            catalog_revision: request.catalog_revision,
            retry_after_ms: self.now_ms().saturating_add(30_000),
        });
    }

    /// Expiry changes the cached view once even when the old worker is slow.
    /// A missing first batch also uses the existing selected request, once ready.
    pub(super) fn tick_row_guide(&mut self) {
        let Some(now_s) = self.wall_now_s() else { return; };
        if self.row_guide.as_ref().is_some_and(|cache| !cache.expiry_notified
            && cache.request.current(self) && !cache.fresh(now_s))
        {
            if let Some(cache) = self.row_guide.as_mut() { cache.expiry_notified = true; }
            self.changed();
        }
        if !self.row_guide_context() || !self.row_guide_retry_ready()
            || !self.metadata_done.load(Ordering::Acquire) || self.metadata_next.is_some()
        { return; }
        if self.row_guide.as_ref().is_some_and(|cache| cache.request.current(self) && cache.fresh(now_s)) {
            self.clamp_row_guide_refresh();
            return;
        }
        let Some((profile_id, entry_id)) = self.metadata_target.as_ref() else { return; };
        let Some(profile) = self.effective_profile().filter(|profile| &profile.id == profile_id) else { return; };
        if entry_id.starts_with("favorite:") { return; }
        if !self.catalog.entries.iter().any(|entry| &entry.id == entry_id && live_entry(entry)) {
            // A retained playback target outside this catalogue cannot start a batch.
            self.row_guide_attempt = Some(Attempt {
                profile: profile.clone(), generation: self.generation, catalog_revision: self.catalog_revision,
                retry_after_ms: self.now_ms().saturating_add(30_000),
            });
            return;
        }
        self.metadata_refresh_ms = self.now_ms().max(1);
    }

    /// Use the existing metadata deadline/worker, including when another row's
    /// programme changes before the selected programme does.
    pub(super) fn clamp_row_guide_refresh(&mut self) {
        let Some(cache) = self.row_guide.as_ref().filter(|cache| cache.request.current(self)) else { return; };
        let Some(now_s) = self.wall_now_s() else { return; };
        if !cache.fresh(now_s) && !self.row_guide_retry_ready() { return; }
        let delay = if now_s < cache.snapshot.now_s { 0 } else {
            u64::try_from(cache.expires_at()).unwrap_or(0).saturating_mul(1000)
                .saturating_sub(self.wall_clock_ms)
        };
        let deadline = self.now_ms().saturating_add(delay).max(1);
        if self.metadata_refresh_ms == 0 || deadline < self.metadata_refresh_ms {
            self.metadata_refresh_ms = deadline;
        }
    }
}

//! Compact XMLTV row facts. Build once off-frame; select without scanning schedules.
//! The owner supplies time, cancellation, source precedence and publication authority.
use super::normalized;
use crate::ott::{epg, CatalogError, MAX_CATALOG_MODEL_BYTES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentProgramme {
    pub title: String,
    pub start_s: i64,
    pub end_s: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub entry_id: String,
    pub provider_id: Option<String>,
    pub tvg_name: String,
    pub title: String,
    pub shift_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub matched: bool,
    pub current: Option<CurrentProgramme>,
    /// A next-only selection still claims this XMLTV source, just like guide::select.
    pub has_next: bool,
    /// Includes an overlapping programme's start before the current programme ends.
    pub next_transition_s: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub now_s: i64,
    pub next_transition_s: Option<i64>,
    pub programmes: HashMap<String, CurrentProgramme>,
}

/// Closed, bounded diagnostics. Never carries source addresses or programme text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageState { Complete, Partial, Unavailable }

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageReason {
    SourceLimit, SourceUnavailable, SourceInvalid, DocumentLimit, IndexLimit,
    OutputLimit, Deadline, NoGuide, Unmatched, InvalidRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub state: CoverageState,
    pub reasons: std::collections::BTreeSet<CoverageReason>,
    pub requested: usize,
    pub matched: usize,
    pub current: usize,
    pub configured_sources: usize,
    pub attempted_sources: usize,
    pub loaded_sources: usize,
}

impl CoverageReport {
    pub fn new(requested: usize) -> Self {
        Self { state: CoverageState::Unavailable, reasons: Default::default(), requested,
            matched: 0, current: 0, configured_sources: 0, attempted_sources: 0, loaded_sources: 0 }
    }
    pub fn finish(&mut self, matched: usize, current: usize) {
        self.matched = matched;
        self.current = current;
        if matched < self.requested { self.reasons.insert(CoverageReason::Unmatched); }
        self.state = if matched == 0 { CoverageState::Unavailable }
            else if self.reasons.is_empty() { CoverageState::Complete }
            else { CoverageState::Partial };
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub snapshot: Option<Snapshot>,
    pub report: CoverageReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    Cancelled,
    Invalid(CatalogError),
}

#[derive(Default)]
struct Schedule {
    rows: Vec<CurrentProgramme>,
    invalid: Option<CatalogError>,
    /// Flat segment tree: each node holds the greatest end in its range.
    max_end: Vec<i64>,
    leaves: usize,
}

#[derive(Default)]
struct RetainedBytes(usize);
impl RetainedBytes {
    fn add(&mut self, bytes: usize) -> Result<(), BuildError> {
        self.0 = self.0.checked_add(bytes).filter(|bytes| *bytes <= MAX_CATALOG_MODEL_BYTES)
            .ok_or(BuildError::Invalid(CatalogError::LimitExceeded))?;
        Ok(())
    }
}

impl Schedule {
    fn finish(&mut self, retained: &mut RetainedBytes) -> Result<(), BuildError> {
        if self.invalid.is_some() {
            self.rows.clear();
            return Ok(());
        }
        // Stable order is material: the existing selector uses the LAST current
        // row among equal (start,end) keys. An adjacent identical-title duplicate
        // cannot change compact current/next facts, so no title dedup is needed.
        self.rows.sort_by_key(|p| (p.start_s, p.end_s));
        if self.rows.is_empty() { return Ok(()); }
        self.leaves = self.rows.len().next_power_of_two();
        retained.add(self.leaves * 2 * std::mem::size_of::<i64>())?;
        self.max_end = vec![i64::MIN; self.leaves * 2];
        for (i, row) in self.rows.iter().enumerate() {
            self.max_end[self.leaves + i] = row.end_s;
        }
        for i in (1..self.leaves).rev() {
            self.max_end[i] = self.max_end[i * 2].max(self.max_end[i * 2 + 1]);
        }
        Ok(())
    }

    /// Rightmost covering interval among rows whose start is <= now. A binary
    /// search on start alone fails when a later short interval has already ended.
    fn current(&self, upper: usize, now: i64) -> Option<usize> {
        fn find(tree: &[i64], node: usize, low: usize, high: usize, upper: usize, now: i64) -> Option<usize> {
            if low >= upper || tree[node] <= now { return None; }
            if high - low == 1 { return Some(low); }
            let mid = low + (high - low) / 2;
            find(tree, node * 2 + 1, mid, high, upper, now)
                .or_else(|| find(tree, node * 2, low, mid, upper, now))
        }
        if upper == 0 || self.rows.is_empty() { return None; }
        find(&self.max_end, 1, 0, self.leaves, upper, now)
    }

    fn select(&self, now_s: i64, shift: i64) -> Result<Selection, CatalogError> {
        if let Some(error) = self.invalid { return Err(error); }
        let mut result = Selection { matched: true, current: None, has_next: false, next_transition_s: None };
        let Some(first) = self.rows.first() else { return Ok(result); };
        // guide::select validates EVERY shifted interval, including inactive
        // programmes. The extrema prove the same condition in constant time.
        if first.start_s.checked_add(shift).is_none_or(|start| start < 0)
            || self.max_end[1].checked_add(shift).is_none()
        { return Err(CatalogError::InvalidFormat); }
        // i128 keeps a valid, very large shift/query from overflowing when the
        // query is moved into the unshifted domain (rows themselves are i64).
        let now = i128::from(now_s) - i128::from(shift);
        let upper = self.rows.partition_point(|p| i128::from(p.start_s) <= now);
        let current = i64::try_from(now).ok().and_then(|now| self.current(upper, now));
        let boundary = current.map_or(now, |i| i128::from(self.rows[i].end_s));
        result.has_next = self.rows.partition_point(|p| i128::from(p.start_s) < boundary) < self.rows.len();
        result.current = current.map(|i| {
            let p = &self.rows[i];
            CurrentProgramme { title: p.title.clone(), start_s: p.start_s + shift, end_s: p.end_s + shift }
        });
        let upcoming_start = self.rows.get(upper).map(|p| p.start_s + shift);
        result.next_transition_s = match (result.current.as_ref().map(|p| p.end_s), upcoming_start) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(result)
    }
}

/// No descriptions or raw XML are retained. Channel names point to numeric slots,
/// avoiding a copy of a potentially long channel ID for each display-name alias.
pub struct Index {
    ids: BTreeMap<String, usize>,
    names: BTreeMap<String, Option<usize>>,
    schedules: Vec<Schedule>,
}

impl Index {
    pub fn parse_with_cancel(xml: &str, cancelled: &mut dyn FnMut() -> bool) -> Result<Self, BuildError> {
        let document = epg::compact(xml, cancelled).map_err(BuildError::Invalid)?
            .ok_or(BuildError::Cancelled)?;
        let mut index = Self { ids: BTreeMap::new(), names: BTreeMap::new(), schedules: Vec::new() };
        // Account retained string capacities and collection elements, including
        // conservative room for vector growth and map links. Input parsing has
        // its independent XML byte, programme, tag and per-metadata bounds.
        let mut retained = RetainedBytes::default();
        for channel in document.channels {
            if cancelled() { return Err(BuildError::Cancelled); }
            let slot = if let Some(slot) = index.ids.get(&channel.id) { *slot } else {
                retained.add(channel.id.capacity()
                    + 2 * std::mem::size_of::<(String, usize)>()
                    + 2 * std::mem::size_of::<Schedule>())?;
                let slot = index.schedules.len();
                index.schedules.push(Schedule::default());
                index.ids.insert(channel.id, slot);
                slot
            };
            for name in channel.names {
                if cancelled() { return Err(BuildError::Cancelled); }
                let name = normalized(&name);
                if name.is_empty() { continue; }
                if !index.names.contains_key(&name) {
                    retained.add(name.capacity() + 2 * std::mem::size_of::<(String, Option<usize>)>())?;
                }
                index.names.entry(name).and_modify(|old| {
                    if *old != Some(slot) { *old = None; }
                }).or_insert(Some(slot));
            }
        }
        for (id, error) in document.invalid_channels {
            if cancelled() { return Err(BuildError::Cancelled); }
            if let Some(slot) = index.ids.get(&id) { index.schedules[*slot].invalid = Some(error); }
        }
        for row in document.programmes {
            if cancelled() { return Err(BuildError::Cancelled); }
            if let Some(slot) = index.ids.get(&row.channel) {
                let schedule = &mut index.schedules[*slot];
                if schedule.invalid.is_none() {
                    retained.add(row.title.capacity() + 2 * std::mem::size_of::<CurrentProgramme>())?;
                    schedule.rows.push(CurrentProgramme { title: row.title, start_s: row.start, end_s: row.end });
                }
            }
        }
        for schedule in &mut index.schedules {
            if cancelled() { return Err(BuildError::Cancelled); }
            schedule.finish(&mut retained)?;
        }
        if cancelled() { return Err(BuildError::Cancelled); }
        Ok(index)
    }

    pub fn select(&self, selector: &Selector, now_s: i64) -> Result<Selection, CatalogError> {
        if now_s < 0 { return Err(CatalogError::InvalidFormat); }
        let exact = selector.provider_id.as_deref().filter(|id| !id.is_empty())
            .and_then(|id| self.ids.get(id).copied());
        let slot = match exact {
            Some(slot) => Some(slot),
            None => {
                let mut found = None;
                for name in [&selector.tvg_name, &selector.title] {
                    let name = normalized(name);
                    if let Some(slot) = self.names.get(&name) {
                        // Ambiguous tvg-name refuses the whole match; it does
                        // not silently fall through to the display title.
                        found = *slot;
                        break;
                    }
                }
                found
            }
        };
        match slot {
            Some(slot) => self.schedules[slot].select(now_s, selector.shift_seconds),
            None => Ok(Selection { matched: false, current: None, has_next: false, next_transition_s: None }),
        }
    }
}

#[cfg(test)]
#[path = "guide_rows_tests.rs"]
mod tests;

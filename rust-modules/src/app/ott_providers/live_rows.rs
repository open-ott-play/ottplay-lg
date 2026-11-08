//! Bounded sequential XMLTV batches; diagnostics never carry source or programme text.
use super::*;
use ott::guide::rows::{BuildError, CoverageReason as Reason, CoverageReport, Index, Outcome, Selector, Snapshot};
use std::collections::{HashMap, HashSet};

pub(in crate::app::ott_provider_http) fn m3u_guide_rows(
    profile: &ProviderProfile, entry: &CatalogEntry, request: &GuideRequest,
    selectors: &[Selector], http: &mut dyn ProviderHttp,
) -> (Result<ResultSet, &'static str>, Outcome) {
    let mut report = CoverageReport::new(selectors.len());
    match run(profile, entry, request, selectors, http, &mut report) {
        Ok((selected, snapshot)) => (selected, Outcome { snapshot, report }),
        Err(error) => {
            if report.reasons.is_empty() { report.reasons.insert(Reason::InvalidRequest); }
            report.finish(0, 0);
            (Err(error), Outcome { snapshot: None, report })
        }
    }
}

fn run(
    profile: &ProviderProfile, entry: &CatalogEntry, request: &GuideRequest,
    selectors: &[Selector], http: &mut dyn ProviderHttp, report: &mut CoverageReport,
) -> Result<(Result<ResultSet, &'static str>, Option<Snapshot>), &'static str> {
    profile.validate().map_err(|_| "Invalid M3U profile")?;
    if profile.kind != ott::ProviderKind::M3u
        || ott::m3u_source_kind(&profile.source) != Ok(ott::M3uSourceKind::Playlist)
        || !matches!(entry.media, ott::MediaLocator::Http { .. }) || entry.duration_ms.is_some()
        || request.context.profile_id != profile.id || request.context.entry_id != entry.id
    { return Err("Guide context mismatch"); }
    let ott::guide::Query::CurrentNext { now } = request.query.validate()
        .map_err(|_| "Invalid guide interval")? else { return Err("Row guide needs current time"); };
    let mut sources = m3u_guide_sources(profile, http).map_err(|error| {
        report.reasons.insert(Reason::SourceUnavailable); error
    })?;
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(source.clone()));
    report.configured_sources = sources.len();
    if sources.is_empty() { report.reasons.insert(Reason::NoGuide); }
    let unique = selectors.iter().map(|s| &s.entry_id).collect::<HashSet<_>>();
    let mut batch = !selectors.is_empty() && selectors.len() <= ott::MAX_CATALOG_ENTRIES
        && unique.len() == selectors.len();
    if !batch { report.reasons.insert(Reason::InvalidRequest); }
    let mut snapshot = Snapshot { now_s: now, next_transition_s: None, programmes: HashMap::new() };
    let mut claimed = HashSet::new();
    let mut selected = None;
    let mut valid_source = sources.is_empty();
    let mut model_bytes = 0usize;
    for (source_index, source) in sources.into_iter().enumerate() {
        if http.cancelled() { return Err("Provider request cancelled"); }
        if selected.is_some() && (!batch || claimed.len() == selectors.len()) { break; }
        if batch && source_index >= MAX_GUIDE_SOURCES {
            report.reasons.insert(Reason::SourceLimit); batch = false;
        }
        if batch && http.row_guide_expired() {
            report.reasons.insert(Reason::Deadline); batch = false;
        }
        if !batch && selected.is_some() { break; }
        if batch { report.attempted_sources += 1; }
        let response = http.request(&Request { url: source, headers: Vec::new(), body: None });
        if http.cancelled() { return Err("Provider request cancelled"); }
        if batch && http.row_guide_expired() {
            report.reasons.insert(Reason::Deadline); batch = false;
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if batch { report.reasons.insert(if error == "Provider response exceeds limit" {
                    Reason::DocumentLimit
                } else { Reason::SourceUnavailable }); }
                continue;
            }
        };
        if response.body.len() > ott::MAX_CATALOG_BYTES {
            if batch { report.reasons.insert(Reason::DocumentLimit); }
            continue;
        }
        if !(200..300).contains(&response.status) {
            if batch { report.reasons.insert(Reason::SourceUnavailable); }
            continue;
        }
        let xml = match std::str::from_utf8(&response.body) {
            Ok(xml) => xml,
            Err(_) => { if batch { report.reasons.insert(Reason::SourceInvalid); } continue; }
        };
        // The selected full guide retains the original parser and first useful
        // source. Row budgets never terminate an unsatisfied selected lookup.
        if selected.is_none() {
            if let Ok(channels) = ott::epg::discover_channels(xml) {
                let name = entry.live.as_ref().map_or("", |live| live.tvg_name.as_str());
                if let Some(channel) = ott::guide::channel_id(&channels, entry.provider_id.as_deref(), name, &entry.title) {
                    let shift = entry.live.as_ref().map_or(0, |live| live.guide_shift_seconds);
                    if let Ok(rows) = ott::epg::parse_xmltv(xml, &channel) {
                        if let Ok(programmes) = ott::guide::select(&rows, &channel, request.query, shift) {
                            valid_source = true;
                            if !programmes.is_empty() { selected = Some(programmes); http.row_guide_selected(); }
                        }
                    }
                } else { valid_source = true; }
            }
        }
        if !batch { continue; }
        let index = match Index::parse_with_cancel(xml, &mut || http.cancelled() || http.row_guide_expired()) {
            Ok(index) => index,
            Err(BuildError::Cancelled) if http.cancelled() => return Err("Provider request cancelled"),
            Err(BuildError::Cancelled) => { report.reasons.insert(Reason::Deadline); batch = false; continue; }
            Err(BuildError::Invalid(error)) => {
                report.reasons.insert(if error == ott::CatalogError::LimitExceeded { Reason::IndexLimit } else { Reason::SourceInvalid });
                continue;
            }
        };
        report.loaded_sources += 1;
        for selector in selectors {
            if http.cancelled() { return Err("Provider request cancelled"); }
            if http.row_guide_expired() { report.reasons.insert(Reason::Deadline); batch = false; break; }
            if claimed.contains(&selector.entry_id) { continue; }
            let selection = match index.select(selector, now) {
                Ok(selection) => selection,
                Err(_) => { report.reasons.insert(Reason::SourceInvalid); continue; }
            };
            if selection.current.is_none() && !selection.has_next { continue; }
            if let Some(programme) = selection.current {
                let next_bytes = model_bytes.saturating_add(selector.entry_id.len())
                    .saturating_add(programme.title.len())
                    .saturating_add(std::mem::size_of::<(String, ott::guide::rows::CurrentProgramme)>());
                if next_bytes > ott::MAX_CATALOG_MODEL_BYTES { report.reasons.insert(Reason::OutputLimit); batch = false; break; }
                model_bytes = next_bytes;
                snapshot.programmes.insert(selector.entry_id.clone(), programme);
            }
            // Commit claims only after the output passed admission. Stopping
            // returns this prefix; later sources never override it.
            claimed.insert(selector.entry_id.clone());
            if let Some(transition) = selection.next_transition_s {
                snapshot.next_transition_s = Some(snapshot.next_transition_s.map_or(transition, |old| old.min(transition)));
            }
        }
    }
    if http.cancelled() { return Err("Provider request cancelled"); }
    let selected = if selected.is_some() || valid_source {
        Ok(ResultSet { context: request.context.clone(), query: request.query,
            coverage: ott::guide::Coverage::RequestedWindow, programmes: selected.unwrap_or_default() })
    } else { Err("No XMLTV source could be loaded") };
    report.finish(claimed.len(), snapshot.programmes.len());
    Ok((selected, (!claimed.is_empty()).then_some(snapshot)))
}

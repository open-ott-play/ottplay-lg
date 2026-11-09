//! Live guide I/O through the existing worker port. No cache, queue or playback owner.
use super::{ProviderHttp, Request};
use plx_data::ott::{
    self,
    guide::{Request as GuideRequest, ResultSet},
    CatalogEntry, ProviderProfile,
};

// Bound atomic Records and catalogue-row batches; never truncate their source set.
const MAX_GUIDE_SOURCES: usize = 16;

pub(super) fn m3u_guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &GuideRequest,
    http: &mut dyn ProviderHttp,
) -> Result<ResultSet, &'static str> {
    profile.validate().map_err(|_| "Invalid M3U profile")?;
    if profile.kind != ott::ProviderKind::M3u {
        return Err("Invalid M3U profile");
    }
    request
        .query
        .validate()
        .map_err(|_| "Invalid guide interval")?;
    if request.context.profile_id != profile.id || request.context.entry_id != entry.id {
        return Err("Guide context mismatch");
    }
    let records = if let ott::guide::Query::Records { start, end } = request.query {
        // The UI captures the effective depth, including the saved fallback.
        // Recheck that it still fits this source before the first request.
        let expected = ott::records::query(profile, entry, ((end - start) / 3600) as u64, end)
            .map_err(|_| "Records require an explicit M3U catch-up source")?;
        if expected != request.query { return Err("Records source changed; reopen Records"); }
        true
    } else { false };
    let mut sources = m3u_guide_sources(profile, http)?;
    if records {
        let mut seen = std::collections::BTreeSet::new();
        sources.retain(|source| seen.insert(source.clone()));
        if sources.is_empty() { return Err("No XMLTV guide is configured for Records"); }
        if sources.len() > MAX_GUIDE_SOURCES { return Err("Records guide source count exceeds limit"); }
    }
    let mut records_bytes = 0usize;
    let mut records_programmes = Vec::new();
    let mut records_matched = false;
    // A playlist without an advertised guide is still a valid playable catalogue.
    let mut valid_source = sources.is_empty();
    for source in sources {
        if http.cancelled() {
            return Err("Provider request cancelled");
        }
        let response = http.request(&Request {
            url: source,
            headers: Vec::new(),
            body: None,
        });
        if http.cancelled() {
            return Err("Provider request cancelled");
        }
        let response = match response {
            Ok(response) => response,
            Err(_) if records => return Err("A Records XMLTV source could not be loaded"),
            Err(_) => continue,
        };
        if !(200..300).contains(&response.status) || response.body.len() > ott::MAX_CATALOG_BYTES {
            if records { return Err("A Records XMLTV source failed or exceeds its limit"); }
            continue;
        }
        if records {
            records_bytes = records_bytes.checked_add(response.body.len())
                .filter(|bytes| *bytes <= ott::MAX_CATALOG_BYTES)
                .ok_or("Records XMLTV sources exceed the aggregate byte limit")?;
        }
        let Ok(xml) = std::str::from_utf8(&response.body) else {
            if records { return Err("A Records XMLTV source contains invalid text"); }
            continue;
        };
        let Ok(channels) = ott::epg::discover_channels(xml) else {
            if records { return Err("A Records XMLTV source is invalid or incomplete"); }
            continue;
        };
        let name = entry
            .live
            .as_ref()
            .map_or("", |live| live.tvg_name.as_str());
        let Some(channel) =
            ott::guide::channel_id(&channels, entry.provider_id.as_deref(), name, &entry.title)
        else {
            valid_source = true;
            continue;
        };
        let shift = entry
            .live
            .as_ref()
            .map_or(0, |live| live.guide_shift_seconds);
        let Ok(rows) = ott::epg::parse_xmltv(xml, &channel) else {
            if records { return Err("A Records XMLTV source is invalid or incomplete"); }
            continue;
        };
        let Ok(programmes) = ott::guide::select(&rows, &channel, request.query, shift) else {
            if records { return Err("Records programme times are invalid"); }
            continue;
        };
        if records {
            records_matched = true;
            if records_programmes.len().saturating_add(programmes.len()) > ott::MAX_CATALOG_ENTRIES {
                return Err("Records programme count exceeds limit");
            }
            records_programmes.extend(programmes);
            continue;
        }
        valid_source = true;
        // A valid but irrelevant/empty first source must not hide a useful second source.
        if !programmes.is_empty() {
            return Ok(ResultSet {
                context: request.context.clone(),
                query: request.query,
                coverage: ott::guide::Coverage::RequestedWindow,
                programmes,
            });
        }
    }
    if records {
        if !records_matched { return Err("No XMLTV guide matches this Records channel"); }
        return Ok(ResultSet { context: request.context.clone(), query: request.query,
            coverage: ott::guide::Coverage::RequestedWindow, programmes: records_programmes });
    }
    if valid_source {
        Ok(ResultSet {
            context: request.context.clone(),
            query: request.query,
            coverage: ott::guide::Coverage::RequestedWindow,
            programmes: Vec::new(),
        })
    } else {
        Err("No XMLTV source could be loaded")
    }
}

pub(super) fn m3u_guide_sources(profile: &ProviderProfile, http: &mut dyn ProviderHttp) -> Result<Vec<String>, &'static str> {
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let mut head: Vec<u8> = Vec::new();
    let mut complete = false;
    let mut oversized = false;
    let result = http.stream(
        &Request {
            url: profile.source.clone(),
            headers: Vec::new(),
            body: None,
        },
        &mut |chunk| {
            for &byte in chunk {
                if byte == b'\n' {
                    if head.iter().any(|b| !b.is_ascii_whitespace()) {
                        complete = true;
                        return false;
                    }
                    head.clear();
                } else {
                    head.push(byte);
                    if head.len() > ott::MAX_CATALOG_LINE_BYTES {
                        oversized = true;
                        return false;
                    }
                }
            }
            true
        },
    );
    if oversized {
        return Err("M3U header exceeds limit");
    }
    if !complete {
        result?;
    }
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let header = std::str::from_utf8(&head)
        .map_err(|_| "Invalid M3U header")?
        .trim()
        .trim_start_matches('\u{feff}')
        .trim();
    if (!complete && head.is_empty()) || !header.starts_with("#EXTM3U") {
        return Err("Invalid M3U header");
    }
    Ok(ott::epg_urls(header, &profile.source))
}

#[path = "live_rows.rs"]
mod rows;
pub(super) use rows::m3u_guide_rows;

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;

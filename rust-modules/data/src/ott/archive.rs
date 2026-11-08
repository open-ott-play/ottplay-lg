//! Pure catch-up planning. The existing playback owner executes the returned target.
use super::{http_url, resolve_http_url, CatalogError, MAX_CATALOG_LINE_BYTES};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatchupMode {
    Template,
    Append,
    Shift,
    Flussonic,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchupSource {
    pub mode: CatchupMode,
    pub source: String,
    pub retention_seconds: u64,
    #[serde(default)]
    pub correction_seconds: i64,
}
impl std::fmt::Debug for CatchupSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatchupSource")
            .field("mode", &self.mode)
            .field("source", &"<private>")
            .field("retention_seconds", &self.retention_seconds)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveInterval {
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ArchiveTarget {
    pub url: String,
    pub interval: ArchiveInterval,
}
impl std::fmt::Debug for ArchiveTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveTarget")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// End is exclusive. Future, expired, overflowing and empty intervals are refused.
pub fn plan(
    source: &CatchupSource,
    live_url: &str,
    interval: ArchiveInterval,
    now: i64,
) -> Result<ArchiveTarget, CatalogError> {
    let retention =
        i64::try_from(source.retention_seconds).map_err(|_| CatalogError::InvalidFormat)?;
    let oldest = now
        .checked_sub(retention)
        .ok_or(CatalogError::InvalidFormat)?;
    if now < 0
        || retention <= 0
        || interval.start < 0
        || interval.start < oldest
        || interval.end > now
        || interval.end <= interval.start
        || source.source.len() > MAX_CATALOG_LINE_BYTES
        || source.source.chars().any(char::is_control)
        || http_url(live_url).is_none()
    {
        return Err(CatalogError::InvalidFormat);
    }
    let start = interval
        .start
        .checked_add(source.correction_seconds)
        .ok_or(CatalogError::InvalidFormat)?;
    let end = interval
        .end
        .checked_add(source.correction_seconds)
        .ok_or(CatalogError::InvalidFormat)?;
    if start < 0 || end < start {
        return Err(CatalogError::InvalidFormat);
    }
    let duration = end - start;
    let raw = match source.mode {
        CatchupMode::Template => expand(&source.source, start, end, now, duration)?,
        CatchupMode::Append => {
            if source.source.is_empty() {
                return Err(CatalogError::InvalidFormat);
            }
            let suffix = expand(&source.source, start, end, now, duration)?;
            if !suffix.starts_with(['?', '&']) {
                return Err(CatalogError::InvalidFormat);
            }
            format!(
                "{}{}{}",
                live_url,
                if live_url.contains('?') { "&" } else { "?" },
                suffix.trim_start_matches(['?', '&'])
            )
        }
        CatchupMode::Shift => format!(
            "{}{}utc={start}&lutc={now}",
            live_url,
            if live_url.contains('?') { "&" } else { "?" }
        ),
        CatchupMode::Flussonic => {
            let (base, query) = live_url.split_once('?').unwrap_or((live_url, ""));
            let (parent, name) = base.rsplit_once('/').ok_or(CatalogError::InvalidUrl)?;
            // The rolling recent window has a distinct endpoint; exactly 600 seconds is archived.
            let recent = now - interval.start < 600;
            let file = if name == "mpegts" {
                if recent {
                    format!("timeshift_abs-{start}.ts")
                } else {
                    format!("archive-{start}-{duration}.ts")
                }
            } else if name == "index.m3u8" {
                if recent {
                    format!("timeshift_abs-{start}.m3u8")
                } else {
                    format!("archive-{start}-{duration}.m3u8")
                }
            } else if let Some(stem) = name.strip_suffix(".m3u8") {
                if recent {
                    format!("{stem}-timeshift_abs-{start}.m3u8")
                } else {
                    format!("{stem}-{start}-{duration}.m3u8")
                }
            } else {
                return Err(CatalogError::InvalidFormat);
            };
            format!(
                "{parent}/{file}{}{}",
                if query.is_empty() { "" } else { "?" },
                query
            )
        }
    };
    let url = resolve_http_url(live_url, &raw).ok_or(CatalogError::InvalidUrl)?;
    Ok(ArchiveTarget { url, interval })
}

fn expand(
    template: &str,
    start: i64,
    end: i64,
    now: i64,
    duration: i64,
) -> Result<String, CatalogError> {
    if template.is_empty() {
        return Err(CatalogError::InvalidFormat);
    }
    let offset = now.checked_sub(start).ok_or(CatalogError::InvalidFormat)?;
    let mut result = template.to_string();
    for (key, value) in [
        ("start", start),
        ("end", end),
        ("timestamp", now),
        ("offset", offset),
        ("duration", duration),
        ("utc", start),
        ("utcend", end),
        ("lutc", now),
    ] {
        // Replace the longer syntax first so `${start}` cannot become `$123`.
        result = result
            .replace(&format!("${{{key}}}"), &value.to_string())
            .replace(&format!("{{{key}}}"), &value.to_string());
    }
    if result.contains(['{', '}']) {
        return Err(CatalogError::InvalidFormat);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;

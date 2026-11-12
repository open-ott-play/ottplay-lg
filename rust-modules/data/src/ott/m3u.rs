use super::{
    identity, resolve_http_url, text, Catalog, CatalogEntry, CatalogError, MediaLocator,
    ProviderKind, ProviderProfile, MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES, MAX_CATALOG_LINE_BYTES,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Information {
    title: String,
    group: String,
    logo: String,
    id: Option<String>,
    duration_ms: Option<u64>,
    live: super::guide::ChannelMetadata,
}

/// Streaming line parser: a runner can publish the first returned row before the scan completes.
pub struct M3uParser {
    profile: ProviderProfile,
    pending: Information,
    // Retain partial header fields until the entry supplies its own overrides.
    guide_defaults: BTreeMap<String, String>,
    bytes: usize,
    entries: usize,
}

impl M3uParser {
    pub fn new(profile: &ProviderProfile) -> Result<Self, CatalogError> {
        profile.validate()?;
        if profile.kind != ProviderKind::M3u {
            return Err(CatalogError::InvalidProfile);
        }
        Ok(Self {
            profile: profile.clone(),
            pending: Information::default(),
            guide_defaults: BTreeMap::new(),
            bytes: 0,
            entries: 0,
        })
    }

    pub fn push_line(&mut self, line: &str) -> Result<Option<CatalogEntry>, CatalogError> {
        self.bytes = self.bytes.saturating_add(line.len() + 1);
        if line.len() > MAX_CATALOG_LINE_BYTES || self.bytes > MAX_CATALOG_BYTES {
            return Err(CatalogError::LimitExceeded);
        }
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            return Ok(None);
        }
        if line.starts_with("#EXT-X-") {
            return Err(CatalogError::InvalidFormat);
        }
        if let Some(header) = line.strip_prefix("#EXTM3U") {
            self.guide_defaults = attributes(header);
            return Ok(None);
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            self.pending = information(info, &self.guide_defaults);
            return Ok(None);
        }
        if let Some(group) = line.strip_prefix("#EXTGRP:") {
            self.pending.group = text(group, 1024);
            return Ok(None);
        }
        if line.starts_with('#') {
            return Ok(None);
        }
        let pending = std::mem::take(&mut self.pending);
        let Some(url) = resolve_http_url(&self.profile.source, line) else {
            return Ok(None);
        };
        if self.entries >= MAX_CATALOG_ENTRIES {
            return Err(CatalogError::LimitExceeded);
        }
        self.entries += 1;
        let untitled = pending.title.is_empty();
        let title = if untitled {
            format!("Video {}", self.entries)
        } else {
            pending.title
        };
        // Ignore known expiring query credentials while preserving distinct source paths.
        let key = format!(
            "{}\0{}\0{}\0{}",
            pending.id.as_deref().unwrap_or(""),
            if untitled { "" } else { &title },
            pending.group,
            stable_source(&url)
        );
        Ok(Some(CatalogEntry {
            id: identity(&self.profile.id, &key),
            title,
            category: pending.group,
            description: String::new(),
            synopsis: None,
            poster: resolve_http_url(&self.profile.source, &pending.logo).unwrap_or_default(),
            media: MediaLocator::Http { url },
            duration_ms: pending.duration_ms,
            provider_id_field: None,
            provider_id: pending.id,
            live: pending.duration_ms.is_none().then_some(pending.live),
        }))
    }
}

pub fn parse_m3u(input: &str, profile: &ProviderProfile) -> Result<Catalog, CatalogError> {
    if input.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::LimitExceeded);
    }
    let mut parser = M3uParser::new(profile)?;
    let mut catalogue = Catalog::default();
    for line in input.lines() {
        if let Some(entry) = parser.push_line(line)? {
            catalogue.entries.push(entry);
        }
    }
    // Deduplication applies to repeated source rows, not repeated display names or tvg-ids.
    let mut result = Catalog::default();
    result.merge(catalogue)?;
    Ok(result)
}

fn information(input: &str, defaults: &BTreeMap<String, String>) -> Information {
    let mut quote = None;
    let mut separator = None;
    for (index, ch) in input.char_indices() {
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
        } else if ch == ',' && quote.is_none() {
            separator = Some(index);
            break;
        }
    }
    let (head, title) = separator.map_or((input, ""), |i| (&input[..i], &input[i + 1..]));
    let seconds = head
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok());
    let duration_ms = seconds
        .filter(|s| s.is_finite() && *s > 0.0 && *s <= 86400.0 * 365.0)
        .map(|s| (s * 1000.0) as u64);
    let fields = attributes(head);
    let field = |name: &str| fields.get(name).map(String::as_str).unwrap_or("");
    let title = if title.trim().is_empty() {
        field("tvg-name")
    } else {
        title
    };
    Information {
        title: text(title, 1024),
        group: text(field("group-title"), 1024),
        logo: field("tvg-logo").into(),
        id: (!field("tvg-id").is_empty()).then(|| text(field("tvg-id"), 256)),
        duration_ms,
        live: guide_metadata(&fields, defaults),
    }
}

fn guide_metadata(
    fields: &BTreeMap<String, String>,
    defaults: &BTreeMap<String, String>,
) -> super::guide::ChannelMetadata {
    // Explicit empty/invalid entry values also override defaults, including archive opt-out.
    let field = |key: &str| {
        fields
            .get(key)
            .or_else(|| defaults.get(key))
            .map(String::as_str)
    };
    let shift = field("tvg-shift")
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|hours| hours.is_finite() && hours.abs() <= 48.0)
                .map_or(0, |hours| (hours * 3600.0) as i64)
        })
        .unwrap_or(0);
    let catchup = (|| {
        use super::archive::{CatchupMode, CatchupSource};
        let mode = match field("catchup")? {
            "default" => CatchupMode::Template,
            "append" => CatchupMode::Append,
            "shift" => CatchupMode::Shift,
            "flussonic" => CatchupMode::Flussonic,
            _ => return None,
        };
        // Zero is a missing-depth marker only. Explicit invalid/zero values
        // remain opt-outs and must never be revived by a profile fallback.
        let retention = match field("catchup-days") {
            Some(value) => {
                let seconds = seconds(value, 86400.0)?;
                if seconds <= 0 {
                    return None;
                }
                seconds
            }
            None => 0,
        };
        let source = field("catchup-source").unwrap_or("").to_owned();
        if matches!(mode, CatchupMode::Template | CatchupMode::Append) && source.is_empty() {
            return None;
        }
        let correction_seconds = match field("catchup-correction") {
            Some(value) => seconds(value, 3600.0)?,
            None => 0,
        };
        Some(CatchupSource {
            mode,
            source,
            retention_seconds: retention as u64,
            correction_seconds,
        })
    })();
    super::guide::ChannelMetadata {
        guide_id: None, guide_feed: String::new(),
        tvg_name: text(field("tvg-name").unwrap_or(""), 1024),
        guide_shift_seconds: shift,
        catchup,
    }
}

fn seconds(value: &str, scale: f64) -> Option<i64> {
    let value = value.parse::<f64>().ok()? * scale;
    (value.is_finite() && value.abs() <= 365.0 * 86400.0).then(|| value.round() as i64)
}

fn attributes(input: &str) -> BTreeMap<String, String> {
    let mut rest = input;
    let mut fields = BTreeMap::new();
    while !rest.is_empty() {
        rest = rest.trim_start();
        let Some(end) = rest.find(|c: char| c == '=' || c.is_whitespace()) else {
            break;
        };
        let key = &rest[..end];
        rest = &rest[end..];
        if !rest.starts_with('=') {
            rest = rest.trim_start();
            continue;
        }
        rest = rest[1..].trim_start();
        let value;
        if rest.starts_with(['\'', '"']) {
            let q = rest.as_bytes()[0] as char;
            rest = &rest[1..];
            let Some(end) = rest.find(q) else {
                break;
            };
            value = &rest[..end];
            rest = &rest[end + 1..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            value = &rest[..end];
            rest = &rest[end..];
        }
        fields.insert(key.to_ascii_lowercase(), value.to_string());
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unnamed_sources_keep_identity_after_reordering() {
        let a = parse_m3u(
            "#EXTM3U\nhttps://tv.invalid/a.mp4\nhttps://tv.invalid/b.mp4",
            &profile(),
        )
        .unwrap();
        let b = parse_m3u(
            "#EXTM3U\nhttps://tv.invalid/b.mp4\nhttps://tv.invalid/a.mp4",
            &profile(),
        )
        .unwrap();
        assert_eq!(a.entries[0].id, b.entries[1].id);
        assert_eq!(a.entries[1].id, b.entries[0].id);
    }
    #[test]
    fn timestamped_channel_export_refresh_preserves_saved_id_lookup_and_fresh_media() {
        let source = "#EXTM3U\n#EXTINF:-1 tvg-id=\"news\" group-title=\"News\",Daily news\nhttps://proxy.invalid/channel/opaque_7/index.m3u8?q=1790000000000&id=source&pid=7\n";
        let first = parse_m3u(source, &profile()).unwrap();
        let saved_id = first.entries[0].id.clone();
        let refreshed = parse_m3u(&source.replace("1790000000000", "1790001100164"), &profile()).unwrap();
        let restored = refreshed.entries.iter().find(|entry| entry.id == saved_id)
            .expect("a saved channel ID must resolve after export timestamp refresh");
        assert_eq!(restored.title, "Daily news");
        assert_ne!(restored.media, first.entries[0].media, "playback keeps the fresh timestamped URL");
        for (from, to) in [("proxy.invalid", "other.invalid"), ("opaque_7", "opaque_8"),
            ("id=source", "id=other"), ("pid=7", "pid=8"), ("Daily news", "Other news"),
            ("tvg-id=\"news\"", "tvg-id=\"other\""), ("group-title=\"News\"", "group-title=\"Other\"")] {
            let different = parse_m3u(&source.replace(from, to), &profile()).unwrap();
            assert_ne!(different.entries[0].id, saved_id, "{from} remains part of channel identity");
        }
    }

    #[test]
    fn ordinary_q_selectors_and_non_timestamp_proxy_q_remain_distinct() {
        for (path, a, b) in [("/watch", "1790000000000", "1790001100164"),
            ("/channel/opaque/index.m3u8", "7", "8"),
            ("/channel/opaque/index.m3u8", "channel-a", "channel-b"),
            ("/channel/opaque/extra/index.m3u8", "1790000000000", "1790001100164")] {
            let playlist = format!("#EXTM3U\n#EXTINF:-1,Same name\nhttps://proxy.invalid{path}?q={a}\n#EXTINF:-1,Same name\nhttps://proxy.invalid{path}?q={b}\n");
            let catalog = parse_m3u(&playlist, &profile()).unwrap();
            assert_eq!(catalog.entries.len(), 2, "meaningful source selectors cannot deduplicate");
            assert_ne!(catalog.entries[0].id, catalog.entries[1].id);
        }
    }

    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "m3u-home".into(),
            name: "Home".into(),
            kind: ProviderKind::M3u,
            source: "https://tv.invalid/lists/live.m3u".into(),
            source_alias: "".into(),
            enabled: true,
        }
    }
    #[test]
    fn first_row_is_available_without_finish() {
        let mut parser = M3uParser::new(&profile()).unwrap();
        assert!(parser.push_line("#EXTM3U").unwrap().is_none());
        parser
            .push_line("#EXTINF:-1 tvg-name=\"News, one\" group-title=\"World\",News")
            .unwrap();
        let row = parser.push_line("../news.m3u8").unwrap().unwrap();
        assert_eq!(row.title, "News");
        assert_eq!(row.category, "World");
        assert_eq!(
            row.media,
            MediaLocator::Http {
                url: "https://tv.invalid:443/news.m3u8".into()
            }
        );
    }
    #[test]
    fn metadata_quotes_commas_and_duplicate_ids_are_preserved() {
        let result = parse_m3u("\u{feff}#EXTM3U\r\n#EXTINF:61.5 tvg-id='one' tvg-logo='/one.png' group-title='A,B',Film, part 1\r\nhttps://tv.invalid/1.mp4\n#EXTINF:-1 tvg-id='one',Other\nhttps://tv.invalid/2\n", &profile()).unwrap();
        assert_eq!(result.entries.len(), 2);
        assert_ne!(result.entries[0].id, result.entries[1].id);
        assert_eq!(result.entries[0].duration_ms, Some(61500));
        assert_eq!(result.entries[0].category, "A,B");
        assert_eq!(result.entries[0].poster, "https://tv.invalid:443/one.png");
    }
    #[test]
    fn invalid_transports_and_hls_segments_do_not_become_channels() {
        assert!(parse_m3u(
            "#EXTM3U\nfile:///private/movie.mp4\njavascript:alert(1)",
            &profile()
        )
        .unwrap()
        .entries
        .is_empty());
        assert_eq!(
            parse_m3u("#EXTM3U\n#EXT-X-TARGETDURATION:10\nsegment.ts", &profile()),
            Err(CatalogError::InvalidFormat)
        );
    }
}

// This proxy route uses one export timestamp shared by its channel URLs. Keep
// arbitrary q selectors everywhere else, and preserve the host and opaque path.
fn timestamped_channel_proxy(base: &str) -> bool {
    base.split_once("://").and_then(|(_, authority_path)| authority_path.split_once('/'))
        .and_then(|(_, path)| path.strip_prefix("channel/"))
        .and_then(|path| path.strip_suffix("/index.m3u8"))
        .is_some_and(|opaque| !opaque.is_empty()
            && opaque.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
}

fn stable_source(url: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let channel_proxy = timestamped_channel_proxy(base);
    let stable: Vec<&str> = query
        .split('&')
        .filter(|part| {
            let (raw_key, value) = part.split_once('=').unwrap_or((part, ""));
            let key = raw_key.to_ascii_lowercase();
            !part.is_empty()
                && !(channel_proxy && raw_key == "q" && value.len() == 13
                    && value.bytes().all(|byte| byte.is_ascii_digit()))
                && !matches!(
                    key.as_str(),
                    "token"
                        | "access_token"
                        | "auth"
                        | "key"
                        | "signature"
                        | "sig"
                        | "expires"
                        | "exp"
                        | "username"
                        | "password"
                        | "session"
                )
        })
        .collect();
    if stable.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", stable.join("&"))
    }
}

/// Source-advertised XMLTV references, resolved with the same strict URL policy.
pub fn epg_urls(header: &str, base: &str) -> Vec<String> {
    let fields = attributes(
        header
            .trim_start_matches('\u{feff}')
            .trim_start_matches("#EXTM3U"),
    );
    ["url-tvg", "x-tvg-url", "tvg-url"]
        .into_iter()
        .filter_map(|key| fields.get(key))
        .flat_map(|value| value.split(','))
        .filter_map(|url| resolve_http_url(base, url.trim()))
        .collect()
}

#[cfg(test)]
mod guide_metadata_tests {
    use super::*;
    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "p".into(),
            name: "P".into(),
            kind: ProviderKind::M3u,
            source: "https://list.invalid/list.m3u".into(),
            source_alias: String::new(),
            enabled: true,
        }
    }
    #[test]
    fn header_shift_is_inherited_entry_shift_overrides_without_changing_ids() {
        let p = profile();
        let text="#EXTM3U tvg-shift=\"1.5\"\n#EXTINF:-1 tvg-id=\"world\" tvg-name=\"Guide name\",Display name\nhttps://video.invalid/live\n#EXTINF:-1 tvg-shift=\"-1\",Other\nhttps://video.invalid/other\n";
        let catalog = parse_m3u(text, &p).unwrap();
        assert_eq!(
            catalog.entries[0]
                .live
                .as_ref()
                .unwrap()
                .guide_shift_seconds,
            5400
        );
        assert_eq!(
            catalog.entries[0].live.as_ref().unwrap().tvg_name,
            "Guide name"
        );
        assert_eq!(
            catalog.entries[1]
                .live
                .as_ref()
                .unwrap()
                .guide_shift_seconds,
            -3600
        );
        let changed = parse_m3u(&text.replace("1.5", "0"), &p).unwrap();
        assert_eq!(catalog.entries[0].id, changed.entries[0].id);
    }
    #[test]
    fn vod_has_no_live_metadata_and_invalid_shift_does_not_overflow() {
        let p = profile();
        let c=parse_m3u("#EXTM3U tvg-shift=\"3\"\n#EXTINF:120 tvg-name=\"Movie\",Movie\nhttps://video.invalid/movie\n#EXTINF:-1 tvg-shift=\"NaN\",Live\nhttps://video.invalid/live",&p).unwrap();
        assert!(c.entries[0].live.is_none());
        assert_eq!(c.entries[1].live.as_ref().unwrap().guide_shift_seconds, 0);
    }

    #[test]
    fn partial_header_catchup_is_completed_by_entry_and_retains_guide_metadata() {
        let p = profile();
        let c = parse_m3u("#EXTM3U catchup=\"append\" catchup-days=\"3\" tvg-shift=\"1.5\"\n#EXTINF:-1 catchup-source=\"?start={utc}&duration={duration}\" catchup-correction=\"-1\" tvg-name=\"Guide name\",News\nhttps://video.invalid/live", &p).unwrap();
        let live = c.entries[0].live.as_ref().unwrap();
        let catchup = live.catchup.as_ref().unwrap();
        assert_eq!(live.guide_shift_seconds, 5400);
        assert_eq!(live.tvg_name, "Guide name");
        assert_eq!(catchup.mode, super::super::archive::CatchupMode::Append);
        assert_eq!(catchup.retention_seconds, 3 * 86400);
        assert_eq!(catchup.correction_seconds, -3600);
        let target = super::super::archive::plan(
            catchup,
            "https://video.invalid/live",
            super::super::archive::ArchiveInterval {
                start: 5000,
                end: 5600,
            },
            6000,
        )
        .unwrap();
        assert_eq!(
            target.url,
            "https://video.invalid:443/live?start=1400&duration=600"
        );
    }

    #[test]
    fn explicit_disabled_or_invalid_catchup_never_recovers_header_defaults() {
        let header = "#EXTM3U catchup=\"default\" catchup-days=\"2\" catchup-source=\"https://video.invalid/{utc}\"\n";
        for override_ in [
            "catchup=\"none\"",
            "catchup-source=\"\"",
            "catchup-days=\"NaN\"",
            "catchup-days=\"0\"",
            "catchup-correction=\"NaN\"",
        ] {
            let c = parse_m3u(
                &format!("{header}#EXTINF:-1 {override_},News\nhttps://video.invalid/live"),
                &profile(),
            )
            .unwrap();
            assert!(
                c.entries[0].live.as_ref().unwrap().catchup.is_none(),
                "{override_}"
            );
        }
    }

    #[test]
    fn catchup_does_not_change_identity_or_turn_finite_files_into_live_channels() {
        let plain = "#EXTM3U\n#EXTINF:-1 tvg-id=\"news\",News\nhttps://video.invalid/live\n#EXTINF:120,Movie\nhttps://video.invalid/movie";
        let archive = plain.replace("#EXTM3U", "#EXTM3U catchup=\"shift\" catchup-days=\"2\"");
        let a = parse_m3u(plain, &profile()).unwrap();
        let b = parse_m3u(&archive, &profile()).unwrap();
        assert_eq!(a.entries[0].id, b.entries[0].id);
        assert!(b.entries[0].live.as_ref().unwrap().catchup.is_some());
        assert!(b.entries[1].live.is_none());
    }

    #[test]
    fn missing_depth_preserves_supported_rules_without_granting_archive() {
        for mode in ["default", "append", "shift", "flussonic"] {
            for header in [true, false] {
                let rule =
                    format!("catchup=\"{mode}\" catchup-source=\"?start={{utc}}&end={{utcend}}\"");
                let input = if header {
                    format!("#EXTM3U {rule}\n#EXTINF:-1,News\nhttps://video.invalid/index.m3u8")
                } else {
                    format!("#EXTM3U\n#EXTINF:-1 {rule},News\nhttps://video.invalid/index.m3u8")
                };
                let catalog = parse_m3u(&input, &profile()).unwrap();
                let source = catalog.entries[0]
                    .live
                    .as_ref()
                    .unwrap()
                    .catchup
                    .as_ref()
                    .expect("supported source survives until profile fallback is applied");
                assert_eq!(source.retention_seconds, 0);
                assert!(
                    super::super::archive::plan(
                        source,
                        "https://video.invalid/index.m3u8",
                        super::super::archive::ArchiveInterval {
                            start: 1000,
                            end: 1100
                        },
                        1500
                    )
                    .is_err(),
                    "a missing depth alone never grants archive access"
                );
            }
        }
    }

    #[test]
    fn explicit_bad_depth_and_unsupported_rules_do_not_become_fallback_candidates() {
        for depth in ["0", "-1", "", "NaN", "Infinity"] {
            for header in [true, false] {
                let rule = format!("catchup=\"shift\" catchup-days=\"{depth}\"");
                let input = if header {
                    format!("#EXTM3U {rule}\n#EXTINF:-1,News\nhttps://video.invalid/live")
                } else {
                    format!("#EXTM3U catchup=\"shift\" catchup-days=\"2\"\n#EXTINF:-1 {rule},News\nhttps://video.invalid/live")
                };
                let catalog = parse_m3u(&input, &profile()).unwrap();
                assert!(
                    catalog.entries[0].live.as_ref().unwrap().catchup.is_none(),
                    "{input}"
                );
            }
        }
        for rule in [
            "",
            "catchup=\"none\"",
            "catchup=\"unknown\"",
            "catchup=\"default\"",
            "catchup=\"append\"",
        ] {
            let input = format!("#EXTM3U {rule}\n#EXTINF:-1,News\nhttps://video.invalid/live");
            assert!(parse_m3u(&input, &profile()).unwrap().entries[0]
                .live
                .as_ref()
                .unwrap()
                .catchup
                .is_none());
        }
    }
}

#[cfg(test)]
mod advertised_guide_alias_tests {
    #[test]
    fn tvg_url_alias_remains_an_explicit_provider_guide() {
        assert_eq!(super::epg_urls("#EXTM3U tvg-url=\"guide.xml\"", "https://provider.invalid/list.m3u"),
            ["https://provider.invalid:443/guide.xml"]);
    }
}

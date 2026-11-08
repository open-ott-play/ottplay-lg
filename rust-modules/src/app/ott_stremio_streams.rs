//! Pure Stremio stream-response normalization. These values are proposals, never
//! playback authority or companion leases. The existing runtime performs source
//! policy checks and asks the user to choose before any network/media operation.

use plx_plex::plex::urlenc_str;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidResponse,
    ResponseLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Feature {
    ExternalPlayer,
    Headers,
    Drm,
    Archive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rejection {
    InvalidShape,
    Unsupported(Feature),
    InvalidSource,
    InvalidFileIndex,
    InvalidTrackers,
    SourceLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rejected {
    pub position: usize,
    pub reason: Rejection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FileChoice {
    Largest,
    Index(u32),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TorrentProposal {
    info_hash: String,
    trackers: Vec<String>,
    pub file: FileChoice,
}
impl TorrentProposal {
    pub fn info_hash(&self) -> &str {
        &self.info_hash
    }
    pub fn trackers(&self) -> &[String] {
        &self.trackers
    }

    /// Still an untrusted candidate: creating a session remains an explicit
    /// existing-runtime operation with captured policy/deadline/cancellation.
    pub fn magnet_candidate(&self) -> String {
        let mut value = format!("magnet:?xt=urn:btih:{}", self.info_hash());
        for tracker in self.trackers() {
            value.push_str("&tr=");
            value.push_str(&urlenc_str(tracker));
        }
        value
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SourceProposal {
    Http { url: String },
    Torrent(TorrentProposal),
}
impl std::fmt::Debug for SourceProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Http { .. } => "HttpProposal([untrusted])",
            Self::Torrent(_) => "TorrentProposal([untrusted])",
        })
    }
}

#[derive(Clone)]
pub(super) struct Candidate {
    pub title: String,
    pub description: String,
    pub source: SourceProposal,
}
impl std::fmt::Debug for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidate")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct Streams {
    pub candidates: Vec<Candidate>,
    pub rejected: Vec<Rejected>,
    pub duplicate_count: usize,
}

/// A valid empty response is different from a response whose entries require
/// unsupported capabilities. No response text, URL or credential enters errors.
pub(super) fn parse(bytes: &[u8], addon_name: &str) -> Result<Streams, Error> {
    if bytes.len() > 524_288 {
        return Err(Error::ResponseLimit);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let rows = value["streams"].as_array().ok_or(Error::InvalidResponse)?;
    if rows.len() > 256 {
        return Err(Error::ResponseLimit);
    }
    let fallback = plain(addon_name, 256).unwrap_or_default();
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut duplicate_count = 0;
    for (position, row) in rows.iter().enumerate() {
        match candidate(row, &fallback) {
            Ok(candidate) if seen.insert(candidate.source.clone()) => candidates.push(candidate),
            Ok(_) => duplicate_count += 1,
            Err(reason) => rejected.push(Rejected { position, reason }),
        }
    }
    Ok(Streams {
        candidates,
        rejected,
        duplicate_count,
    })
}

fn candidate(value: &Value, fallback: &str) -> Result<Candidate, Rejection> {
    if !value.is_object() {
        return Err(Rejection::InvalidShape);
    }
    let hints = value.get("behaviorHints");
    if hints.is_some_and(|value| !value.is_object()) {
        return Err(Rejection::InvalidShape);
    }
    for (field, feature) in [
        ("externalUrl", Feature::ExternalPlayer),
        ("ytId", Feature::ExternalPlayer),
        ("nzbUrl", Feature::Archive),
        ("rarUrls", Feature::Archive),
        ("zipUrls", Feature::Archive),
        ("7zipUrls", Feature::Archive),
        ("tgzUrls", Feature::Archive),
        ("tarUrls", Feature::Archive),
        ("headers", Feature::Headers),
        ("requestHeaders", Feature::Headers),
        ("responseHeaders", Feature::Headers),
        ("proxyHeaders", Feature::Headers),
        ("drm", Feature::Drm),
        ("drmInfo", Feature::Drm),
        ("licenseUrl", Feature::Drm),
        ("licenseKey", Feature::Drm),
        ("licenseType", Feature::Drm),
    ] {
        if value.get(field).is_some() || hints.is_some_and(|hints| hints.get(field).is_some()) {
            return Err(Rejection::Unsupported(feature));
        }
    }
    let source = if let Some(url) = value.get("url") {
        if ["infoHash", "fileIdx", "sources"]
            .iter()
            .any(|field| value.get(*field).is_some())
        {
            return Err(Rejection::InvalidSource);
        }
        let url = url
            .as_str()
            .filter(|url| web_url(url, 4096))
            .ok_or(Rejection::InvalidSource)?;
        // Validate without rewriting signed paths/query parameters.
        SourceProposal::Http { url: url.into() }
    } else {
        let hash = value["infoHash"]
            .as_str()
            .filter(|hash| info_hash(hash))
            .ok_or(Rejection::InvalidSource)?;
        let file = match value.get("fileIdx") {
            None => FileChoice::Largest,
            Some(value) => FileChoice::Index(
                value
                    .as_f64()
                    .filter(|index| {
                        *index >= 0.0 && *index <= i32::MAX as f64 && index.fract() == 0.0
                    })
                    .ok_or(Rejection::InvalidFileIndex)? as u32,
            ),
        };
        let mut trackers = BTreeSet::new();
        if let Some(sources) = value.get("sources") {
            let sources = sources
                .as_array()
                .filter(|values| values.len() <= 32)
                .ok_or(Rejection::InvalidTrackers)?;
            for source in sources {
                let source = source.as_str().ok_or(Rejection::InvalidTrackers)?;
                if source
                    .get(..4)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("dht:"))
                    && info_hash(&source[4..])
                {
                    continue;
                }
                let tracker = source
                    .strip_prefix("tracker:")
                    .ok_or(Rejection::InvalidTrackers)?;
                let valid = if tracker
                    .get(..6)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("udp://"))
                {
                    web_url(&format!("http:{}", &tracker[4..]), 2049) && tracker.len() <= 2048
                } else {
                    web_url(tracker, 2048)
                };
                if !valid {
                    return Err(Rejection::InvalidTrackers);
                }
                trackers.insert(tracker.to_owned());
            }
        }
        let torrent = TorrentProposal {
            info_hash: hash.to_ascii_lowercase(),
            trackers: trackers.into_iter().collect(),
            file,
        };
        if torrent.magnet_candidate().len() > 16_384 {
            return Err(Rejection::SourceLimit);
        }
        SourceProposal::Torrent(torrent)
    };
    Ok(Candidate {
        title: value["name"]
            .as_str()
            .and_then(|value| plain(value, 256))
            .or_else(|| value["title"].as_str().and_then(|value| plain(value, 256)))
            .unwrap_or_else(|| fallback.into()),
        description: value["description"]
            .as_str()
            .and_then(|value| plain(value, 2048))
            .or_else(|| value["title"].as_str().and_then(|value| plain(value, 2048)))
            .unwrap_or_default(),
        source,
    })
}

fn plain(value: &str, limit: usize) -> Option<String> {
    (!value.trim().is_empty()
        && value.len() <= limit
        && !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\r' | '\n' | '\t')))
    .then(|| value.trim().into())
}
fn info_hash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn web_url(value: &str, limit: usize) -> bool {
    if value.len() > limit
        || value != value.trim()
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '#' | '\\' | '|' | '<' | '>' | '"' | '\''))
    {
        return false;
    }
    // The shared native validator owns URI syntax. The extra escape check
    // matches Classic's decoded-control/backslash rejection without rewriting.
    let bytes = value.as_bytes();
    let mut decoded_bytes = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(pair) = bytes.get(index + 1..index + 3) else {
                return false;
            };
            let (Some(high), Some(low)) = (hex(pair[0]), hex(pair[1])) else {
                return false;
            };
            let decoded = high * 16 + low;
            if decoded < 32 || matches!(decoded, 127 | b'\\') {
                return false;
            }
            decoded_bytes.push(decoded);
            index += 3;
        } else {
            decoded_bytes.push(bytes[index]);
            index += 1;
        }
    }
    if std::str::from_utf8(&decoded_bytes).is_err() {
        return false;
    }
    plx_data::ott::http_url(value).is_some()
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "ott_stremio_streams/tests.rs"]
mod tests;

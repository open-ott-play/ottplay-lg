//! Guide matching and time selection; no network, clock or separate guide store.
use super::{
    epg::{Channel, Programme},
    CatalogError,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[path = "guide_rows.rs"]
pub mod rows;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelMetadata {
    #[serde(default)]
    pub tvg_name: String,
    /// Provider programme identifier when it differs from the live stream identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_id: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guide_feed: String,
    #[serde(default)]
    pub guide_shift_seconds: i64,
    #[serde(default)]
    pub catchup: Option<super::archive::CatchupSource>,
}

impl std::fmt::Debug for ChannelMetadata {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ChannelMetadata").field("tvg_name",&self.tvg_name)
            .field("guide_shift_seconds",&self.guide_shift_seconds).field("catchup",&self.catchup)
            .field("guide_id_present",&self.guide_id.is_some()).field("guide_feed_present",&!self.guide_feed.is_empty()).finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub generation: u64,
    pub profile_id: String,
    pub entry_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Query {
    CurrentNext { now: i64 },
    Day { start: i64, end: i64 },
    /// The complete admitted retention interval, not a single calendar day.
    Records { start: i64, end: i64 },
}
impl Query {
    pub fn validate(self) -> Result<Self, CatalogError> {
        match self {
            Self::CurrentNext { now } if now >= 0 => Ok(self),
            // UTC boundaries are supplied by the UI's clock; local DST days can be 23/25 hours.
            Self::Day { start, end } if start >= 0 && end > start && end - start <= 172800 => {
                Ok(self)
            }
            Self::Records { start, end } if super::records::valid_window(start, end) => Ok(self),
            _ => Err(CatalogError::InvalidFormat),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub context: Context,
    pub query: Query,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    RequestedWindow,
    ProviderShortGuide,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultSet {
    pub context: Context,
    pub query: Query,
    pub coverage: Coverage,
    pub programmes: Vec<Programme>,
}
impl ResultSet {
    pub fn can_publish(&self, current: &Request, cancelled: bool) -> bool {
        self.query == current.query && (!cancelled && self.context == current.context)
    }
}

pub(super) fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Exact ID wins. A name match must identify exactly one XMLTV channel.
pub fn channel_id(
    channels: &[Channel],
    id: Option<&str>,
    tvg_name: &str,
    title: &str,
) -> Option<String> {
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        if channels.iter().any(|c| c.id == id) {
            return Some(id.to_string());
        }
    }
    for name in [tvg_name, title] {
        let name = normalized(name);
        if name.is_empty() {
            continue;
        }
        let matches: BTreeSet<&str> = channels
            .iter()
            .filter(|c: &&Channel| c.names.iter().any(|n| normalized(n) == name))
            .map(|c| c.id.as_str())
            .collect();
        if matches.len() == 1 {
            return matches.first().map(|id| (*id).to_string());
        }
        if matches.len() > 1 {
            return None;
        }
    }
    None
}

pub fn select(
    rows: &[Programme],
    channel: &str,
    query: Query,
    shift: i64,
) -> Result<Vec<Programme>, CatalogError> {
    query.validate()?;
    let mut rows: Vec<Programme> = rows
        .iter()
        .filter(|p| p.channel == channel)
        .map(|p| {
            let mut p = p.clone();
            p.start = p
                .start
                .checked_add(shift)
                .ok_or(CatalogError::InvalidFormat)?;
            p.end = p
                .end
                .checked_add(shift)
                .ok_or(CatalogError::InvalidFormat)?;
            if p.start < 0 || p.end <= p.start {
                return Err(CatalogError::InvalidFormat);
            }
            Ok(p)
        })
        .collect::<Result<_, _>>()?;
    // Records retains source order for equal starts. Its presentation alone
    // chooses the latest exact title, without destroying programme identity.
    if let Query::Records { start, end } = query {
        return Ok(rows.into_iter().filter(|p| p.start > start && p.end <= end).collect());
    }
    rows.sort_by_key(|p| (p.start, p.end));
    rows.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.title == b.title);
    match query {
        Query::Day { start, end } => Ok(rows
            .into_iter()
            .filter(|programme| programme.start < end && programme.end > start)
            .collect()),
        Query::CurrentNext { now } => {
            let current = rows.iter().rposition(|p| p.start <= now && now < p.end);
            let boundary = current.map_or(now, |i| rows[i].end);
            let next = rows.iter().position(|p| p.start >= boundary);
            Ok(current
                .into_iter()
                .chain(next)
                .map(|i| rows[i].clone())
                .collect())
        }
        Query::Records { .. } => unreachable!("records returned above"),
    }
}

#[cfg(test)]
#[path = "guide_tests.rs"]
mod tests;

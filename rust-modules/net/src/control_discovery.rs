//! Explicit, bounded DNS-SD discovery for native command-server selection.
//! Only the existing off-frame control worker calls this module. No listener,
//! multicast responder, credential, persisted endpoint, or automatic pairing.
use plx_base::task::OffFrame;
use std::{collections::{BTreeMap, BTreeSet}, time::{Duration, Instant}};
mod wire;
mod system;
const SERVICE: &str = "_ottplay-ctrl._tcp";
const PTR: u16 = 12;
const TXT: u16 = 16;
const SRV: u16 = 33;
const MAX_DOMAINS: usize = 16;
const MAX_SERVERS: usize = 8;
pub const WINDOW_MS: u64 = 3000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error { Unsupported, Unavailable, TimedOut, Malformed, TooLarge }
#[derive(Debug, PartialEq, Eq)]
pub struct ControlServer { pub id: String, pub domain: String, pub address: String }
#[derive(Debug)]
struct Record { data: Vec<u8>, ttl: Duration }

pub fn supported() -> bool { cfg!(target_os = "linux") }

/// An OS lookup itself can block beyond the UI deadline. The app must retain
/// its sole worker slot until it returns; cancellation only prevents more IO.
pub fn discover(_worker: &OffFrame, canceled: impl Fn() -> bool)
    -> Result<Vec<ControlServer>, Error>
{
    let start = Instant::now();
    discover_with(system::domains(), || !canceled()
        && start.elapsed() < Duration::from_millis(WINDOW_MS), system::query)
}

fn discover_with(
    domains: Vec<String>, current: impl Fn() -> bool,
    mut query: impl FnMut(&str, u16, bool) -> Result<wire::Answer, Error>,
) -> Result<Vec<ControlServer>, Error> {
    let domains: BTreeSet<_> = domains.into_iter()
        .filter_map(|d| canonical_host(&d)).collect();
    if domains.len() > MAX_DOMAINS { return Err(Error::TooLarge); }
    // No readable resolv.conf in an app jail does not prove the OS lacks DNS.
    // res_search uses the process resolver's own suffix configuration. It finds
    // its first matching suffix; explicit readable suffixes are all queried.
    let lookups: Vec<_> = if domains.is_empty() { vec![(SERVICE.into(), true)] }
        else { domains.into_iter().map(|d| (format!("{SERVICE}.{d}."), false)).collect() };
    let mut found = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (owner, search) in lookups {
        if !current() { return Err(Error::TimedOut); }
        let answer = query(&owner, PTR, search)?;
        if !current() { return Err(Error::TimedOut); }
        if answer.records.is_empty() { continue; }
        let actual = answer.question;
        let domain = actual.strip_prefix(&format!("{SERVICE}."))
            .and_then(canonical_host).ok_or(Error::Malformed)?;
        if !search && actual != owner { return Err(Error::Malformed); }
        for ptr in answer.records {
            let id = instance_name(&ptr.data, &actual).ok_or(Error::Malformed)?;
            if !seen.insert(id.clone()) { continue; }
            if seen.len() > MAX_SERVERS { return Err(Error::TooLarge); }
            if !current() { return Err(Error::TimedOut); }
            let srv = query(&id, SRV, false)?;
            if !current() { return Err(Error::TimedOut); }
            let txt = query(&id, TXT, false)?;
            if !current() { return Err(Error::TimedOut); }
            let (item, _) = server(&id, &domain, ptr.ttl, &srv.records, &txt.records)
                .ok_or(Error::Malformed)?;
            found.insert(id, item);
        }
    }
    Ok(found.into_values().collect())
}

// Shared with src-rs/core/src/control_discovery.rs (FOSS): canonical wire-name,
// instance, HTTPS TXT/SRV, port and base-path grammar. Keep both forms aligned.
fn canonical_host(value: &str) -> Option<String> {
    let value = value.strip_suffix('.').unwrap_or(value);
    if value.is_empty()
        || value.len() > 253
        || value.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

// RDATA names returned by system-resolver are expanded. Reject compression,
// arbitrary escapes and non-ASCII instance labels so IDs have one canonical
// representation on the player and control server.
fn wire_name(data: &[u8]) -> Option<String> {
    if data.len() > 255 {
        return None;
    }
    let mut pos = 0;
    let mut labels = Vec::new();
    loop {
        let size = *data.get(pos)? as usize;
        pos += 1;
        if size == 0 {
            return (pos == data.len() && !labels.is_empty())
                .then(|| format!("{}.", labels.join(".")));
        }
        if size > 63 {
            return None;
        }
        let label = data.get(pos..pos + size)?;
        if !label
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return None;
        }
        labels.push(std::str::from_utf8(label).ok()?.to_ascii_lowercase());
        pos += size;
    }
}

fn instance_name(data: &[u8], owner: &str) -> Option<String> {
    let name = wire_name(data)?;
    let label = name.strip_suffix(&format!(".{owner}"))?;
    // The instance must be one label under the exact queried service/domain.
    if label.contains('.') || canonical_host(label).is_none() {
        return None;
    }
    Some(name)
}

fn server(
    id: &str,
    domain: &str,
    ptr_ttl: Duration,
    srv: &[Record],
    txt: &[Record],
) -> Option<(ControlServer, Duration)> {
    // One instance is one pairing endpoint. Ambiguous SRV/TXT answers must not
    // silently choose a different server or combine inconsistent metadata.
    if srv.len() != 1 || txt.len() != 1 {
        return None;
    }
    let srv = &srv[0];
    let txt = &txt[0];
    if srv.data.len() < 7 {
        return None;
    }
    let port = u16::from_be_bytes([srv.data[4], srv.data[5]]);
    if port == 0 {
        return None;
    }
    let host = canonical_host(&wire_name(&srv.data[6..])?)?;
    let values = txt_values(&txt.data)?;
    if values.len() != 3 || values.get("txtvers")? != "1" || values.get("scheme")? != "https" {
        return None;
    }
    let path = base_path(values.get("path")?)?;
    let authority = if port == 443 {
        host
    } else {
        format!("{host}:{port}")
    };
    Some((
        ControlServer {
            id: id.into(),
            domain: domain.into(),
            address: format!("https://{authority}{path}"),
        },
        ptr_ttl.min(srv.ttl).min(txt.ttl),
    ))
}

fn base_path(path: &str) -> Option<&str> {
    if !path.starts_with('/') {
        return None;
    }
    let normalized = path.strip_suffix('/').unwrap_or(path);
    if normalized.is_empty() {
        return Some(normalized);
    }
    if normalized.split('/').skip(1).any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
    }) {
        return None;
    }
    Some(normalized)
}

fn txt_values(data: &[u8]) -> Option<BTreeMap<String, String>> {
    if data.len() > 2048 {
        return None;
    }
    let mut values = BTreeMap::new();
    let mut pos = 0;
    let mut text_size = 0;
    while pos < data.len() {
        let size = data[pos] as usize;
        pos += 1;
        let text = std::str::from_utf8(data.get(pos..pos + size)?).ok()?;
        let (key, value) = text.split_once('=')?;
        text_size += size;
        if text_size > 1024 || !matches!(key, "txtvers" | "scheme" | "path") {
            return None;
        }
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return None;
        }
        pos += size;
    }
    Some(values)
}


#[cfg(test)]
mod tests;

//! Public discovery values only. No IO, settings writes, credentials or fallback origin.
use super::protocol::{self, Request};
use serde::Deserialize;
use std::collections::HashSet;

pub(super) const CATALOG_BODY_LIMIT: usize = 8192;
pub(super) const CONTROLLER_BODY_LIMIT: usize = 65536;
pub(super) const UNCONFIGURED: &str = "Discovery is not configured. Enter a server address.";
pub(super) const INVALID_PROFILE: &str = "Discovery configuration is invalid. Enter a server address.";
pub(super) const FINDING: &str = "Finding command servers…";
pub(super) const CHOOSE: &str = "Select a command server";
pub(super) const EMPTY: &str = "No servers found. Try again or enter an address.";
pub(super) const FAILED: &str = "Discovery failed. Try again or enter an address.";
pub(super) const EXPIRED_CHOICE: &str = "Server choices changed. Find servers again.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CatalogProfile {
    url: String,
    digest: [u8; 32],
}
impl CatalogProfile {
    pub(super) fn configured() -> Result<Option<Self>, &'static str> {
        Self::from_config(option_env!("PLX_CONTROL_DISCOVERY_URL"),
            option_env!("PLX_CONTROL_DISCOVERY_SHA256"))
    }

    pub(super) fn from_config(url: Option<&str>, digest: Option<&str>)
        -> Result<Option<Self>, &'static str>
    {
        if matches!((url, digest), (None, None) | (Some(""), Some(""))) { return Ok(None); }
        let (Some(url), Some(digest)) = (url, digest) else { return Err(INVALID_PROFILE); };
        if !catalog_url(url) || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        { return Err(INVALID_PROFILE); }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16)
                .map_err(|_| INVALID_PROFILE)?;
        }
        Ok(Some(Self { url: url.into(), digest: bytes }))
    }

    pub(super) fn parse(&self, bytes: &[u8]) -> Result<Vec<Candidate>, &'static str> {
        if bytes.is_empty() || bytes.len() > CATALOG_BODY_LIMIT {
            return Err("Discovery catalog exceeds its size limit");
        }
        // Bind the exact fetched bytes before interpreting any nominated endpoint.
        if plx_base::sha256::sha256(bytes) != self.digest {
            return Err("Discovery catalog digest mismatch");
        }
        parse(bytes, true).map_err(|_| "Invalid discovery catalog")
    }
}

// Deliberately narrower than a general URL parser: this public build input must
// identify one exact HTTPS catalog, without shell syntax or normalized aliases.
fn catalog_url(value: &str) -> bool {
    if value.len() > 2048 || !value.is_ascii() { return false; }
    let Some(rest) = value.strip_prefix("https://") else { return false; };
    let Some((authority, path)) = rest.split_once('/') else { return false; };
    let (host, port) = authority.split_once(':').map_or((authority, None),
        |(host, port)| (host, Some(port)));
    if host.is_empty() || host.len() > 253 || !host.contains('.') { return false; }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        if !host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.to_string() == host) { return false; }
    } else if !host.split('.').all(|label| !label.is_empty() && label.len() <= 63
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-') && !label.ends_with('-'))
    { return false; }
    if port.is_some_and(|port| !port.parse::<u16>().is_ok_and(|n| n != 0 && n.to_string() == port)) {
        return false;
    }
    path.split('/').all(|part| !part.is_empty() && part != "." && part != ".."
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ControllerBase(String);
impl ControllerBase {
    pub(super) fn new(value: &str) -> Result<Self, &'static str> {
        protocol::normalize_base(value, false).map(Self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiscoveryTarget {
    ControllerBase(ControllerBase),
    CatalogUrl(CatalogProfile),
}
impl DiscoveryTarget {
    pub(super) fn request(&self) -> Request {
        Request { method: "GET", token: String::new(), body: None, url: match self {
            Self::ControllerBase(base) => format!("{}/api/discovery", base.0),
            Self::CatalogUrl(profile) => profile.url.clone(),
        } }
    }
    pub(super) fn body_limit(&self) -> usize {
        match self { Self::ControllerBase(_) => CONTROLLER_BODY_LIMIT,
            Self::CatalogUrl(_) => CATALOG_BODY_LIMIT }
    }
}

#[derive(Debug)]
pub(super) struct Candidate {
    pub(super) id: String,
    pub(super) domain: String,
    pub(super) address: String,
}
#[derive(Deserialize)]
struct Envelope { version: u8, servers: Vec<serde_json::Value> }
#[derive(Deserialize)]
struct Descriptor { id: String, address: String, domain: Option<serde_json::Value> }

pub(super) fn parse_controller(bytes: &[u8]) -> Result<Vec<Candidate>, &'static str> {
    if bytes.len() > CONTROLLER_BODY_LIMIT { return Err("No command server was discovered"); }
    parse(bytes, false)
}
fn parse(bytes: &[u8], require_domain: bool) -> Result<Vec<Candidate>, &'static str> {
    let value: Envelope = serde_json::from_slice(bytes).map_err(|_| "No command server was discovered")?;
    if value.version != 1 || value.servers.len() > 16 { return Err("No command server was discovered"); }
    let mut ids = HashSet::new();
    value.servers.into_iter().map(|item| {
        let item: Descriptor = serde_json::from_value(item)
            .map_err(|_| "Invalid command server discovery response")?;
        let domain = item.domain.as_ref().and_then(serde_json::Value::as_str);
        // Pairing and discovery share the exact same identity admission rule.
        if !protocol::valid_server_id(&item.id)
            || !ids.insert(item.id.clone()) || (require_domain && domain.is_none())
        { return Err("Invalid command server discovery response"); }
        // Manual descriptors historically ignored non-string optional metadata.
        let domain = domain.unwrap_or_default().to_owned();
        // A manual controller response does not require a catalog-style domain.
        // Keep the stricter domain contract for the digest-bound public catalog.
        if require_domain
            && (domain.len() > 253 || domain.chars().any(|c| c.is_control() || c.is_whitespace()))
        {
            return Err("Invalid command server discovery response");
        }
        let address = protocol::normalize_base(&item.address, false)
            .map_err(|_| "Invalid command server discovery response")?;
        Ok(Candidate { id: item.id, domain, address })
    }).collect()
}

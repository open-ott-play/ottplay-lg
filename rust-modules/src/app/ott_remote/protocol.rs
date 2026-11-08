//! Pure, bounded control-server wire protocol. Time and IO are supplied by the runner.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::VecDeque, net::IpAddr};

pub(super) const BODY_LIMIT: usize = 2 * 1024 * 1024;
const COMMAND_LIMIT: usize = 256;
const SEEN_LIMIT: usize = 2048;
const CACHE_LIMIT: usize = 50;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default = "version")]
    pub version: u8,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub token: String,
    pub device_id: String,
    #[serde(default, rename = "allowInsecureHttp")]
    pub allow_insecure_http: bool,
}
fn version() -> u8 {
    1
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlConfig")
            .field("enabled", &self.enabled)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}
impl Config {
    pub fn disabled(device_id: String) -> Self {
        Self {
            version: 1,
            enabled: false,
            address: String::new(),
            token: String::new(),
            device_id,
            allow_insecure_http: false,
        }
    }
    pub fn parse(value: Value) -> Result<Self, &'static str> {
        let mut config: Self =
            serde_json::from_value(value).map_err(|_| "Invalid control configuration")?;
        if config.version != 1 || !valid_device(&config.device_id) {
            return Err("Invalid device identity");
        }
        if !config.address.is_empty() {
            config.address = normalize_base(&config.address, config.allow_insecure_http)?;
        }
        if config.enabled && (config.address.is_empty() || !valid_token(&config.token)) {
            return Err("Control address or access token is missing");
        }
        if !config.token.is_empty() && !valid_token(&config.token) {
            return Err("Invalid access token");
        }
        Ok(config)
    }
}
pub(super) fn valid_device(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
pub(super) fn valid_token(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn command_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn request_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// An explicit base, never a credential-bearing URL or a redirect target. HTTP is
/// restricted to an explicitly opted-in numeric local address; discovery uses HTTPS.
pub(super) fn normalize_base(value: &str, allow_http: bool) -> Result<String, &'static str> {
    let value = value.trim().trim_end_matches('/');
    if value.len() > 2048
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || value.contains(['\\', '?', '#', '@', '%'])
    {
        return Err("Use an explicit control server address without credentials or query");
    }
    let (scheme, rest, port) = if let Some(rest) = value.strip_prefix("https://") {
        ("https", rest, 443)
    } else if let Some(rest) = value.strip_prefix("http://") {
        ("http", rest, 80)
    } else {
        return Err("Control server must use HTTPS");
    };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    if authority.is_empty() || path.split('/').any(|s| s == "." || s == "..") {
        return Err("Invalid control server address");
    }
    let origin =
        plx_net::net::origin::Origin::parse_connection(&format!("{scheme}://{authority}"), port)
            .ok_or("Invalid control server address")?;
    if scheme == "http" {
        if !allow_http {
            return Err("HTTP control requires explicit local-network consent");
        }
        let local = match origin.host().parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
            Ok(IpAddr::V6(ip)) => {
                ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
            }
            _ => false,
        };
        if !local {
            return Err("HTTP control is restricted to a numeric local address");
        }
    }
    let mut path = if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    };
    for suffix in ["/api/webhook/commands", "/webhook/poll", "/webhook/notify"] {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            break;
        }
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b))
    {
        return Err("Invalid control server path");
    }
    Ok(format!("{}{path}", origin.base()))
}

#[derive(Clone)]
pub(super) struct Request {
    pub method: &'static str,
    pub url: String,
    pub token: String,
    pub body: Option<String>,
}
impl Request {
    pub fn new(config: &Config, method: &'static str, path: &str, body: Option<Value>) -> Self {
        Self {
            method,
            url: format!("{}{path}", config.address),
            token: config.token.clone(),
            body: body.map(|v| v.to_string()),
        }
    }
}
pub(super) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct Invocation {
    pub id: String,
    pub action: String,
    pub params: Value,
    /// Validated received action/params bytes for media_tracks only. The raw poll
    /// boundary supplies them; Value-only legacy/test admission cannot mint proof.
    pub(super) media_tracks_raw: Option<String>,
    /// Received duplicate-checked action/params projection; absent for Value-only admission.
    pub(super) plex_servers_raw: Option<String>,
    pub deadline_ms: u64,
    pub(super) request: bool,
    pub(super) generation: u64,
}
impl Invocation {
    pub(crate) fn media_tracks_projection(&self) -> Option<&[u8]> {
        self.media_tracks_raw.as_deref().map(str::as_bytes)
    }
    pub(crate) fn plex_servers_projection(&self) -> Option<&[u8]> {
        self.plex_servers_raw.as_deref().map(str::as_bytes)
    }
    pub(crate) fn expects_response(&self) -> bool {
        self.request
    }
}
impl std::fmt::Debug for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation")
            .field("id", &self.id)
            .field("action", &self.action)
            .finish_non_exhaustive()
    }
}
#[derive(Default)]
pub(super) struct Delivery {
    seen_commands: VecDeque<String>,
    seen_requests: VecDeque<String>,
    pending_requests: VecDeque<(String, u64)>,
    acks: VecDeque<String>,
    responses: VecDeque<Value>,
    cache: VecDeque<(String, Value)>,
    cache_bytes: usize,
}
#[derive(Clone)]
pub(super) enum SendKind {
    Poll,
    Ack(Vec<String>),
    Response,
    Presence,
}

fn deadline(item: &Value, server_time: f64, now: u64, elapsed_ms: u64) -> Option<u64> {
    let seconds = item.get("expires_at")?.as_f64()? - server_time;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let remaining = (seconds * 1000.0).min(60000.0) as u64;
    remaining
        .checked_sub(elapsed_ms)
        .filter(|left| *left > 0)
        .map(|left| now.saturating_add(left))
}
pub(crate) fn favorites_rejection(params: &Value, code: &str) -> Value {
    json!({"status":"rejected","data":{"version":1,
        "runtime":params["runtime"].as_str().filter(|s|s.len()<=64).unwrap_or(""),
        "op":params["op"].as_str().filter(|s|s.len()<=16).unwrap_or(""),
        "subject":"global","error":code}})
}
fn refused_request(id: &str, action: &str, params: &Value, code: &str, message: &str) -> Value {
    let mut response = if matches!(action, "favorites_global_items" | "favorites_global_recovery") {
        let code = if action == "favorites_global_recovery" && code == "cancelled" { "expired" } else { code };
        favorites_rejection(params, code) }
        else if action == "plex_servers" {
            let error = match code {
                "expired" => super::plex_servers::Error::Expired,
                "cancelled" => super::plex_servers::Error::Cancelled,
                "limit_exceeded" => super::plex_servers::Error::LimitExceeded,
                _ => super::plex_servers::Error::InvalidRequest,
            };
            super::plex_servers::Correlation::from_params(params)
                .map_or_else(|| json!({"status":"rejected","data":{"error":message}}),
                    |correlation| correlation.rejected(error))
        }
        else if action == "local_browser" || (action == "local_media" && params.get("source_handle").is_some()) {
            // Delivery may expire a request before the application sees it. Keep
            // the original correlation and the family's closed error vocabulary;
            // never substitute a new runtime or imply that the command ran.
            let code = if action == "local_media" { "runtime_changed" } else { code };
            json!({"status":"rejected","data":{"version":1,
                "runtime":params["runtime"].as_str().filter(|s|s.len()<=256).unwrap_or(""),
                "op":params["op"].as_str().filter(|s|s.len()<=16).unwrap_or(""),
                "state":"error","error":code}})
        }
        else { json!({"status":"rejected","data":{"error":message}}) };
    response["id"] = json!(id);
    response
}

fn remember(list: &mut VecDeque<String>, id: &str) {
    list.push_back(id.to_owned());
    while list.len() > SEEN_LIMIT {
        list.pop_front();
    }
}
impl Delivery {
    pub fn next(&self, config: &Config) -> (SendKind, Request) {
        if !self.acks.is_empty() {
            let ids: Vec<_> = self.acks.iter().take(50).cloned().collect();
            let request = Request::new(
                config,
                "POST",
                &format!("/api/webhook/commands/ack?device_id={}", config.device_id),
                Some(json!({"ids":ids})),
            );
            (SendKind::Ack(ids), request)
        } else if let Some(response) = self.responses.front() {
            (
                SendKind::Response,
                Request::new(config, "POST", "/api/responses", Some(response.clone())),
            )
        } else {
            (
                SendKind::Poll,
                Request::new(
                    config,
                    "GET",
                    &format!(
                        "/api/webhook/commands?device_id={}&delivery=ack",
                        config.device_id
                    ),
                    None,
                ),
            )
        }
    }
    pub fn idle(&self) -> bool {
        self.acks.is_empty() && self.responses.is_empty()
    }
    pub fn accepted(
        &mut self,
        kind: &SendKind,
        value: &Value,
    ) -> Result<Option<String>, &'static str> {
        if value["status"] != "ok" {
            return Err("Invalid control acknowledgement");
        }
        let mut completed = None;
        match kind {
            SendKind::Ack(ids) => self.acks.retain(|id| !ids.contains(id)),
            SendKind::Response => {
                completed = self
                    .responses
                    .pop_front()
                    .and_then(|v| v["id"].as_str().map(str::to_owned));
            }
            _ => {}
        }
        Ok(completed)
    }
    pub fn expired_response(&mut self) -> Option<String> {
        self.responses
            .pop_front()
            .and_then(|value| value["id"].as_str().map(str::to_owned))
    }
    pub fn poll(
        &mut self,
        value: &Value,
        now: u64,
        elapsed_ms: u64,
    ) -> Result<Vec<Invocation>, &'static str> {
        let commands = value["commands"]
            .as_array()
            .ok_or("Invalid command envelope")?;
        if commands.len() > COMMAND_LIMIT
            || commands
                .iter()
                .any(|cmd| !cmd["id"].as_str().is_some_and(command_id))
        {
            return Err("Invalid command envelope");
        }
        let requests = value.get("requests").and_then(Value::as_array);
        if value.get("requests").is_some() && requests.is_none()
            || requests.is_some_and(|v| v.len() > COMMAND_LIMIT)
        {
            return Err("Invalid request envelope");
        }
        // Validate the complete admitted request before marking any command seen.
        // Otherwise a malformed request could discard already-admitted commands.
        if let Some(request) = requests.and_then(|items| items.first()) {
            if !request["id"].as_str().is_some_and(request_id)
                || !request["action"].as_str().is_some_and(|v| {
                    !v.is_empty()
                        && v.len() <= 80
                        && v.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                })
                || request
                    .get("params")
                    .is_some_and(|params| !params.is_object())
            {
                return Err("Invalid request envelope");
            }
        }
        let server_time = value["server_time"].as_f64();
        let mut actions = Vec::new();
        for command in commands {
            let id = command["id"].as_str().expect("validated");
            if !self.seen_commands.iter().any(|seen| seen == id) {
                remember(&mut self.seen_commands, id);
                let until = match (command.get("expires_at"), server_time) {
                    (None, _) => Some(now.saturating_add(60000)),
                    (Some(_), Some(time)) => deadline(command, time, now, elapsed_ms),
                    _ => None,
                };
                if let Some(until) = until {
                    actions.push(Invocation {
                        id: id.into(),
                        action: "command".into(),
                        params: command.clone(),
                        media_tracks_raw: None,
                        plex_servers_raw: None,
                        deadline_ms: until,
                        request: false,
                        generation: 0,
                    });
                }
            }
            if !self.acks.iter().any(|ack| ack == id) {
                self.acks.push_back(id.into());
            }
        }
        if let Some(request) = requests.and_then(|requests| requests.first()) {
            let id = request["id"]
                .as_str()
                .filter(|id| request_id(id))
                .ok_or("Invalid request identity")?;
            let action = request["action"]
                .as_str()
                .filter(|v| {
                    !v.is_empty()
                        && v.len() <= 80
                        && v.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                })
                .ok_or("Invalid request action")?;
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            if !params.is_object() {
                return Err("Invalid request parameters");
            }
            self.pending_requests.retain(|(_, until)| *until > now);
            if self
                .pending_requests
                .iter()
                .any(|(pending, _)| pending == id)
            {
                // The app may still be persisting or resolving this request. A
                // duplicate poll must not overwrite its eventual result.
            } else if let Some((_, response)) = self.cache.iter().find(|(seen, _)| seen == id) {
                self.responses.push_back(response.clone());
            } else if self.seen_requests.iter().any(|seen| seen == id) {
                // Receipt expiry/eviction says nothing about managed activation effects.
                // Without an original cached result, remain silent: duplicate params
                // cannot reconstruct its correlation or justify cancellation/retry.
                if action != "plex_servers" {
                    self.responses.push_back(refused_request(id, action, &params, "cancelled", "Request receipt expired; submit a new request"));
                }
            } else {
                remember(&mut self.seen_requests, id);
                let until = server_time.and_then(|time| deadline(request, time, now, elapsed_ms));
                if let Some(until) = until {
                    if self.pending_requests.len() >= SEEN_LIMIT {
                        return Err("Too many pending native requests");
                    }
                    self.pending_requests.push_back((id.into(), until));
                    actions.push(Invocation {
                        id: id.into(),
                        action: action.into(),
                        params,
                        media_tracks_raw: None,
                        plex_servers_raw: None,
                        deadline_ms: until,
                        request: true,
                        generation: 0,
                    });
                } else {
                    self.responses.push_back(
                        refused_request(id, action, &params, "expired", "Request expired"),
                    );
                }
            }
        }
        Ok(actions)
    }
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn response_for_id_for_test(&self, id: &str) -> Option<Value> {
        self.cache.iter().find(|(key, _)| key == id).map(|(_, value)| value.clone())
    }

    pub fn complete(&mut self, invocation: Invocation, result: Value) -> bool {
        if !invocation.expects_response() || self.cache.iter().any(|(id, _)| id == &invocation.id) {
            return false;
        }
        self.pending_requests.retain(|(id, _)| id != &invocation.id);
        let valid = result["status"]
            .as_str()
            .is_some_and(|s| matches!(s, "ok" | "rejected" | "unsupported"))
            && result.get("data").is_some();
        let mut response = if valid {
            json!({"id":invocation.id,"status":result["status"],"data":result["data"]})
        } else {
            json!({"id":invocation.id,"status":"rejected","data":{"error":"Invalid native result"}})
        };
        let response_limit = if invocation.action == "plex_servers" { 64 * 1024 } else { BODY_LIMIT };
        // Measure the complete escaped envelope after adding the original request ID.
        if response.to_string().len() > response_limit {
            response = if invocation.action == "plex_servers" {
                refused_request(&invocation.id, &invocation.action, &invocation.params,
                    "limit_exceeded", "Result exceeds the control response limit")
            } else {
                json!({"id":invocation.id,"status":"rejected","data":{"error":"Result exceeds the control response limit"}})
            };
        }
        let accepted = response["status"] == "ok";
        self.cache_bytes += response.to_string().len();
        self.cache.push_back((invocation.id, response.clone()));
        while self.cache.len() > CACHE_LIMIT || self.cache_bytes > BODY_LIMIT {
            if let Some((_, value)) = self.cache.pop_front() {
                self.cache_bytes = self.cache_bytes.saturating_sub(value.to_string().len());
            }
        }
        self.responses.push_back(response);
        accepted
    }
}

pub(super) fn valid_server_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 254
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[derive(Clone)]
pub(super) struct Pairing {
    pub base: String,
    pub server_id: String,
    pub device_id: String,
    pub id: String,
    secret: String,
    pub code: String,
    pub expires_ms: u64,
    pub next_ms: u64,
    pub failures: u8,
}
pub(super) enum PairResult {
    Pending,
    Approved(Config),
}
impl Pairing {
    pub fn new(
        address: &str,
        server_id: &str,
        device_id: &str,
        now: u64,
    ) -> Result<Self, &'static str> {
        if !valid_server_id(server_id) || !valid_device(device_id)
        {
            return Err("Invalid pairing identity");
        }
        Ok(Self {
            base: normalize_base(address, false)?,
            server_id: server_id.into(),
            device_id: device_id.into(),
            id: String::new(),
            secret: String::new(),
            code: String::new(),
            expires_ms: now.saturating_add(600000),
            next_ms: now,
            failures: 0,
        })
    }
    pub fn request(&self, cancel: bool) -> Request {
        let poll = !self.id.is_empty();
        Request {
            method: if cancel {
                "DELETE"
            } else if poll {
                "GET"
            } else {
                "POST"
            },
            url: format!(
                "{}/api/pairings{}",
                self.base,
                if poll {
                    format!("?id={}", self.id)
                } else {
                    String::new()
                }
            ),
            token: self.secret.clone(),
            body: (!poll).then(|| {
                json!({"device_id":self.device_id,"server_id":self.server_id}).to_string()
            }),
        }
    }
    pub fn accept(
        &mut self,
        status: u16,
        value: &Value,
        now: u64,
    ) -> Result<PairResult, &'static str> {
        if now >= self.expires_ms {
            return Err("Pairing expired");
        }
        if self.id.is_empty() {
            let id = value["id"]
                .as_str()
                .filter(|v| request_id(v))
                .ok_or("Invalid pairing response")?;
            let secret = value["secret"]
                .as_str()
                .filter(|v| valid_token(v))
                .ok_or("Invalid pairing response")?;
            let code = value["code"]
                .as_str()
                .filter(|v| {
                    v.len() == 8
                        && v.bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                })
                .ok_or("Invalid pairing response")?;
            let expires = value["expires_in"]
                .as_u64()
                .filter(|s| *s > 0 && *s <= 600)
                .ok_or("Invalid pairing expiry")?;
            if status != 201 {
                return Err("Could not create pairing request");
            }
            self.id = id.into();
            self.secret = secret.into();
            self.code = code.into();
            self.expires_ms = now.saturating_add(expires * 1000);
            self.next_ms = now.saturating_add(2000);
            Ok(PairResult::Pending)
        } else if status == 202 && value["status"] == "pending" {
            self.next_ms = now.saturating_add(2000);
            self.failures = 0;
            Ok(PairResult::Pending)
        } else {
            if status != 200
                || value["status"] != "approved"
                || value["device_id"] != self.device_id
            {
                return Err("Pairing was rejected or expired");
            }
            let address = value["address"].as_str().ok_or("Invalid pairing address")?;
            let config = Config::parse(
                json!({"enabled":true,"address":address,"token":value["token"],"device_id":self.device_id}),
            )?;
            if config.address != self.base {
                return Err("Pairing server changed");
            }
            Ok(PairResult::Approved(config))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(commands: Value, requests: Value) -> Value {
        json!({"commands":commands,"requests":requests,"server_time":1000})
    }
    #[test]
    fn control_urls_do_not_smuggle_credentials_or_plaintext_to_public_servers() {
        assert_eq!(
            normalize_base("https://home.example/api/webhook/commands", false).unwrap(),
            "https://home.example:443"
        );
        for url in [
            "https://user:pass@home.example",
            "https://home.example?token=secret",
            "https://home.example/../api",
            "https://home.example/%2e%2e",
            "http://home.example",
            "http://8.8.8.8",
        ] {
            assert!(normalize_base(url, true).is_err(), "{url}");
        }
        assert!(normalize_base("http://127.0.0.1:8081", false).is_err());
        assert_eq!(
            normalize_base("http://127.0.0.1:8081", true).unwrap(),
            "http://127.0.0.1:8081"
        );
    }
    #[test]
    fn duplicate_commands_are_acked_without_reapplying_relative_actions() {
        let mut state = Delivery::default();
        let value = envelope(
            json!([{"id":"command-1","command":"volume_up","expires_at":1005}]),
            json!([]),
        );
        assert_eq!(state.poll(&value, 10, 20).unwrap().len(), 1);
        assert!(state.poll(&value, 20, 20).unwrap().is_empty());
        assert_eq!(state.acks.len(), 1);
        let (kind, _) = state.next(&Config::disabled("dev_test".into()));
        state.accepted(&kind, &json!({"status":"ok"})).unwrap();
        assert!(state.acks.is_empty());
    }
    #[test]
    fn expired_commands_are_not_executed_and_requests_cache_exact_receipt() {
        let mut state = Delivery::default();
        let id = "0123456789abcdef0123456789abcdef";
        let value = envelope(
            json!([{"id":"old","expires_at":1000}]),
            json!([{"id":id,"action":"playback","params":{"operation":"seek_relative","seconds":10},"expires_at":1020}]),
        );
        let mut calls = state.poll(&value, 500, 200).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].deadline_ms, 20300);
        state.complete(
            calls.remove(0),
            json!({"status":"ok","data":{"accepted":true}}),
        );
        let receipt = state.responses.back().unwrap().clone();
        assert!(state.poll(&value, 600, 200).unwrap().is_empty());
        assert_eq!(state.responses.back(), Some(&receipt));
    }
    #[test]
    fn duplicate_poll_waits_for_pending_worker_then_replays_its_result() {
        let mut state = Delivery::default();
        let id = "0123456789abcdef0123456789abcdef";
        let value = envelope(
            json!([]),
            json!([{"id":id,"action":"profile","params":{"number":1},"expires_at":1020}]),
        );
        let invocation = state.poll(&value, 500, 0).unwrap().remove(0);
        assert!(state.poll(&value, 1000, 0).unwrap().is_empty());
        assert!(state.responses.is_empty());
        assert_eq!(state.pending_requests.len(), 1);
        assert!(state.complete(invocation, json!({"status":"ok","data":{"saved":true}})));
        assert!(state.pending_requests.is_empty());
        let receipt = state.responses.pop_front().unwrap();
        assert!(state.poll(&value, 1500, 0).unwrap().is_empty());
        assert_eq!(state.responses.pop_front(), Some(receipt));
    }
    #[test]
    fn oversize_envelopes_reject_before_execution() {
        let mut state = Delivery::default();
        let rows: Vec<_> = (0..257).map(|n| json!({"id":format!("c{n}")})).collect();
        assert!(state.poll(&envelope(json!(rows), json!([])), 0, 0).is_err());
        assert!(state.seen_commands.is_empty());
    }
    #[test]
    fn malformed_request_never_consumes_the_commands_alongside_it() {
        let mut state = Delivery::default();
        let input = envelope(
            json!([{"id":"valid","command":"pause"}]),
            json!([{"id":"bad","action":"status"}]),
        );
        assert!(state.poll(&input, 0, 0).is_err());
        assert!(state.seen_commands.is_empty());
        assert!(state.acks.is_empty());
    }
    #[test]
    fn pairing_binds_approval_to_original_server_and_device() {
        let mut pair = Pairing::new("https://home.example", "home", "dev_test", 0).unwrap();
        assert!(matches!(pair.accept(201,&json!({"id":"0123456789abcdef0123456789abcdef","secret":"a".repeat(32),"code":"ABCD1234","expires_in":60}),0),Ok(PairResult::Pending)));
        assert!(pair.accept(200,&json!({"status":"approved","device_id":"dev_test","address":"https://foreign.example","token":"b".repeat(32)}),1000).is_err());
        assert!(pair.accept(200,&json!({"status":"approved","device_id":"dev_other","address":"https://home.example","token":"b".repeat(32)}),1000).is_err());
        assert!(matches!(pair.accept(200,&json!({"status":"approved","device_id":"dev_test","address":"https://home.example","token":"b".repeat(32)}),1000),Ok(PairResult::Approved(_))));
    }
}

#[cfg(test)]
mod items_ingress_tests {
    use super::*;
    #[test]
    fn expired_items_request_keeps_typed_common_fields_before_dispatch() {
        let mut delivery=Delivery::default();
        let params=json!({"runtime":"fixture","op":"list","subject":"global","cursor":null,"max_entries":100});
        let actions=delivery.poll(&json!({"commands":[],"server_time":1000,"requests":[{
            "id":"00000000000000000000000000000001","action":"favorites_global_items","params":params,"expires_at":999}]}),100,0).unwrap();
        assert!(actions.is_empty());let response=delivery.responses.front().unwrap();
        assert_eq!(response["data"],json!({"version":1,"runtime":"fixture","op":"list","subject":"global","error":"expired"}));
    }
    #[test]
    fn recovery_early_refusal_uses_its_own_error_vocabulary() {
        let params=json!({"runtime":"fixture","op":"inspect","subject":"global","cursor":null});
        let response=refused_request("00000000000000000000000000000001","favorites_global_recovery",&params,"cancelled","expired receipt");
        assert_eq!(response["data"],json!({"version":1,"runtime":"fixture","op":"inspect","subject":"global","error":"expired"}));
    }
    #[test]
    fn evicted_items_receipt_is_typed_without_redispatch() {
        let mut delivery=Delivery::default();let id="00000000000000000000000000000001";
        delivery.seen_requests.push_back(id.into());
        assert!(delivery.poll(&json!({"commands":[],"server_time":1000,"requests":[{
            "id":id,"action":"favorites_global_items","params":{"runtime":"fixture","op":"current","subject":"global"},"expires_at":1020}]}),100,0).unwrap().is_empty());
        assert_eq!(delivery.responses.front().unwrap()["data"]["error"],"cancelled");
    }
}

#[cfg(test)]
#[path = "protocol_plex_receipt_tests.rs"]
mod plex_receipt_tests;

#[cfg(test)]
#[path = "local_browser_ingress_tests.rs"]
mod local_browser_ingress_tests;

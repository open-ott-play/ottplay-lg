//! Explicit OTTPlay Plex profile import. Never converts a server token into an account token.
use serde_json::Value;
use std::net::IpAddr;

#[derive(Clone)]
pub(crate) struct Profile {
    pub address: String,
    pub token: String,
    pub server_id: String,
    pub name: String,
    pub allow_insecure_http: bool,
}

impl Profile {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > 1024 * 1024 { return Err("profile exceeds 1 MiB"); }
        let mut v: Value = serde_json::from_slice(bytes).map_err(|_| "invalid profile JSON")?;
        if let Some(config) = v.get("plexcfg").cloned() {
            v = if let Some(text) = config.as_str() {
                serde_json::from_str(text).map_err(|_| "invalid plexcfg JSON")?
            } else { config };
        }
        let text = |names: &[&str]| -> String {
            names.iter().find_map(|key| v.get(*key).and_then(Value::as_str))
                .unwrap_or("").trim().to_owned()
        };
        let address = text(&["baseUrl", "address", "server", "url"]);
        let token = text(&["token"]);
        let server_id = text(&["serverId", "machineIdentifier", "machineId"]);
        let mut name = text(&["name", "title"]);
        if name.is_empty() { name = "OTTPlay Plex".to_owned(); }
        let allow_insecure_http = v.get("allowInsecureHttp").and_then(Value::as_bool).unwrap_or(false);
        if token.is_empty() || token.len() > 1024 || token.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) {
            return Err("invalid Plex server token");
        }
        if server_id.len() > 256 || server_id.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("invalid Plex server identity");
        }
        if name.len() > 256 || name.chars().any(char::is_control) { return Err("invalid profile name"); }
        let address = normalize_address(&address, allow_insecure_http)?;
        if address.starts_with("http://") && server_id.is_empty() { return Err("HTTP import requires the expected serverId"); }
        Ok(Self { address, token, server_id, name, allow_insecure_http })
    }
}

fn normalize_address(value: &str, allow_http: bool) -> Result<String, &'static str> {
    let (scheme, rest, default_port) = if let Some(v) = value.strip_prefix("https://") { ("https", v, 443) }
        else if let Some(v) = value.strip_prefix("http://") {
            if !allow_http { return Err("HTTP import requires allowInsecureHttp: true"); }
            ("http", v, 80)
        } else { return Err("server must use HTTP or HTTPS"); };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        || rest.contains(['/', '?', '#', '@', '\\']) { return Err("use a server origin without credentials, path or query"); }
    let (host, port) = if let Some(v) = rest.strip_prefix('[') {
        let (host, tail) = v.split_once(']').ok_or("invalid IPv6 address")?;
        host.parse::<std::net::Ipv6Addr>().map_err(|_| "invalid IPv6 address")?;
        let port = if tail.is_empty() { default_port } else {
            tail.strip_prefix(':').ok_or("invalid server port")?.parse::<u16>().map_err(|_| "invalid server port")?
        };
        (format!("[{host}]"), port)
    } else {
        let (host, port) = match rest.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "invalid server port")?),
            None => (rest, default_port),
        };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') { return Err("invalid server host"); }
        (host.to_owned(), port)
    };
    if port == 0 { return Err("invalid server port"); }
    if scheme == "http" {
        let ip = host.trim_matches(['[', ']']).parse::<IpAddr>().map_err(|_| "HTTP import requires a numeric local IP address")?;
        let local = match ip {
            IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_unique_local() || ip.is_loopback() || ip.is_unicast_link_local(),
        };
        if !local { return Err("HTTP import is limited to local network addresses"); }
    }
    Ok(format!("{scheme}://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_current_config_and_explicit_contract() {
        let p = Profile::parse(br#"{"address":"https://plex.example/","token":"secret","serverId":"machine"}"#).unwrap();
        assert_eq!(p.address,"https://plex.example:443"); assert_eq!(p.server_id,"machine"); assert_eq!(p.token,"secret"); assert!(!p.allow_insecure_http);
        let p = Profile::parse(br#"{"plexcfg":"{\"baseUrl\":\"https://plex.example:32400\",\"token\":\"secret\"}"}"#).unwrap();
        assert_eq!(p.address,"https://plex.example:32400");
    }
    #[test]
    fn http_needs_explicit_consent_local_literal_and_identity() {
        for input in [
            br#"{"address":"http://127.0.0.1:32400","token":"secret","serverId":"one"}"#.as_slice(),
            br#"{"address":"http://127.0.0.1:32400","token":"secret","allowInsecureHttp":true}"#,
            br#"{"address":"http://plex.example:32400","token":"secret","serverId":"one","allowInsecureHttp":true}"#,
            br#"{"address":"http://192.0.2.1:32400","token":"secret","serverId":"one","allowInsecureHttp":true}"#,
        ] { assert!(Profile::parse(input).is_err()); }
        assert!(Profile::parse(br#"{"address":"http://127.0.0.1:32400","token":"secret","serverId":"one","allowInsecureHttp":true}"#).unwrap().allow_insecure_http);
    }
    #[test]
    fn rejects_paths_bad_ports_and_tokens_without_echoing_them() {
        for address in ["https://user:secret@host", "https://host/path", "https://host?token=secret", "https://host:0", "https://host:65536", "file:///secret"] {
            let input = serde_json::json!({"address":address,"token":"top-secret"});
            let result = Profile::parse(input.to_string().as_bytes());
            assert!(result.is_err()); assert!(!result.err().unwrap().contains("top-secret"));
        }
        assert!(Profile::parse(br#"{"address":"https://host","token":"a\nb"}"#).is_err());
    }
}

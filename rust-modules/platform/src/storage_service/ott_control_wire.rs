//! Closed helper-owned OTT control credentials. No arbitrary encryption or key selection.
use super::{ott, ErrorCode, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const KEY_NAME: &str = "ott.control.v2";
pub const ENVELOPE_BYTES: usize = ott::ENVELOPE_BYTES + 65536;
const CIPHER_BYTES: usize = 32768;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub enabled: bool,
    pub address: String,
    pub token: SecretString,
    pub device_id: String,
    #[serde(rename = "allowInsecureHttp")]
    pub allow_insecure_http: bool,
}
impl Config {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let valid_token = self.token.0.is_empty() || ((32..=256).contains(&self.token.0.len())
            && self.token.0.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)));
        let valid_address = self.address.is_empty()
            || valid_address(&self.address, self.allow_insecure_http);
        if self.version != 1 || !ott::valid_device(&self.device_id) || !valid_token
            || !valid_address || (self.enabled && (self.address.is_empty() || self.token.0.is_empty()))
        { return Err(ErrorCode::Invalid); }
        Ok(())
    }
}
// The app admits a normalized control base. Defense in depth keeps HTTP limited
// to an explicitly consented numeric local address, as in its control protocol.
fn valid_address(address: &str, allow_http: bool) -> bool {
    let (rest, http) = if let Some(rest) = address.strip_prefix("https://") { (rest, false) }
        else if let Some(rest) = address.strip_prefix("http://") { (rest, true) }
        else { return false; };
    if address.len() > 2048
        || !ott::public_address_with_limit(&format!("https://{rest}"), 2048 + usize::from(http))
    { return false; }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b)) {
        return false;
    }
    if !http { return true; }
    if !allow_http { return false; }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host)
    } else { authority.rsplit_once(':').map(|(host, _)| host) };
    match host.and_then(|host| host.parse::<std::net::IpAddr>().ok()) {
        Some(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        None => false,
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttControlConfig([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Absent {},
    Legacy { sha256: String },
    Protected { sha256: String },
}
impl Source {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        match self {
            Self::Absent {} => Ok(()),
            Self::Legacy { sha256 } | Self::Protected { sha256 } if valid_digest(sha256) => Ok(()),
            _ => Err(ErrorCode::Invalid),
        }
    }
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Loaded {
    Missing {},
    Legacy { envelope: SecretString },
    Protected { value: Config, envelope_sha256: String },
}
impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttControlLoaded([REDACTED])")
    }
}

/// The original app-owned v1 bytes survive migration and later updates unchanged.
/// These fields are ciphertext/metadata; the typed Config is never persisted here in cleartext.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u8,
    pub install: String,
    pub namespace: String,
    pub cipher: SecretString,
    pub legacy: Option<SecretString>,
}
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttControlEnvelope([REDACTED])")
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cipher {
    format: String,
    key_name: String,
    iv: String,
    ciphertext: String,
    context: Value,
}
impl Envelope {
    pub fn context(&self) -> Value {
        let legacy_sha256 = self.legacy.as_ref()
            .map(|value| super::super::state::digest_bytes(value.0.as_bytes()));
        json!({"format":"ott-control", "version":2, "install":self.install,
            "namespace":"control", "legacy_sha256":legacy_sha256})
    }
    pub fn parse(text: &str, expected_install: &str) -> Result<Self, ErrorCode> {
        if text.len() > ENVELOPE_BYTES { return Err(ErrorCode::Invalid); }
        let value: Self = serde_json::from_str(text).map_err(|_| ErrorCode::Invalid)?;
        value.validate(expected_install)?;
        Ok(value)
    }
    pub fn validate(&self, expected_install: &str) -> Result<(), ErrorCode> {
        if self.version != 2 || self.install != expected_install || self.install.is_empty()
            || self.namespace != "control" || self.cipher.0.len() > CIPHER_BYTES
        { return Err(ErrorCode::Invalid); }
        if let Some(legacy) = &self.legacy {
            if ott::envelope_app(&legacy.0, ott::Namespace::Control)? != expected_install {
                return Err(ErrorCode::Invalid);
            }
        }
        let cipher: Cipher = serde_json::from_str(&self.cipher.0).map_err(|_| ErrorCode::Invalid)?;
        if cipher.format != "keymanager3-aes256-gcm-v1" || cipher.key_name != KEY_NAME
            || !ott::base64_shape(&cipher.iv) || cipher.iv.len() > 256
            || !ott::base64_shape(&cipher.ciphertext) || cipher.context != self.context()
        { return Err(ErrorCode::Invalid); }
        Ok(())
    }
    pub fn encode(&self) -> Result<String, ErrorCode> {
        self.validate(&self.install)?;
        let text = serde_json::to_string(self).map_err(|_| ErrorCode::Invalid)?;
        if text.len() > ENVELOPE_BYTES { return Err(ErrorCode::Invalid); }
        Ok(text)
    }
}
/// Detection is not validation. Unknown/malformed data must subsequently fail closed.
pub fn is_protected(text: &str) -> bool {
    serde_json::from_str::<Value>(text).ok()
        .is_some_and(|value| value.get("version").and_then(Value::as_u64) == Some(2))
}
pub fn validate_record(text: &str, expected_install: &str) -> Result<(), ErrorCode> {
    if is_protected(text) { Envelope::parse(text, expected_install).map(|_| ()) }
    else if ott::envelope_app(text, ott::Namespace::Control)? == expected_install { Ok(()) }
    else { Err(ErrorCode::Invalid) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_empty_variants_reject_extra_fields() {
        assert!(serde_json::from_value::<Source>(json!({"type":"absent","token":"x"})).is_err());
        assert!(serde_json::from_value::<Loaded>(json!({"type":"missing","payload":"x"})).is_err());
    }
    #[test]
    fn config_is_bounded_and_never_displays_secret() {
        let config = Config { version:1, enabled:true, address:"https://home.example:443/control".into(),
            token:SecretString("a".repeat(32)), device_id:"dev_example".into(), allow_insecure_http:false };
        assert!(config.validate().is_ok());
        assert_eq!(format!("{config:?}"), "OttControlConfig([REDACTED])");
        let mut unsafe_config = config.clone(); unsafe_config.address = "https://user:pass@host:443".into();
        assert!(unsafe_config.validate().is_err());
        unsafe_config = config.clone(); unsafe_config.address = "http://host:80".into();
        assert!(unsafe_config.validate().is_err());
        unsafe_config.allow_insecure_http = true;
        assert!(unsafe_config.validate().is_err());
        unsafe_config.address = "http://192.168.1.4:80".into();
        assert!(unsafe_config.validate().is_ok());
        unsafe_config.address = "https://home.example:443/path:bad".into();
        assert!(unsafe_config.validate().is_err());
        let mut value = serde_json::to_value(config).unwrap(); value["arbitrary"] = json!("payload");
        assert!(serde_json::from_value::<Config>(value).is_err());
    }

    #[test]
    fn http_limit_is_applied_to_the_original_address() {
        let mut address = "http://192.168.1.4:80/".to_string();
        address.push_str(&"a".repeat(2048 - address.len()));
        assert!(valid_address(&address, true));
        address.push('a');
        assert!(!valid_address(&address, true));
    }
}

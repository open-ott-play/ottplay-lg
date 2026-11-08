//! Closed helper-owned Saved document; no arbitrary key or namespace selection.
use super::{ott, ErrorCode, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const KEY_NAME: &str = "ott.state.v2";
// Saved stays at its existing 1 MiB logical limit. The encryption primitive wraps its
// JSON text in another JSON string, then base64: permit that expansion, not 32 KiB.
pub const CIPHER_BYTES: usize = 3 * ott::LOGICAL_BYTES + 65536;
// Includes retained legacy ciphertext; the complete DB8 request is independently
// checked against FRAME_BYTES before put. No payload is truncated to fit.
pub const ENVELOPE_BYTES: usize = ott::FRAME_BYTES;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Document(Value);
impl TryFrom<Value> for Document {
    type Error = ErrorCode;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let document = Self(value);
        document.validate()?;
        Ok(document)
    }
}
impl AsRef<Value> for Document {
    fn as_ref(&self) -> &Value { &self.0 }
}
impl From<Document> for Value {
    fn from(document: Document) -> Self { document.0 }
}
impl Document {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let object = self.0.as_object().ok_or(ErrorCode::Invalid)?;
        if serde_json::to_vec(&self.0).map_err(|_| ErrorCode::Invalid)?.len() > ott::LOGICAL_BYTES {
            return Err(ErrorCode::Invalid);
        }
        // Matches Saved's defaulted top-level schema. The app retains ownership of
        // provider, policy, receipts and Favorites semantics and validates them too.
        for (key, value) in object {
            let valid = match key.as_str() {
                "version" => value.as_u64() == Some(1),
                "profiles" | "favorites" | "favorite_items" | "reminders"
                    | "local_favorite_proofs" | "settings" | "resume" => value.is_object(),
                "category" => value.as_str().is_some_and(|text| text.len() <= 4096),
                "favorites_selected" => value.is_boolean(),
                "receipts" => value.as_array().is_some_and(|rows| rows.len() <= 32),
                _ => false,
            };
            if !valid { return Err(ErrorCode::Invalid); }
        }
        Ok(())
    }
}
impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttStateDocument([REDACTED])")
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
    Protected { value: Document, envelope_sha256: String },
}
impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttStateLoaded([REDACTED])")
    }
}

/// The original app-owned v1 bytes survive migration and later updates unchanged.
/// These fields are ciphertext/metadata; the typed Document is never persisted here in cleartext.
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
        f.write_str("OttStateEnvelope([REDACTED])")
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
        json!({"format":"ott-state", "version":2, "install":self.install,
            "namespace":"state", "legacy_sha256":legacy_sha256})
    }
    pub fn parse(text: &str, expected_install: &str) -> Result<Self, ErrorCode> {
        if text.len() > ENVELOPE_BYTES { return Err(ErrorCode::Invalid); }
        let value: Self = serde_json::from_str(text).map_err(|_| ErrorCode::Invalid)?;
        value.validate(expected_install)?;
        Ok(value)
    }
    pub fn validate(&self, expected_install: &str) -> Result<(), ErrorCode> {
        if self.version != 2 || self.install != expected_install || self.install.is_empty()
            || self.namespace != "state" || self.cipher.0.len() > CIPHER_BYTES
        { return Err(ErrorCode::Invalid); }
        if let Some(legacy) = &self.legacy {
            if ott::envelope_app(&legacy.0, ott::Namespace::State)? != expected_install {
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
    else if ott::envelope_app(text, ott::Namespace::State)? == expected_install { Ok(()) }
    else { Err(ErrorCode::Invalid) }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_keeps_exact_defaulted_shape_and_redacts_debug() {
        let value = json!({"settings":{"token":"private-fixture"},"category":"Favorites"});
        let document = Document::try_from(value.clone()).unwrap();
        assert_eq!(document.as_ref(), &value);
        assert_eq!(serde_json::to_value(&document).unwrap(), value);
        assert_eq!(format!("{document:?}"), "OttStateDocument([REDACTED])");
        assert_eq!(Value::from(document), value);
        assert!(Document::try_from(json!({})).is_ok());
    }
    #[test]
    fn unknown_fields_wrong_types_and_explicit_null_metadata_are_rejected() {
        for value in [json!({"namespace":"control"}), json!({"version":2}),
            json!({"version":null}),json!({"version":1.0}), json!({"favorite_items":null}),
            json!({"settings":[]}), json!({"profiles":[]}), json!({"resume":[]}),
            json!({"receipts":{}}), json!({"category":0}), json!({"favorites_selected":"true"})] {
            assert!(Document::try_from(value).is_err());
        }
        assert!(serde_json::from_value::<Source>(json!({"type":"absent","extra":1})).is_err());
        assert!(serde_json::from_value::<Loaded>(json!({"type":"missing","extra":1})).is_err());
    }
    #[test]
    fn saved_logical_limit_is_one_mib_not_control_cipher_limit() {
        let base = json!({"settings":{"padding":""}});
        let overhead = serde_json::to_vec(&base).unwrap().len();
        let value = json!({"settings":{"padding":"x".repeat(ott::LOGICAL_BYTES-overhead)}});
        assert_eq!(serde_json::to_vec(&value).unwrap().len(), ott::LOGICAL_BYTES);
        assert!(Document::try_from(value).is_ok());
        assert!(Document::try_from(json!({"settings":{"padding":"x".repeat(ott::LOGICAL_BYTES-overhead+1)}})).is_err());
    }
    #[test]
    fn expanded_cipher_fits_checked_envelope_budget() {
        let overhead = serde_json::to_vec(&json!({"settings":{"padding":""}})).unwrap().len();
        let value = json!({"settings":{"padding":"\\".repeat((ott::LOGICAL_BYTES-overhead)/2)}});
        let document = Document::try_from(value).unwrap();
        let plain = serde_json::to_string(&document).unwrap();
        let mut envelope = Envelope { version:2, install:"com.openottplay.player.native".into(),
            namespace:"state".into(), cipher:SecretString(String::new()), legacy:None };
        let context = envelope.context();
        let inner = serde_json::to_vec(&json!({"context":context,"payload":plain})).unwrap();
        // GCM adds an authentication tag; envelope cipher contains base64, not raw JSON.
        let ciphertext = "A".repeat(4*((inner.len()+16).div_ceil(3)));
        envelope.cipher.0 = serde_json::to_string(&json!({"format":"keymanager3-aes256-gcm-v1",
            "key_name":KEY_NAME,"iv":"AAAAAAAAAAAAAAAA","ciphertext":ciphertext,"context":context})).unwrap();
        assert!(envelope.cipher.0.len() > 32768);
        assert!(envelope.cipher.0.len() <= CIPHER_BYTES);
        assert!(envelope.encode().is_ok());
    }
}

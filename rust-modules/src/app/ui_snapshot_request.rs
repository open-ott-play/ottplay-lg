//! Closed, path-free launch request for one UI snapshot.
//!
//! This module only interprets supplied bytes. The caller owns launch routing, the
//! one-shot capture state and the fixed diagnostics-export destination. In particular,
//! a rejected snapshot payload must never be reoffered as an ordinary player action.
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;

/// Protocol input bound. Ordinary launch payloads are not discarded at this limit.
const REQUEST_BYTES: usize = 16 * 1024;
/// Mirrors the existing platform launch queue's admission limit.
const REQUEST_BATCH: usize = 8;

/// Opaque correlation only; it cannot contain an output path or any credentials.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestId([u8; 32]);

impl RequestId {
    fn parse(value: &str) -> Option<Self> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let mut bytes = [0; 32];
        bytes.copy_from_slice(value.as_bytes());
        Some(Self(bytes))
    }

    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated ASCII request id")
    }
}

impl fmt::Debug for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RequestId([redacted])")
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) request_id: RequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reject {
    TooLarge,
    InvalidSchema,
    WrongEvent,
    InvalidVersion,
    InvalidRequestId,
    MultipleRequests,
    ConflictingDiagnostic,
    TooManyPayloads,
}

impl Reject {
    /// Static diagnostics only: never include malformed JSON, unknown keys or values.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "ui_snapshot_request_too_large",
            Self::InvalidSchema => "ui_snapshot_request_invalid_schema",
            Self::WrongEvent => "ui_snapshot_request_wrong_event",
            Self::InvalidVersion => "ui_snapshot_request_invalid_version",
            Self::InvalidRequestId => "ui_snapshot_request_invalid_id",
            Self::MultipleRequests => "ui_snapshot_request_multiple",
            Self::ConflictingDiagnostic => "ui_snapshot_request_conflicting_diagnostic",
            Self::TooManyPayloads => "ui_snapshot_request_batch_limit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    Ordinary,
    Rejected(Reject),
    Requested(Request),
}

// SAM owns the outer metadata schema. Only event and parameters are interpreted;
// serde rejects duplicate occurrences of these known fields. Unknown SAM metadata
// is ignored, not deserialized to a retained serde_json::Value tree.
#[derive(Deserialize)]
struct Envelope {
    event: String,
    parameters: Parameters,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    #[serde(rename = "ottUiSnapshot")]
    snapshot: WireRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    version: u32,
    request_id: String,
}

#[derive(Clone, Copy)]
enum Key {
    Parameters,
    Snapshot,
    StorageProbe,
    Other,
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = Key;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object key")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Key, E> {
                Ok(match value {
                    "parameters" => Key::Parameters,
                    "ottUiSnapshot" => Key::Snapshot,
                    "ottStorageProbe" => Key::StorageProbe,
                    _ => Key::Other,
                })
            }
        }
        deserializer.deserialize_str(KeyVisitor)
    }
}

#[derive(Default)]
struct OwnedKeys {
    snapshot: bool,
    storage_probe: bool,
}

struct Probe<'a> {
    keys: &'a mut OwnedKeys,
    parameters: bool,
}

impl<'de> DeserializeSeed<'de> for Probe<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Probe<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("launch metadata")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<Key>()? {
            match (self.parameters, key) {
                (false, Key::Parameters) => {
                    map.next_value_seed(Probe {
                        keys: self.keys,
                        parameters: true,
                    })?;
                }
                (true, Key::Snapshot) => {
                    // Record ownership before reading the value, including malformed values.
                    self.keys.snapshot = true;
                    map.next_value::<IgnoredAny>()?;
                }
                (true, Key::StorageProbe) => {
                    self.keys.storage_probe = true;
                    map.next_value::<IgnoredAny>()?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}

fn owned_keys(raw: &str) -> OwnedKeys {
    let mut keys = OwnedKeys::default();
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    // A streaming ownership probe skips unknown values and retains no raw strings.
    // serde's ordinary recursion bound remains enabled. A malformed tail cannot
    // erase ownership already observed. Escaped key names are decoded by serde.
    let _ = Probe {
        keys: &mut keys,
        parameters: false,
    }
    .deserialize(&mut deserializer);
    keys
}

fn parse_owned(raw: &str, keys: &OwnedKeys) -> Launch {
    if !keys.snapshot {
        return Launch::Ordinary;
    }
    if raw.len() > REQUEST_BYTES {
        return Launch::Rejected(Reject::TooLarge);
    }
    if keys.storage_probe {
        return Launch::Rejected(Reject::ConflictingDiagnostic);
    }
    let Ok(envelope) = serde_json::from_str::<Envelope>(raw) else {
        return Launch::Rejected(Reject::InvalidSchema);
    };
    if envelope.event != "launch" && envelope.event != "relaunch" {
        return Launch::Rejected(Reject::WrongEvent);
    }
    let request = envelope.parameters.snapshot;
    if request.version != 1 {
        return Launch::Rejected(Reject::InvalidVersion);
    }
    match RequestId::parse(&request.request_id) {
        Some(request_id) => Launch::Requested(Request { request_id }),
        None => Launch::Rejected(Reject::InvalidRequestId),
    }
}

pub(crate) fn parse_launch(raw: &str) -> Launch {
    parse_owned(raw, &owned_keys(raw))
}

/// Select at most one snapshot without consuming or rewriting any launch bytes.
///
/// The caller must check this before the storage-canary selector. On a request or
/// rejection, reoffer ONLY payloads individually classified as Ordinary, and do
/// do not run the storage-canary selector on the original batch after any
/// non-Ordinary result. ConflictingDiagnostic rejects both explicit requests.
pub(crate) fn select_launches(payloads: &[String]) -> Launch {
    let mut selected = None;
    let mut rejected = None;
    let mut any_snapshot = false;
    let mut any_storage_probe = false;
    for raw in payloads {
        let keys = owned_keys(raw);
        any_snapshot |= keys.snapshot;
        any_storage_probe |= keys.storage_probe;
        match parse_owned(raw, &keys) {
            Launch::Ordinary => {}
            Launch::Rejected(reason) => {
                rejected.get_or_insert(reason);
            }
            Launch::Requested(request) => {
                if selected.is_some() {
                    rejected.get_or_insert(Reject::MultipleRequests);
                }
                selected = Some(request);
            }
        }
    }
    if !any_snapshot {
        return Launch::Ordinary;
    }
    if any_storage_probe {
        return Launch::Rejected(Reject::ConflictingDiagnostic);
    }
    if payloads.len() > REQUEST_BATCH {
        return Launch::Rejected(Reject::TooManyPayloads);
    }
    if let Some(reason) = rejected {
        return Launch::Rejected(reason);
    }
    selected.map_or(Launch::Ordinary, Launch::Requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "0123456789abcdef0123456789abcdef";
    const VALID: &str = r#"{"event":"launch","parameters":{"ottUiSnapshot":{"version":1,"request_id":"0123456789abcdef0123456789abcdef"}}}"#;

    #[test]
    fn accepts_cold_and_warm_explicit_requests() {
        for raw in [VALID.to_owned(), VALID.replace("launch", "relaunch")] {
            let Launch::Requested(request) = parse_launch(&raw) else {
                panic!("valid request rejected");
            };
            assert_eq!(request.request_id.as_str(), ID);
            assert_eq!(
                serde_json::to_string(&request.request_id).unwrap(),
                format!("\"{ID}\"")
            );
        }
    }

    #[test]
    fn leaves_ordinary_payloads_and_values_named_like_request_untouched() {
        for raw in [
            "{}",
            "not json",
            r#"{"parameters":{"plex":{"ratingKey":"1"}}}"#,
            r#"{"reason":"ottUiSnapshot","parameters":{"plex":{"ottUiSnapshot":true}}}"#,
            r#"{"ottUiSnapshot":true,"parameters":{}}"#,
        ] {
            assert_eq!(parse_launch(raw), Launch::Ordinary);
        }
        let raw = format!(
            r#"{{"reason":"{}","parameters":{{"plex":{{"ratingKey":"1"}}}}}}"#,
            "x".repeat(REQUEST_BYTES)
        );
        assert_eq!(parse_launch(&raw), Launch::Ordinary);
        let payloads = vec![raw; REQUEST_BATCH + 1];
        let before = payloads.clone();
        assert_eq!(select_launches(&payloads), Launch::Ordinary);
        assert_eq!(payloads, before);
    }

    #[test]
    fn permits_sam_metadata_without_retaining_it() {
        let raw = VALID.replacen('{', r#"{"appId":"com.example.app","interfaceVersion":2,"reason":"private metadata","metadata":{"nested":[1,true,null]},"#, 1);
        assert!(matches!(parse_launch(&raw), Launch::Requested(_)));
        assert!(!format!("{:?}", parse_launch(&raw)).contains("private metadata"));
    }

    #[test]
    fn request_id_is_exact_lowercase_ascii_hex() {
        for id in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789ABCDEF",
            "../../private/path",
            "0123456789abcdef0123456789abcdeg",
            "0123456789abcdef0123456789abcde ",
            "кириллица",
        ] {
            assert_eq!(
                parse_launch(&VALID.replace(ID, id)),
                Launch::Rejected(Reject::InvalidRequestId)
            );
        }
        assert!(!format!("{:?}", RequestId::parse(ID).unwrap()).contains(ID));
    }

    #[test]
    fn schema_rejects_extra_paths_options_and_mixed_player_actions() {
        for raw in [
            VALID.replace("\"version\":1", "\"path\":\"/tmp/private\",\"version\":1"),
            VALID.replace("\"version\":1", "\"timeout_ms\":1000,\"version\":1"),
            VALID.replace(
                "\"ottUiSnapshot\":",
                "\"plex\":{\"ratingKey\":\"1\"},\"ottUiSnapshot\":",
            ),
            VALID.replace("\"version\":1", "\"version\":1.0"),
            VALID.replace("\"event\":\"launch\",", ""),
            VALID.replace("\"request_id\":", "\"unknown\":"),
            r#"{"event":"launch","parameters":{"ottUiSnapshot":true}}"#.into(),
        ] {
            assert_eq!(parse_launch(&raw), Launch::Rejected(Reject::InvalidSchema));
        }
        assert_eq!(
            parse_launch(&VALID.replace("\"version\":1", "\"version\":2")),
            Launch::Rejected(Reject::InvalidVersion)
        );
        assert_eq!(
            parse_launch(&VALID.replace("launch", "close")),
            Launch::Rejected(Reject::WrongEvent)
        );
    }

    #[test]
    fn duplicate_known_keys_and_trailing_json_are_rejected() {
        for raw in [
            VALID.replace("\"event\":", "\"event\":\"launch\",\"event\":"),
            VALID.replace("\"parameters\":", "\"parameters\":{},\"parameters\":"),
            VALID.replace("\"version\":1", "\"version\":1,\"version\":1"),
            VALID.replace(
                "\"request_id\":",
                &format!("\"request_id\":\"{ID}\",\"request_id\":"),
            ),
            VALID.replace(
                "\"ottUiSnapshot\":",
                "\"ottUiSnapshot\":{},\"ottUiSnapshot\":",
            ),
            format!("{VALID} {{}}"),
        ] {
            assert_eq!(parse_launch(&raw), Launch::Rejected(Reject::InvalidSchema));
        }
    }

    #[test]
    fn escaped_owned_keys_cannot_bypass_request_checks() {
        let raw = VALID
            .replace("parameters", "parame\\u0074ers")
            .replace("ottUiSnapshot", "ott\\u0055iSnapshot");
        assert!(matches!(parse_launch(&raw), Launch::Requested(_)));
        let duplicate = VALID.replace("\"version\":1", "\"version\":1,\"ver\\u0073ion\":1");
        assert_eq!(
            parse_launch(&duplicate),
            Launch::Rejected(Reject::InvalidSchema)
        );
    }

    #[test]
    fn oversized_explicit_and_late_escaped_requests_fail_closed() {
        let prefix = format!(r#"{{"reason":"{}","#, "x".repeat(REQUEST_BYTES));
        for request in [
            VALID.to_owned(),
            VALID.replace("ottUiSnapshot", "ott\\u0055iSnapshot"),
        ] {
            let raw = prefix.clone() + &request[1..];
            assert_eq!(parse_launch(&raw), Launch::Rejected(Reject::TooLarge));
        }
        let exact = format!("{VALID}{}", " ".repeat(REQUEST_BYTES - VALID.len()));
        assert!(matches!(parse_launch(&exact), Launch::Requested(_)));
        assert_eq!(
            parse_launch(&(exact + " ")),
            Launch::Rejected(Reject::TooLarge)
        );
    }

    #[test]
    fn ownership_survives_malformed_owned_value_and_tail() {
        for raw in [
            r#"{"event":"launch","parameters":{"ottUiSnapshot":"#,
            r#"{"event":"launch","parameters":{"ottUiSnapshot":{}}"#,
        ] {
            assert_eq!(parse_launch(raw), Launch::Rejected(Reject::InvalidSchema));
        }
    }

    #[test]
    fn batch_rejects_duplicates_and_preserves_non_snapshot_actions() {
        let payloads = vec![
            "{}".into(),
            VALID.into(),
            r#"{"parameters":{"plex":{"ratingKey":"1"}}}"#.into(),
        ];
        let before = payloads.clone();
        assert!(matches!(select_launches(&payloads), Launch::Requested(_)));
        assert_eq!(payloads, before);
        assert_eq!(
            select_launches(&[VALID.into(), VALID.into()]),
            Launch::Rejected(Reject::MultipleRequests)
        );
        let different = VALID.replace(ID, "abcdef0123456789abcdef0123456789");
        assert_eq!(
            select_launches(&[VALID.into(), different]),
            Launch::Rejected(Reject::MultipleRequests)
        );
        let mut excessive = vec!["{}".into(); REQUEST_BATCH];
        excessive.push(VALID.into());
        assert_eq!(
            select_launches(&excessive),
            Launch::Rejected(Reject::TooManyPayloads)
        );
    }

    #[test]
    fn storage_canary_never_combines_with_snapshot_even_in_separate_payloads() {
        let storage = VALID.replace("ottUiSnapshot", "ottStorageProbe");
        assert_eq!(parse_launch(&storage), Launch::Ordinary);
        assert_eq!(
            select_launches(&[storage.clone(), VALID.into()]),
            Launch::Rejected(Reject::ConflictingDiagnostic)
        );
        assert_eq!(
            select_launches(&[VALID.into(), storage]),
            Launch::Rejected(Reject::ConflictingDiagnostic)
        );
        let combined = VALID.replace(
            "\"ottUiSnapshot\":",
            "\"ottStorageProbe\":{},\"ottUiSnapshot\":",
        );
        assert_eq!(
            parse_launch(&combined),
            Launch::Rejected(Reject::ConflictingDiagnostic)
        );
    }

    #[test]
    fn refusal_metadata_is_fixed_and_has_no_payload_echo() {
        let raw = VALID.replace(
            "\"version\":1",
            "\"private_unknown\":\"sensitive-value\",\"version\":1",
        );
        let Launch::Rejected(reason) = parse_launch(&raw) else {
            panic!("expected rejection");
        };
        assert_eq!(reason.code(), "ui_snapshot_request_invalid_schema");
        assert!(!format!("{reason:?}").contains("sensitive-value"));
        assert!(!reason.code().contains("private_unknown"));
    }
}

//! Owner-bound keymanager3 AES-256-GCM. Never export keys or choose an implicit fallback.
//! ParamSet/handle contract: LG Keymanager3 reference and exact 43.21.71 keymanager3
//! decompilation (keymanager-params/semantics/binding in the DB8 crypto evidence lab).
use super::wire::{
    CanaryCleanup, CanaryResult, ErrorCode, KeymanagerFailure, KeymanagerFailureCategory,
    KeymanagerOperation, KeymanagerStage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub trait Rpc {
    fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode>;
}
/// Retain only a closed stage and a bounded numeric service code. In particular, never retain
/// an LS2 reply/error string (services can echo keys and input data in their diagnostics).
struct DiagnosticRpc<'a, R> {
    inner: &'a mut R,
    stage: KeymanagerStage,
    service_code: Option<i32>,
    rejected: bool,
    failed: bool,
}
impl<R: Rpc> Rpc for DiagnosticRpc<'_, R> {
    fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode> {
        let stage = if uri.ends_with("/generateKey") {
            Some(KeymanagerStage::Generate)
        } else if uri.ends_with("/begin") {
            Some(KeymanagerStage::Begin)
        } else if uri.ends_with("/finish") {
            Some(KeymanagerStage::Finish)
        } else {
            None
        };
        let result = self.inner.call(uri, payload);
        if let Some(stage) = stage.filter(|_| !self.failed) {
            self.stage = stage;
            if let Ok(reply) = &result {
                self.rejected = reply["returnValue"] == false;
                self.service_code = if self.rejected {
                    reply["errorCode"]
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                } else {
                    None
                };
            }
            self.failed = result.is_err() || self.rejected;
        }
        result
    }
}
pub fn failure(
    operation: KeymanagerOperation,
    stage: KeymanagerStage,
    code: ErrorCode,
) -> KeymanagerFailure {
    KeymanagerFailure {
        operation,
        stage,
        code,
        category: match code {
            ErrorCode::Unavailable => KeymanagerFailureCategory::Unavailable,
            ErrorCode::Timeout => KeymanagerFailureCategory::Timeout,
            ErrorCode::Corrupt => KeymanagerFailureCategory::InvalidResponse,
            _ => KeymanagerFailureCategory::Other,
        },
        service_code: None,
        hub_error: None,
    }
}
fn diagnose<R: Rpc, T>(
    bus: &mut R,
    operation: KeymanagerOperation,
    action: impl FnOnce(&mut DiagnosticRpc<'_, R>) -> Result<T, ErrorCode>,
) -> Result<T, KeymanagerFailure> {
    let mut traced = DiagnosticRpc {
        inner: bus,
        stage: KeymanagerStage::Validate,
        service_code: None,
        rejected: false,
        failed: false,
    };
    action(&mut traced).map_err(|code| {
        let mut evidence = failure(operation, traced.stage, code);
        evidence.service_code = traced.service_code;
        if traced.rejected {
            evidence.category = KeymanagerFailureCategory::ServiceRejected;
        }
        evidence
    })
}
pub fn seal_detailed(
    bus: &mut impl Rpc,
    name: &str,
    plaintext: &str,
    context: Value,
) -> Result<String, KeymanagerFailure> {
    diagnose(bus, KeymanagerOperation::Seal, |bus| {
        seal(bus, name, plaintext, context)
    })
}
pub fn unseal_detailed(
    bus: &mut impl Rpc,
    name: &str,
    encoded: &str,
    context: &Value,
    operation: KeymanagerOperation,
) -> Result<String, KeymanagerFailure> {
    diagnose(bus, operation, |bus| unseal(bus, name, encoded, context))
}

// Fixed helper-owned keys never enter Plex generation-key retirement/cleanup.
const CONTROL_KEY: &str = super::wire::control::KEY_NAME;
const SAVED_KEY: &str = super::wire::saved::KEY_NAME;
pub fn seal_control_detailed(bus: &mut impl Rpc, plaintext: &str, context: Value)
    -> Result<String, KeymanagerFailure> {
    seal_owned_detailed(bus, CONTROL_KEY, plaintext, context)
}
pub fn seal_saved_detailed(bus: &mut impl Rpc, plaintext: &str, context: Value)
    -> Result<String, KeymanagerFailure> {
    seal_owned_detailed(bus, SAVED_KEY, plaintext, context)
}
// Private primitive: callers can only select one of the fixed typed wrappers above.
fn seal_owned_detailed(bus: &mut impl Rpc, name: &str, plaintext: &str, context: Value)
    -> Result<String, KeymanagerFailure> {
    diagnose(bus, KeymanagerOperation::Seal, |bus| {
        let generated = bus.call(
            &format!("{URI}/generateKey"),
            &json!({"name":name,"params":generate_params()}),
        )?;
        if !(generated["returnValue"] == false && generated["errorCode"] == -10002) {
            checked(generated)?;
        }
        Ok(())
    })?;
    // An accepted existing-key reply is not the failure of a later encryption RPC.
    diagnose(bus, KeymanagerOperation::Seal, |bus| {
        seal_created(bus, name, plaintext, context)
    })
}
pub fn open_control_detailed(bus: &mut impl Rpc, encoded: &str, context: &Value)
    -> Result<String, KeymanagerFailure> {
    open_owned_detailed(bus, CONTROL_KEY, encoded, context)
}
pub fn open_saved_detailed(bus: &mut impl Rpc, encoded: &str, context: &Value)
    -> Result<String, KeymanagerFailure> {
    open_owned_detailed(bus, SAVED_KEY, encoded, context)
}
fn open_owned_detailed(bus: &mut impl Rpc, name: &str, encoded: &str, context: &Value)
    -> Result<String, KeymanagerFailure> {
    diagnose(bus, KeymanagerOperation::Readback, |bus| {
        unseal_with_name(bus, name, encoded, context)
    })
}

const CANARY: &str = "ott-keymanager-canary-v1";
fn canary_context() -> Value {
    json!({"purpose":"ott-keymanager-canary","version":1})
}

/// Explicit diagnostic only; not a capability check or a credential-storage operation.
/// The caller must keep the named helper bus and impose an overall RPC deadline with a
/// cleanup reserve. No retry, DB8 access, logging, or persistent probe state occurs here.
/// A lost generate reply may leave an unconfirmed probe key; deleting it would be unsafe.
pub fn probe_owned_key(bus: &mut impl Rpc) -> CanaryResult {
    probe_owned_key_with(bus, || {
        super::state::Generation::random().map_err(|_| ErrorCode::Unavailable)
    })
}
fn probe_owned_key_with(
    bus: &mut impl Rpc,
    generation: impl FnOnce() -> Result<super::state::Generation, ErrorCode>,
) -> CanaryResult {
    let name = match generation() {
        Ok(generation) => generation.key_name(),
        Err(code) => return CanaryResult {
            operation: Err(failure(KeymanagerOperation::Seal, KeymanagerStage::Validate, code)),
            cleanup: CanaryCleanup::NotCreated {},
        },
    };
    // Names come only from fresh helper entropy, never from an RPC request or saved state.
    let mut cleanup = CanaryCleanup::CreationUnconfirmed {};
    let generated = diagnose(bus, KeymanagerOperation::Seal, |bus| {
        let reply = bus.call(
            &format!("{URI}/generateKey"),
            &json!({"name":name,"params":generate_params()}),
        )?;
        if reply["returnValue"] == false {
            cleanup = CanaryCleanup::NotCreated {};
        }
        checked(reply).map(|_| ())
    });
    if let Err(error) = generated {
        return CanaryResult { operation: Err(error), cleanup };
    }

    let context = canary_context();
    let operation = diagnose(bus, KeymanagerOperation::Seal, |bus| {
        // Unlike seal(), this primitive does not remove the key on failure.
        seal_created(bus, &name, CANARY, context.clone())
    }).and_then(|encoded| {
        unseal_detailed(bus, &name, &encoded, &context, KeymanagerOperation::Readback)
    }).and_then(|opened| {
        if opened == CANARY {
            Ok(())
        } else {
            Err(failure(KeymanagerOperation::Readback, KeymanagerStage::Roundtrip, ErrorCode::Corrupt))
        }
    });
    // Exactly one removal attempt, and only after this invocation confirmed generation.
    let cleanup = match bus.call(&format!("{URI}/removeKey"), &json!({"name":name})) {
        Ok(reply) if reply["returnValue"] == true => CanaryCleanup::Removed {},
        Ok(reply) if reply["returnValue"] == false => CanaryCleanup::Failed {
            code: ErrorCode::Capability,
            category: KeymanagerFailureCategory::ServiceRejected,
            service_code: reply["errorCode"].as_i64().and_then(|code| i32::try_from(code).ok()),
        },
        result => {
            let code = match result { Err(code) => code, Ok(_) => ErrorCode::Corrupt };
            CanaryCleanup::Failed {
                code,
                category: failure(KeymanagerOperation::Readback, KeymanagerStage::Roundtrip, code).category,
                service_code: None,
            }
        }
    };
    CanaryResult { operation, cleanup }
}
const URI: &str = "luna://com.webos.service.keymanager3";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub format: String,
    pub key_name: String,
    pub iv: String,
    pub ciphertext: String,
    pub context: Value,
}

fn checked(value: Value) -> Result<Value, ErrorCode> {
    match value["returnValue"].as_bool() {
        Some(true) => Ok(value),
        Some(false) => Err(ErrorCode::Capability),
        None => Err(ErrorCode::Corrupt),
    }
}
fn params(purpose: &[&str]) -> Value {
    json!({"type":"AES","mode":["GCM"],"purpose":purpose,"padding":["None"]})
}
fn generate_params() -> Value {
    let mut value = params(&["encrypt", "decrypt"]);
    value["size"] = json!(256);
    value
}
pub fn seal(
    bus: &mut impl Rpc,
    name: &str,
    plaintext: &str,
    context: Value,
) -> Result<String, ErrorCode> {
    // The caller supplies the freshly generated tenure key name explicitly. A collision is a
    // refusal, never permission to overwrite or use a different owner's key.
    if !valid_key_name(name) {
        return Err(ErrorCode::Invalid);
    }
    checked(bus.call(
        &format!("{URI}/generateKey"),
        &json!({"name":name,"params":generate_params()}),
    )?)?;
    let result = seal_created(bus, name, plaintext, context);
    if result.is_err() {
        // Only this successfully generated, not-yet-persisted generation is eligible.
        let _ = bus.call(&format!("{URI}/removeKey"), &json!({"name":name}));
    }
    result
}
fn seal_created(
    bus: &mut impl Rpc,
    name: &str,
    plaintext: &str,
    context: Value,
) -> Result<String, ErrorCode> {
    let begin = checked(bus.call(
        &format!("{URI}/begin"),
        &json!({"name":name,"params":params(&["encrypt"])}),
    )?)?;
    let handle = begin["handle"]
        .as_str()
        .filter(|h| h.parse::<u64>().is_ok())
        .ok_or(ErrorCode::Corrupt)?;
    let result = (|| {
        let iv = begin["iv"].as_str().ok_or(ErrorCode::Corrupt)?;
        if unbase64(iv)?.is_empty() {
            return Err(ErrorCode::Corrupt);
        }
        let inner = serde_json::to_vec(&json!({"context":context,"payload":plaintext}))
            .map_err(|_| ErrorCode::Invalid)?;
        let result = checked(bus.call(
            &format!("{URI}/finish"),
            &json!({"handle":handle,"data":base64(&inner)}),
        )?)?;
        let ciphertext = result["output"].as_str().ok_or(ErrorCode::Corrupt)?;
        if unbase64(ciphertext)?.len() < 16 {
            return Err(ErrorCode::Corrupt);
        }
        serde_json::to_string(&Envelope {
            format: "keymanager3-aes256-gcm-v1".into(),
            key_name: name.into(),
            iv: iv.into(),
            ciphertext: ciphertext.into(),
            context,
        })
        .map_err(|_| ErrorCode::Invalid)
    })();
    if result.is_err() {
        let _ = bus.call(&format!("{URI}/abort"), &json!({"handle":handle}));
    }
    result
}
pub fn unseal(
    bus: &mut impl Rpc,
    name: &str,
    encoded: &str,
    context: &Value,
) -> Result<String, ErrorCode> {
    if !valid_key_name(name) {
        return Err(ErrorCode::Corrupt);
    }
    unseal_with_name(bus, name, encoded, context)
}
fn unseal_with_name(
    bus: &mut impl Rpc,
    name: &str,
    encoded: &str,
    context: &Value,
) -> Result<String, ErrorCode> {
    let envelope: Envelope = serde_json::from_str(encoded).map_err(|_| ErrorCode::Corrupt)?;
    if envelope.format != "keymanager3-aes256-gcm-v1"
        || envelope.key_name != name
        || &envelope.context != context
        || unbase64(&envelope.iv)?.is_empty()
        || unbase64(&envelope.ciphertext)?.len() < 16
    {
        return Err(ErrorCode::Corrupt);
    }
    let mut p = params(&["decrypt"]);
    p["iv"] = json!(envelope.iv);
    let begin = checked(bus.call(&format!("{URI}/begin"), &json!({"name":name,"params":p}))?)?;
    let handle = begin["handle"]
        .as_str()
        .filter(|h| h.parse::<u64>().is_ok())
        .ok_or(ErrorCode::Corrupt)?;
    let result = (|| {
        let result = checked(bus.call(
            &format!("{URI}/finish"),
            &json!({"handle":handle,"data":envelope.ciphertext}),
        )?)?;
        let inner: Value = serde_json::from_slice(&unbase64(
            result["output"].as_str().ok_or(ErrorCode::Corrupt)?,
        )?)
        .map_err(|_| ErrorCode::Corrupt)?;
        if inner["context"] != *context {
            return Err(ErrorCode::Corrupt);
        }
        inner["payload"]
            .as_str()
            .map(str::to_owned)
            .ok_or(ErrorCode::Corrupt)
    })();
    if result.is_err() {
        let _ = bus.call(&format!("{URI}/abort"), &json!({"handle":handle}));
    }
    result
}
pub fn available(bus: &mut impl Rpc) -> bool {
    // A read-only begin against a reserved, never generated name proves service dispatch and
    // the closed key-not-found error. No stored key or arbitrary platform value is read.
    match bus.call(
        &format!("{URI}/begin"),
        &json!({"name":"plxnative-capability-probe","params":params(&["encrypt"])}),
    ) {
        Ok(v) if v["returnValue"] == false && v["errorCode"] == -10001 => true,
        Ok(v) if v["returnValue"] == true => {
            if let Some(handle) = v["handle"].as_str() {
                let _ = bus.call(&format!("{URI}/abort"), &json!({"handle":handle}));
            }
            false
        }
        _ => false,
    }
}
fn valid_key_name(name: &str) -> bool {
    name.len() == 24
        && name.starts_with("a.")
        && name[2..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn remove_retired(bus: &mut impl Rpc, name: &str) {
    if valid_key_name(name) {
        let _ = bus.call(&format!("{URI}/removeKey"), &json!({"name":name}));
    }
}
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let v = (c[0] as u32) << 16
            | (c.get(1).copied().unwrap_or(0) as u32) << 8
            | c.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[(v >> 18) as usize] as char);
        out.push(ALPHABET[((v >> 12) & 63) as usize] as char);
        out.push(if c.len() > 1 {
            ALPHABET[((v >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            ALPHABET[(v & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
pub fn unbase64(text: &str) -> Result<Vec<u8>, ErrorCode> {
    if text.len() % 4 != 0 {
        return Err(ErrorCode::Corrupt);
    }
    let mut out = Vec::new();
    for (i, c) in text.as_bytes().chunks(4).enumerate() {
        let mut v = 0u32;
        let mut padding = 0;
        for (j, b) in c.iter().enumerate() {
            let n = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if j >= 2 && i + 1 == text.len() / 4 => {
                    padding += 1;
                    0
                }
                _ => return Err(ErrorCode::Corrupt),
            };
            if padding > 0 && *b != b'=' {
                return Err(ErrorCode::Corrupt);
            }
            v = v << 6 | n as u32;
        }
        out.push((v >> 16) as u8);
        if padding < 2 {
            out.push((v >> 8) as u8);
        }
        if padding == 0 {
            out.push(v as u8);
        }
    }
    if base64(&out) != text {
        return Err(ErrorCode::Corrupt);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_key_reuse_does_not_hide_later_failure_or_retire_the_key() {
        for generated in [json!({"returnValue":true}), json!({"returnValue":false,"errorCode":-10002})] {
            let mut rpc = Script { replies:vec![generated,
                json!({"returnValue":false,"errorCode":-20030,"errorText":"private-fixture"})].into(), calls:vec![] };
            let error = seal_control_detailed(&mut rpc, "private-fixture", json!({"namespace":"control"})).unwrap_err();
            assert_eq!(error.stage, KeymanagerStage::Begin);
            assert_eq!(error.service_code, Some(-20030));
            assert_eq!(rpc.calls.len(), 2);
            assert!(rpc.calls.iter().all(|(uri, value)| !uri.ends_with("/removeKey") && value["name"] == CONTROL_KEY));
            assert!(!serde_json::to_string(&error).unwrap().contains("private-fixture"));
        }
    }

    #[test]
    fn control_generation_requires_success_or_explicit_existing_key_reply() {
        for generated in [json!({"returnValue":false,"errorCode":-1000}),
            json!({"errorCode":-10002}), json!({"returnValue":"false","errorCode":-10002})] {
            let mut rpc = Script { replies:vec![generated].into(), calls:vec![] };
            assert!(seal_control_detailed(&mut rpc, "fixture", json!({})).is_err());
            assert_eq!(rpc.calls.len(), 1);
            assert!(rpc.calls[0].0.ends_with("/generateKey"));
        }
    }

    #[test]
    fn control_key_is_never_accepted_by_plex_generation_or_retirement() {
        let mut rpc = Script { replies:vec![].into(), calls:vec![] };
        assert!(seal(&mut rpc, CONTROL_KEY, "fixture", json!({})).is_err());
        remove_retired(&mut rpc, CONTROL_KEY);
        assert!(unseal(&mut rpc, CONTROL_KEY, "{}", &json!({})).is_err());
        let foreign = serde_json::to_string(&Envelope { format:"keymanager3-aes256-gcm-v1".into(),
            key_name:"a.AAAAAAAAAAAAAAAAAAAAAA".into(), iv:base64(&[1;12]), ciphertext:base64(&[2;32]), context:json!({}) }).unwrap();
        assert!(open_control_detailed(&mut rpc, &foreign, &json!({})).is_err());
        assert!(rpc.calls.is_empty());
    }

    #[test]
    fn saved_key_is_distinct_and_never_used_by_plex_or_control() {
        assert_ne!(SAVED_KEY, CONTROL_KEY);
        let mut rpc = Script { replies: vec![].into(), calls: vec![] };
        assert!(seal(&mut rpc, SAVED_KEY, "fixture", json!({})).is_err());
        remove_retired(&mut rpc, SAVED_KEY);
        assert!(unseal(&mut rpc, SAVED_KEY, "{}", &json!({})).is_err());
        let foreign = serde_json::to_string(&Envelope { format:"keymanager3-aes256-gcm-v1".into(),
            key_name:CONTROL_KEY.into(), iv:base64(&[1;12]), ciphertext:base64(&[2;32]), context:json!({}) }).unwrap();
        assert!(open_saved_detailed(&mut rpc, &foreign, &json!({})).is_err());
        assert!(rpc.calls.is_empty());
    }

    #[test]
    fn saved_generate_denial_has_no_fallback_or_key_cleanup() {
        let mut rpc = Script { replies:vec![json!({"returnValue":false,"errorCode":-1000})].into(), calls:vec![] };
        let error = seal_saved_detailed(&mut rpc, "fixture", json!({"namespace":"state"})).unwrap_err();
        assert_eq!(error.service_code, Some(-1000));
        assert_eq!(rpc.calls.len(), 1);
        assert_eq!(rpc.calls[0].1["name"], SAVED_KEY);
        assert!(rpc.calls[0].0.ends_with("/generateKey"));
    }

    struct CanaryScript {
        replies: std::collections::VecDeque<Result<Value, ErrorCode>>,
        calls: Vec<(String, Value)>,
    }
    impl Rpc for CanaryScript {
        fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode> {
            self.calls.push((uri.into(), payload.clone()));
            self.replies.pop_front().expect("unexpected canary RPC")
        }
    }
    fn canary_script(replies: Vec<Result<Value, ErrorCode>>) -> CanaryScript {
        CanaryScript { replies: replies.into(), calls: vec![] }
    }
    fn canary_success() -> Vec<Result<Value, ErrorCode>> {
        vec![
            Ok(json!({"returnValue":true})),
            Ok(json!({"returnValue":true,"handle":"41","iv":base64(&[0;12])})),
            Ok(json!({"returnValue":true,"output":base64(&[7;32])})),
            Ok(json!({"returnValue":true,"handle":"42"})),
            Ok(json!({"returnValue":true,"output":base64(&serde_json::to_vec(
                &json!({"context":canary_context(),"payload":CANARY})
            ).unwrap())})),
            Ok(json!({"returnValue":true})),
        ]
    }
    fn probe_fixture(rpc: &mut CanaryScript) -> CanaryResult {
        probe_owned_key_with(rpc, || Ok(super::super::state::Generation([5;16])))
    }
    fn canary_methods(rpc: &CanaryScript) -> Vec<&str> {
        rpc.calls.iter().map(|(uri, _)| uri.rsplit('/').next().unwrap()).collect()
    }
    fn assert_canary_private(result: &CanaryResult) {
        let output = format!("{result:?} {}", serde_json::to_string(result).unwrap());
        for forbidden in ["private-response", "key_name", "handle", "ciphertext", "errorText", CANARY] {
            assert!(!output.contains(forbidden), "canary evidence included {forbidden}");
        }
        assert!(!output.contains(&super::super::state::Generation([5;16]).key_name()));
    }

    #[test]
    fn canary_roundtrip_removes_only_its_generated_key_and_returns_no_material() {
        let mut rpc = canary_script(canary_success());
        let result = probe_fixture(&mut rpc);
        assert!(result.complete());
        assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "finish", "begin", "finish", "removeKey"]);
        let name = super::super::state::Generation([5;16]).key_name();
        for i in [0, 1, 3, 5] {
            assert_eq!(rpc.calls[i].1["name"], name);
        }
        assert_eq!(rpc.calls[0].1["params"], generate_params());
        assert_eq!(rpc.calls[1].1["params"], params(&["encrypt"]));
        let inner: Value = serde_json::from_slice(
            &unbase64(rpc.calls[2].1["data"].as_str().unwrap()).unwrap()
        ).unwrap();
        assert_eq!(inner, json!({"context":canary_context(),"payload":CANARY}));
        assert!(rpc.replies.is_empty());
        assert_canary_private(&result);
    }

    #[test]
    fn canary_entropy_failure_never_reaches_keymanager() {
        let mut rpc = canary_script(vec![]);
        let result = probe_owned_key_with(&mut rpc, || Err(ErrorCode::Unavailable));
        assert_eq!(result.cleanup, CanaryCleanup::NotCreated {});
        assert_eq!(result.operation.unwrap_err().stage, KeymanagerStage::Validate);
        assert!(!result.complete());
        assert!(rpc.calls.is_empty());
    }

    #[test]
    fn canary_rejected_or_unconfirmed_generation_never_opens_removes_or_retries() {
        for (reply, cleanup, service_code) in [
            (Ok(json!({"returnValue":false,"errorCode":-10002,"errorText":"private-response"})), CanaryCleanup::NotCreated {}, Some(-10002)),
            (Ok(json!({"returnValue":false,"errorCode":-1000})), CanaryCleanup::NotCreated {}, Some(-1000)),
            (Err(ErrorCode::Timeout), CanaryCleanup::CreationUnconfirmed {}, None),
            (Err(ErrorCode::Unavailable), CanaryCleanup::CreationUnconfirmed {}, None),
            (Ok(json!({"returnValue":"true","errorText":"private-response"})), CanaryCleanup::CreationUnconfirmed {}, None),
        ] {
            let mut rpc = canary_script(vec![reply]);
            let result = probe_fixture(&mut rpc);
            let failure = result.operation.unwrap_err();
            assert_eq!(failure.stage, KeymanagerStage::Generate);
            assert_eq!(failure.service_code, service_code);
            assert_eq!(failure.hub_error, None);
            assert_eq!(result.cleanup, cleanup);
            assert!(!result.complete());
            assert_eq!(canary_methods(&rpc), ["generateKey"]);
            assert_canary_private(&result);
        }
    }

    #[test]
    fn canary_seal_begin_failure_still_removes_once_without_opening() {
        let mut rpc = canary_script(vec![
            Ok(json!({"returnValue":true})),
            Ok(json!({"returnValue":false,"errorCode":-20030})),
            Ok(json!({"returnValue":true})),
        ]);
        let result = probe_fixture(&mut rpc);
        let failure = result.operation.unwrap_err();
        assert_eq!(failure.operation, KeymanagerOperation::Seal);
        assert_eq!(failure.stage, KeymanagerStage::Begin);
        assert_eq!(failure.service_code, Some(-20030));
        assert_eq!(result.cleanup, CanaryCleanup::Removed {});
        assert!(!result.complete());
        assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "removeKey"]);
    }

    #[test]
    fn canary_failed_finish_preserves_primary_error_across_abort_and_cleanup_failures() {
        let mut replies = canary_success();
        replies.truncate(2);
        replies.extend([
            Ok(json!({"returnValue":false,"errorCode":-20030,"errorText":"private-response"})),
            Err(ErrorCode::Timeout),
            Ok(json!({"returnValue":false,"errorCode":-1000,"errorText":"private-response"})),
        ]);
        let mut rpc = canary_script(replies);
        let result = probe_fixture(&mut rpc);
        let failure = result.operation.unwrap_err();
        assert_eq!(failure.operation, KeymanagerOperation::Seal);
        assert_eq!(failure.stage, KeymanagerStage::Finish);
        assert_eq!(failure.service_code, Some(-20030));
        assert_eq!(result.cleanup, CanaryCleanup::Failed {
            code: ErrorCode::Capability,
            category: KeymanagerFailureCategory::ServiceRejected,
            service_code: Some(-1000),
        });
        assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "finish", "abort", "removeKey"]);
        assert_eq!(rpc.calls[3].1, json!({"handle":"41"}));
        assert_canary_private(&result);
    }

    #[test]
    fn canary_wrong_plaintext_is_not_success_and_still_removes_once() {
        let mut replies = canary_success();
        replies[4] = Ok(json!({"returnValue":true,"output":base64(&serde_json::to_vec(
            &json!({"context":canary_context(),"payload":"private-response"})
        ).unwrap())}));
        let mut rpc = canary_script(replies);
        let result = probe_fixture(&mut rpc);
        let failure = result.operation.unwrap_err();
        assert_eq!(failure.operation, KeymanagerOperation::Readback);
        assert_eq!(failure.stage, KeymanagerStage::Roundtrip);
        assert_eq!(failure.code, ErrorCode::Corrupt);
        assert_eq!(result.cleanup, CanaryCleanup::Removed {});
        assert!(!result.complete());
        assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "finish", "begin", "finish", "removeKey"]);
        assert_canary_private(&result);
    }

    #[test]
    fn canary_readback_failure_aborts_its_handle_then_removes_the_created_key() {
        let mut replies = canary_success();
        replies.truncate(4);
        replies.extend([
            Ok(json!({"returnValue":false,"errorCode":-20030})),
            Ok(json!({"returnValue":true})),
            Ok(json!({"returnValue":true})),
        ]);
        let mut rpc = canary_script(replies);
        let result = probe_fixture(&mut rpc);
        let failure = result.operation.unwrap_err();
        assert_eq!(failure.operation, KeymanagerOperation::Readback);
        assert_eq!(failure.stage, KeymanagerStage::Finish);
        assert_eq!(failure.service_code, Some(-20030));
        assert_eq!(result.cleanup, CanaryCleanup::Removed {});
        assert!(!result.complete());
        assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "finish", "begin", "finish", "abort", "removeKey"]);
        assert_eq!(rpc.calls[5].1, json!({"handle":"42"}));
    }

    #[test]
    fn canary_cleanup_failure_never_becomes_complete_or_retries() {
        for (reply, code, service_code) in [
            (Err(ErrorCode::Timeout), ErrorCode::Timeout, None),
            (Ok(json!({"returnValue":null,"errorText":"private-response"})), ErrorCode::Corrupt, None),
            (Ok(json!({"returnValue":false,"errorCode":i64::MAX,"errorText":"private-response"})), ErrorCode::Capability, None),
            (Ok(json!({"returnValue":false,"errorCode":-10001})), ErrorCode::Capability, Some(-10001)),
        ] {
            let mut replies = canary_success();
            replies[5] = reply;
            let mut rpc = canary_script(replies);
            let result = probe_fixture(&mut rpc);
            assert_eq!(result.operation, Ok(()));
            let CanaryCleanup::Failed { code: actual_code, service_code: actual_service_code, .. } = result.cleanup else {
                panic!("cleanup failure was lost");
            };
            assert_eq!(actual_code, code);
            assert_eq!(actual_service_code, service_code);
            assert!(!result.complete());
            assert_eq!(canary_methods(&rpc), ["generateKey", "begin", "finish", "begin", "finish", "removeKey"]);
            assert_canary_private(&result);
        }
    }

    #[test]
    fn canonical_base64_and_invalid_padding() {
        for b in [b"".as_slice(), b"f", b"fo", b"foo", b"foob"] {
            assert_eq!(unbase64(&base64(b)).unwrap(), b);
        }
        for bad in ["Zg=Z", "====", "Zg=", "Zh=="] {
            assert!(unbase64(bad).is_err());
        }
    }
    struct Absent;
    impl Rpc for Absent {
        fn call(&mut self, _: &str, _: &Value) -> Result<Value, ErrorCode> {
            Err(ErrorCode::Unavailable)
        }
    }
    #[test]
    fn absent_service_is_failure_not_plaintext_fallback() {
        assert!(!available(&mut Absent));
        assert!(seal(&mut Absent, "a.AAAAAAAAAAAAAAAAAAAAAA", "secret", json!({})).is_err());
    }
    #[test]
    fn failure_evidence_accepts_only_bounded_numeric_service_codes() {
        for (raw, expected) in [
            (json!(-20030), Some(-20030)),
            (json!(i32::MIN), Some(i32::MIN)),
            (json!(i32::MAX), Some(i32::MAX)),
            (json!(i64::MAX), None),
            (json!(u64::MAX), None),
            (json!("private-fixture"), None),
            (json!(-1.5), None),
        ] {
            let mut rpc = Script {
                replies: vec![json!({"returnValue":false,"errorCode":raw,"errorText":"private-fixture","key_name":"private-fixture"})].into(),
                calls: vec![],
            };
            let evidence = seal_detailed(
                &mut rpc,
                "a.AAAAAAAAAAAAAAAAAAAAAA",
                "private-fixture",
                json!({}),
            )
            .unwrap_err();
            assert_eq!(evidence.service_code, expected);
            assert_eq!(
                evidence.category,
                KeymanagerFailureCategory::ServiceRejected
            );
            assert!(!serde_json::to_string(&evidence)
                .unwrap()
                .contains("private-fixture"));
            assert!(!format!("{evidence:?}").contains("private-fixture"));
        }
    }

    #[test]
    fn malformed_return_value_is_invalid_response_not_service_rejection() {
        for reply in [
            json!({}),
            json!({"returnValue":"false"}),
            json!({"returnValue":null}),
        ] {
            let mut rpc = Script {
                replies: vec![reply].into(),
                calls: vec![],
            };
            let failure = seal_detailed(&mut rpc, "a.AAAAAAAAAAAAAAAAAAAAAA", "fixture", json!({}))
                .unwrap_err();
            assert_eq!(failure.category, KeymanagerFailureCategory::InvalidResponse);
            assert_eq!(failure.code, ErrorCode::Corrupt);
            assert_eq!(failure.stage, KeymanagerStage::Generate);
            assert_eq!(failure.service_code, None);
        }
    }

    struct Script {
        replies: std::collections::VecDeque<Value>,
        calls: Vec<(String, Value)>,
    }
    impl Rpc for Script {
        fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode> {
            self.calls.push((uri.into(), payload.clone()));
            Ok(self
                .replies
                .pop_front()
                .expect("unexpected keymanager call"))
        }
    }
    #[test]
    fn sealing_uses_generation_key_aes256_gcm_and_authenticated_context() {
        let mut rpc = Script {
            replies: vec![
                json!({"returnValue":true}),
                json!({"returnValue":true,"handle":"18446744073709551615","iv":base64(&[0;12])}),
                json!({"returnValue":true,"output":base64(&[7;32])}),
            ]
            .into(),
            calls: vec![],
        };
        let name = "a.AAAAAAAAAAAAAAAAAAAAAA";
        let context = json!({"generation":"fixture","epoch":7});
        let envelope = seal(&mut rpc, name, "secret-fixture", context.clone()).unwrap();
        assert_eq!(rpc.calls[0].1["name"], name);
        assert_eq!(rpc.calls[0].1["params"]["size"], 256);
        assert!(rpc.calls[1].1["params"].get("size").is_none());
        assert_eq!(rpc.calls[0].1["params"]["mode"], json!(["GCM"]));
        assert_eq!(rpc.calls[2].1["handle"], "18446744073709551615");
        assert!(rpc.calls[0].1["params"].get("mac_length").is_none());
        assert!(rpc.calls[2].1.get("aad").is_none());
        let inner: Value =
            serde_json::from_slice(&unbase64(rpc.calls[2].1["data"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(inner, json!({"context":context,"payload":"secret-fixture"}));
        assert!(!envelope.contains("secret-fixture"));
        let count = rpc.calls.len();
        assert!(unseal(&mut rpc, name, &envelope, &json!({"epoch":8})).is_err());
        assert_eq!(rpc.calls.len(), count);
    }
    #[test]
    fn failed_finish_aborts_the_acquired_handle() {
        let mut rpc = Script {
            replies: vec![
                json!({"returnValue":true}),
                json!({"returnValue":true,"handle":"9","iv":base64(&[0;12])}),
                json!({"returnValue":false,"errorCode":-20030}),
                json!({"returnValue":true}),
                json!({"returnValue":true}),
            ]
            .into(),
            calls: vec![],
        };
        assert!(seal(&mut rpc, "a.AAAAAAAAAAAAAAAAAAAAAA", "secret", json!({})).is_err());
        assert_eq!(
            &rpc.calls[3],
            &(format!("{URI}/abort"), json!({"handle":"9"}))
        );
        assert_eq!(
            rpc.calls.last().unwrap(),
            &(
                format!("{URI}/removeKey"),
                json!({"name":"a.AAAAAAAAAAAAAAAAAAAAAA"})
            )
        );
    }

    #[test]
    fn authenticated_inner_context_must_match_even_when_outer_was_replaced() {
        let name = "a.AAAAAAAAAAAAAAAAAAAAAA";
        let outer = json!({"epoch":2});
        let inner = json!({"context":{"epoch":1},"payload":"secret"});
        let envelope = serde_json::to_string(&Envelope {
            format: "keymanager3-aes256-gcm-v1".into(),
            key_name: name.into(),
            iv: base64(&[1; 16]),
            ciphertext: base64(&[2; 32]),
            context: outer.clone(),
        })
        .unwrap();
        let mut rpc = Script {
            replies: vec![
                json!({"returnValue":true,"handle":"9"}),
                json!({"returnValue":true,"output":base64(&serde_json::to_vec(&inner).unwrap())}),
                json!({"returnValue":true}),
            ]
            .into(),
            calls: vec![],
        };
        assert!(unseal(&mut rpc, name, &envelope, &outer).is_err());
        assert!(rpc.calls[0].1["params"].get("size").is_none());
        assert_eq!(
            rpc.calls[1]
                .1
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["data", "handle"]
        );
        assert!(rpc.calls.iter().all(|(uri, _)| !uri.ends_with("removeKey")));
    }
}

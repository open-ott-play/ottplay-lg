//! Actual named-key sequencing through the existing synthetic LS2 boundary.
//! These tests do not establish firmware availability, real crypto, or durable storage.
use super::*;
use crate::storage::wire::{
    ErrorCode, KeymanagerFailure, KeymanagerFailureCategory, KeymanagerOperation, KeymanagerStage,
};
use crate::tv::secure::Namespace;
use std::cell::RefCell;
use std::collections::VecDeque;

type Reply = (&'static str, Result<String, ()>);
type RpcHook = fn(&str, &str) -> Result<String, ()>;

#[derive(Default)]
struct Script {
    replies: VecDeque<Reply>,
    calls: Vec<(String, Value)>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
}

struct Hook(Option<RpcHook>);
impl Hook {
    fn new(replies: Vec<Reply>) -> Self {
        // Production detailed operations publish through the existing worker.
        // Keep this same-thread RPC regression free of diagnostic filesystem IO.
        crate::storage::diagnostics::disable();
        SCRIPT.with(|script| {
            let mut script = script.borrow_mut();
            assert!(script.replies.is_empty());
            *script = Script {
                replies: replies.into(),
                calls: Vec::new(),
            };
        });
        Self(RPC_FOR_TEST.with(|hook| hook.replace(Some(rpc))))
    }

    fn calls(&self) -> Vec<(String, Value)> {
        SCRIPT.with(|script| {
            let script = script.borrow();
            assert!(
                script.replies.is_empty(),
                "expected LS2 stage was not reached"
            );
            script.calls.clone()
        })
    }
}
impl Drop for Hook {
    fn drop(&mut self) {
        RPC_FOR_TEST.with(|hook| hook.set(self.0));
        SCRIPT.with(|script| *script.borrow_mut() = Script::default());
        crate::storage::diagnostics::reset_for_test();
    }
}

fn rpc(uri: &str, payload: &str) -> Result<String, ()> {
    SCRIPT.with(|script| {
        let mut script = script.borrow_mut();
        let (method, response) = script.replies.pop_front().expect("unexpected LS2 call");
        assert_eq!(
            uri,
            format!("luna://com.webos.service.keymanager3/{method}")
        );
        script
            .calls
            .push((method.to_owned(), serde_json::from_str(payload).unwrap()));
        response
    })
}

fn reply(method: &'static str, value: Value) -> Reply {
    (method, Ok(value.to_string()))
}

fn prefix(stage: KeymanagerStage) -> Vec<Reply> {
    let mut replies = Vec::new();
    if stage != KeymanagerStage::Generate {
        replies.push(reply("generateKey", json!({"returnValue":true})));
    }
    if stage == KeymanagerStage::Finish {
        replies.push(reply(
            "begin",
            json!({"returnValue":true,"handle":"17","iv":"AA=="}),
        ));
    }
    replies
}

fn method(stage: KeymanagerStage) -> &'static str {
    match stage {
        KeymanagerStage::Generate => "generateKey",
        KeymanagerStage::Begin => "begin",
        KeymanagerStage::Finish => "finish",
        _ => panic!("not a named-key RPC stage"),
    }
}

fn failure<T>(result: Result<T, KeymanagerFailure>) -> KeymanagerFailure {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected named-key failure"),
    }
}

fn assert_private(error: KeymanagerFailure) {
    let text = serde_json::to_string(&error).unwrap();
    let debug = format!("{error:?}");
    for secret in [
        "private-input",
        "private-response",
        "private-key",
        "private-iv",
    ] {
        assert!(!text.contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[test]
fn named_seal_denials_keep_stage_code_and_primary_failure() {
    let _serial = plx_base::testlock::serial();
    for stage in [
        KeymanagerStage::Generate,
        KeymanagerStage::Begin,
        KeymanagerStage::Finish,
    ] {
        let mut replies = prefix(stage);
        replies.push(reply(
            method(stage),
            json!({
                "returnValue":false,"errorCode":-20030,"errorText":"private-response",
                "name":"private-key","iv":"private-iv","output":"private-input"
            }),
        ));
        if stage == KeymanagerStage::Finish {
            // A different cleanup error must not replace the failed finish.
            replies.push(reply("abort", json!({"returnValue":false,"errorCode":-9})));
        }
        let hook = Hook::new(replies);
        let error = failure(seal_named_detailed(Namespace::OttState, b"private-input"));
        assert_eq!(error.operation, KeymanagerOperation::Seal);
        assert_eq!(error.stage, stage);
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.category, KeymanagerFailureCategory::ServiceRejected);
        assert_eq!(error.service_code, Some(-20030));
        assert_private(error);
        let calls = hook.calls();
        assert!(!calls.iter().any(|(name, _)| name == "removeKey"));
        if stage == KeymanagerStage::Finish {
            assert_eq!(calls.last().unwrap().1, json!({"handle":"17"}));
        }
    }
}

#[test]
fn named_seal_transport_and_malformed_json_fail_at_the_attempted_stage() {
    let _serial = plx_base::testlock::serial();
    for stage in [
        KeymanagerStage::Generate,
        KeymanagerStage::Begin,
        KeymanagerStage::Finish,
    ] {
        for transport_error in [false, true] {
            let mut replies = prefix(stage);
            replies.push((
                method(stage),
                if transport_error {
                    Err(())
                } else {
                    Ok("private-response is not JSON".into())
                },
            ));
            if stage == KeymanagerStage::Finish {
                replies.push(reply("abort", json!({"returnValue":true})));
            }
            let hook = Hook::new(replies);
            let error = failure(seal_named_detailed(Namespace::OttState, b"private-input"));
            assert_eq!(error.operation, KeymanagerOperation::Seal);
            assert_eq!(error.stage, stage);
            assert_eq!(error.service_code, None);
            assert_eq!(
                error.code,
                if transport_error {
                    ErrorCode::Unavailable
                } else {
                    ErrorCode::Corrupt
                }
            );
            assert_eq!(
                error.category,
                if transport_error {
                    KeymanagerFailureCategory::Unavailable
                } else {
                    KeymanagerFailureCategory::InvalidResponse
                }
            );
            assert_private(error);
            hook.calls();
        }
    }
}

#[test]
fn named_seal_missing_required_fields_never_become_success() {
    let _serial = plx_base::testlock::serial();
    for (stage, response, abort) in [
        (
            KeymanagerStage::Generate,
            json!({"returnValue":"true"}),
            false,
        ),
        (
            KeymanagerStage::Begin,
            json!({"returnValue":true,"iv":"AA=="}),
            false,
        ),
        (
            KeymanagerStage::Begin,
            json!({"returnValue":true,"handle":"17"}),
            true,
        ),
        (KeymanagerStage::Finish, json!({"returnValue":true}), false),
    ] {
        let mut replies = prefix(stage);
        replies.push(reply(method(stage), response));
        if abort {
            replies.push(reply("abort", json!({"returnValue":true})));
        }
        let hook = Hook::new(replies);
        let error = failure(seal_named_detailed(Namespace::OttState, b"private-input"));
        assert_eq!(error.stage, stage);
        assert_eq!(error.code, ErrorCode::Corrupt);
        assert_eq!(error.category, KeymanagerFailureCategory::InvalidResponse);
        assert_eq!(error.service_code, None);
        hook.calls();
    }
}

#[test]
fn named_seal_service_codes_are_bounded_and_payload_free() {
    let _serial = plx_base::testlock::serial();
    for (raw, expected) in [
        (json!(i32::MIN), Some(i32::MIN)),
        (json!(i32::MAX), Some(i32::MAX)),
        (json!(i64::MAX), None),
        (json!(u64::MAX), None),
        (json!("private-response"), None),
        (json!(1.5), None),
    ] {
        let raw_text = raw.to_string();
        let hook = Hook::new(vec![reply(
            "generateKey",
            json!({
                "returnValue":false,"errorCode":raw,"errorText":"private-response"
            }),
        )]);
        let error = failure(seal_named_detailed(Namespace::OttState, b"private-input"));
        assert_eq!(error.stage, KeymanagerStage::Generate);
        assert_eq!(error.category, KeymanagerFailureCategory::ServiceRejected);
        assert_eq!(error.service_code, expected);
        assert_private(error);
        if expected.is_none() {
            assert!(!serde_json::to_string(&error).unwrap().contains(&raw_text));
        }
        hook.calls();
    }
}

#[test]
fn existing_named_key_keeps_old_envelope_openable_after_failed_replacement() {
    let _serial = plx_base::testlock::serial();
    let ciphertext = b64::encode(b"opaque synthetic ciphertext");
    let hook = Hook::new(vec![
        reply(
            "generateKey",
            json!({"returnValue":false,"errorCode":-10002}),
        ),
        reply(
            "begin",
            json!({"returnValue":true,"handle":"17","iv":"AA=="}),
        ),
        reply("finish", json!({"returnValue":true,"output":ciphertext})),
        reply(
            "generateKey",
            json!({"returnValue":false,"errorCode":-10002}),
        ),
        reply(
            "begin",
            json!({"returnValue":true,"handle":"18","iv":"AQ=="}),
        ),
        reply("finish", json!({"returnValue":false,"errorCode":-20030})),
        reply("abort", json!({"returnValue":true})),
        reply("begin", json!({"returnValue":true,"handle":"19"})),
        reply(
            "finish",
            json!({"returnValue":true,"output":b64::encode(b"old private value")}),
        ),
    ]);
    let sealed = seal_named_detailed(Namespace::OttState, b"old private value").unwrap();
    let old = sealed.clone();
    assert_eq!(
        failure(seal_named_detailed(Namespace::OttState, b"replacement")).stage,
        KeymanagerStage::Finish
    );
    assert_eq!(sealed, old);
    assert_eq!(
        open_named_detailed(Namespace::OttState, &sealed)
            .unwrap()
            .as_slice(),
        b"old private value"
    );
    let calls = hook.calls();
    assert_eq!(sealed.key, Namespace::OttState.key_name());
    assert_ne!(sealed.key, Namespace::OttControl.key_name());
    assert_ne!(sealed.key, KEY_NAME);
    assert_eq!(
        calls[0].1,
        json!({"name":sealed.key,"params":{
            "type":"AES","size":256,"mode":["GCM"],
            "purpose":["encrypt","decrypt"],"padding":["None"]
        }})
    );
    assert_eq!(
        calls[1].1,
        json!({"name":sealed.key,"params":{
            "type":"AES","mode":["GCM"],"purpose":["encrypt"],
            "padding":["None"],"mac_length":"128"
        }})
    );
    assert_eq!(
        calls[2].1,
        json!({"handle":"17","data":b64::encode(b"old private value")})
    );
    assert_eq!(
        calls[7].1,
        json!({"name":sealed.key,"params":{
            "type":"AES","mode":["GCM"],"purpose":["decrypt"],
            "padding":["None"],"mac_length":"128","iv":"AA=="
        }})
    );
    assert_eq!(calls[8].1, json!({"handle":"19","data":ciphertext}));
    assert!(!calls.iter().any(|(name, _)| name == "removeKey"));
}

#[test]
fn named_open_refuses_foreign_envelopes_before_rpc_and_labels_open_failures() {
    let _serial = plx_base::testlock::serial();
    let hook = Hook::new(Vec::new());
    let mut sealed = Sealed {
        backend: Backend::Keymanager3,
        key: Namespace::OttState.key_name(),
        iv: "AA==".into(),
        data: b64::encode(b"opaque synthetic ciphertext"),
    };
    let error = failure(open_named_detailed(Namespace::OttControl, &sealed));
    assert_eq!(error.operation, KeymanagerOperation::Open);
    assert_eq!(error.stage, KeymanagerStage::Validate);
    sealed.backend = Backend::PalmKeymanager;
    assert_eq!(
        failure(open_named_detailed(Namespace::OttState, &sealed)).stage,
        KeymanagerStage::Validate
    );
    assert!(hook.calls().is_empty());
    drop(hook);

    sealed.backend = Backend::Keymanager3;
    let hook = Hook::new(vec![reply(
        "begin",
        json!({"returnValue":false,"errorCode":-20030}),
    )]);
    let error = failure(open_named_detailed(Namespace::OttState, &sealed));
    assert_eq!(error.operation, KeymanagerOperation::Open);
    assert_eq!(error.stage, KeymanagerStage::Begin);
    assert_eq!(error.category, KeymanagerFailureCategory::ServiceRejected);
    assert_eq!(error.service_code, Some(-20030));
    hook.calls();
}

#[test]
fn typed_ls2_timeout_and_setup_failure_keep_their_distinct_categories() {
    // The existing RPC hook returns only (). This checks the production typed
    // LS2 mapping, not an elapsed timeout or a real bus-registration failure.
    let timeout =
        CallFailure::timeout().into_failure(KeymanagerOperation::Open, KeymanagerStage::Finish);
    assert_eq!(timeout.operation, KeymanagerOperation::Open);
    assert_eq!(timeout.stage, KeymanagerStage::Finish);
    assert_eq!(timeout.code, ErrorCode::Timeout);
    assert_eq!(timeout.category, KeymanagerFailureCategory::Timeout);
    assert_eq!(timeout.service_code, None);
    assert_eq!(timeout.hub_error, None);
    let setup = CallFailure::unavailable(Some(-1027))
        .into_failure(KeymanagerOperation::Seal, KeymanagerStage::Generate);
    assert_eq!(setup.operation, KeymanagerOperation::Seal);
    assert_eq!(setup.stage, KeymanagerStage::Generate);
    assert_eq!(setup.code, ErrorCode::Unavailable);
    assert_eq!(setup.category, KeymanagerFailureCategory::Unavailable);
    assert_eq!(setup.service_code, Some(-1027));
    assert_eq!(setup.hub_error, None);
}

#[test]
fn named_rejection_does_not_infer_hub_origin_from_json_or_repeat_the_rpc() {
    let _serial = plx_base::testlock::serial();
    let hook = Hook::new(vec![reply("generateKey", json!({
        "returnValue":false, "errorCode":-1000, "hub_error":true,
        "errorText":"private-response"
    }))]);
    let error = failure(seal_named_detailed(Namespace::OttState, b"private-input"));
    assert_eq!(error.hub_error, None);
    assert_eq!(error.operation, KeymanagerOperation::Seal);
    assert_eq!(error.stage, KeymanagerStage::Generate);
    assert_eq!(error.code, ErrorCode::Capability);
    assert_eq!(error.category, KeymanagerFailureCategory::ServiceRejected);
    assert_eq!(error.service_code, Some(-1000));
    assert_private(error);
    let calls = hook.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "generateKey");
}

#[test]
fn reply_origin_is_transport_evidence_not_json_or_authorization() {
    for stage in [KeymanagerStage::Generate, KeymanagerStage::Begin, KeymanagerStage::Finish] {
        for hub_error in [None, Some(false), Some(true)] {
            let (value, observed) = decode_reply(CallReply {
                payload: json!({
                    "returnValue":false, "errorCode":-1000,
                    "errorText":"private-response", "hub_error":hub_error != Some(true),
                    "name":"private-key", "data":"private-input"
                }).to_string(),
                hub_error,
            }, KeymanagerOperation::Seal, stage).unwrap();
            let error = response_failure(&value, KeymanagerOperation::Seal, stage, observed);
            assert_eq!(error.hub_error, hub_error);
            assert_eq!(error.stage, stage);
            assert_eq!(error.service_code, Some(-1000));
            // In particular, false says only that LS2 did not mark this as a hub error.
            // It cannot turn the service's refusal into permission or crypto success.
            assert_eq!(error.code, ErrorCode::Capability);
            assert_eq!(error.category, KeymanagerFailureCategory::ServiceRejected);
            assert_private(error);
        }
    }
}

#[test]
fn invalid_json_preserves_only_transport_origin() {
    for hub_error in [None, Some(false), Some(true)] {
        let error = failure(decode_reply(CallReply {
            payload: "private-response: invalid JSON private-input".into(),
            hub_error,
        }, KeymanagerOperation::Open, KeymanagerStage::Begin));
        assert_eq!(error.hub_error, hub_error);
        assert_eq!(error.service_code, None);
        assert_eq!(error.category, KeymanagerFailureCategory::InvalidResponse);
        assert_private(error);
    }
}

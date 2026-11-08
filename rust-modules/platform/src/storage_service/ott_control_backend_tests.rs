use super::*;
use std::collections::BTreeMap;

const APP: &str = "com.openottplay.player.native";
#[derive(Default)]
struct Bus {
    object: Option<Value>,
    calls: Vec<(String, Value)>,
    key_exists: bool,
    deny_generate: bool,
    deny_decrypt: bool,
    fail_postwrite_decrypt: bool,
    wrong_postwrite_plaintext: bool,
    reject_put: bool,
    lose_ack: bool,
    alter_after_put: bool,
    put_seen: bool,
    next: u64,
    handles: BTreeMap<String, bool>,
    ciphertext: BTreeMap<String, Vec<u8>>,
}
impl Rpc for Bus {
    fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode> {
        self.calls.push((uri.into(), payload.clone()));
        match uri {
            "luna://com.palm.db/get" => Ok(json!({"returnValue":true,
                "results":self.object.clone().into_iter().collect::<Vec<_>>()})),
            "luna://com.palm.db/put" => {
                self.put_seen = true;
                if self.reject_put { return Ok(json!({"returnValue":false,"errorCode":-3961})); }
                let mut object = payload["objects"][0].clone();
                let old = self.object.as_ref().and_then(|object| object["_rev"].as_u64());
                assert_eq!(old, object["_rev"].as_u64(), "CAS revision omitted or changed");
                object["_rev"] = json!(old.unwrap_or(0) + 1);
                if self.alter_after_put {
                    let mut state: Value = serde_json::from_str(object["state"].as_str().unwrap()).unwrap();
                    state["public_preferences"] = json!({"version":1,"interface_mode":1});
                    object["state"] = json!(serde_json::to_string(&state).unwrap());
                }
                self.object = Some(object);
                if self.lose_ack { Err(ErrorCode::Timeout) } else { Ok(json!({"returnValue":true})) }
            }
            "luna://com.webos.service.keymanager3/generateKey" => {
                assert_eq!(payload["name"], control::KEY_NAME);
                if self.deny_generate { return Ok(json!({"returnValue":false,"errorCode":-1000,"errorText":"private-rejection"})); }
                if self.key_exists { return Ok(json!({"returnValue":false,"errorCode":-10002})); }
                self.key_exists = true;
                Ok(json!({"returnValue":true}))
            }
            "luna://com.webos.service.keymanager3/begin" => {
                assert_eq!(payload["name"], control::KEY_NAME);
                let encrypt = payload["params"]["purpose"][0] == "encrypt";
                if !encrypt && (self.deny_decrypt || self.fail_postwrite_decrypt && self.put_seen) {
                    return Ok(json!({"returnValue":false,"errorCode":-20030}));
                }
                self.next += 1;
                let handle = self.next.to_string();
                self.handles.insert(handle.clone(), encrypt);
                Ok(json!({"returnValue":true,"handle":handle,"iv":keymanager::base64(&[0;12])}))
            }
            "luna://com.webos.service.keymanager3/finish" => {
                let handle = payload["handle"].as_str().unwrap();
                let encrypt = self.handles.remove(handle).unwrap();
                let input = payload["data"].as_str().unwrap();
                if encrypt {
                    let mut bytes = [0;32];
                    bytes[..8].copy_from_slice(&self.next.to_le_bytes());
                    let cipher = keymanager::base64(&bytes);
                    self.ciphertext.insert(cipher.clone(), keymanager::unbase64(input).unwrap());
                    Ok(json!({"returnValue":true,"output":cipher}))
                } else {
                    let mut plain = self.ciphertext.get(input).unwrap().clone();
                    if self.wrong_postwrite_plaintext && self.put_seen {
                        let mut inner: Value = serde_json::from_slice(&plain).unwrap();
                        inner["payload"] = json!(serde_json::to_string(&config('z')).unwrap());
                        plain = serde_json::to_vec(&inner).unwrap();
                    }
                    Ok(json!({"returnValue":true,"output":keymanager::base64(&plain)}))
                }
            }
            "luna://com.webos.service.keymanager3/abort" => {
                self.handles.remove(payload["handle"].as_str().unwrap());
                Ok(json!({"returnValue":true}))
            }
            _ => panic!("unexpected RPC, including forbidden key deletion: {uri}"),
        }
    }
}
fn backend() -> Backend<Bus> {
    Backend::new(Bus::default(), Flavor::Stable, format!("{APP}.storage"))
}
fn config(token: char) -> control::Config {
    control::Config { version:1, enabled:true, address:"https://home.example:443/control".into(),
        token:SecretString(token.to_string().repeat(32)), device_id:"dev_fixture".into(), allow_insecure_http:false }
}
fn stored(b: &Backend<Bus>) -> State {
    State::decode(b.rpc.object.as_ref().unwrap()["state"].as_str().unwrap(), Flavor::Stable).unwrap()
}
fn expected(b: &Backend<Bus>) -> ott::Expected {
    match &b.rpc.object {
        None => ott::Expected::Missing {},
        Some(object) => {
            let state = stored(b);
            ott::Expected::Present { db_rev:object["_rev"].as_u64().unwrap().to_string(),
                revision:state.revision, epoch:state.epoch }
        }
    }
}
fn signed(mut request: Request) -> Request {
    let digest = state::digest_bytes(&wire::request_digest_bytes(&request).unwrap());
    match &mut request {
        Request::OttControlCommit { digest: field, .. } | Request::OttCommit { digest: field, .. } => *field = digest,
        _ => panic!("unsigned request"),
    }
    request
}
fn request(b: &Backend<Bus>, n: u64, source: control::Source, value: control::Config) -> Request {
    signed(Request::OttControlCommit { expected:expected(b), operation_id:format!("{n:032x}"),
        digest:String::new(), source, value })
}
fn generic(b: &Backend<Bus>, n: u64, mutation: ott::Mutation) -> Request {
    signed(Request::OttCommit { expected:expected(b), operation_id:format!("{n:032x}"), digest:String::new(), mutation })
}
fn legacy(namespace: &str) -> SecretString {
    // Deliberate whitespace survives migration exactly, rather than being reserialized.
    SecretString(format!("{{ \"version\": 1, \"sealed\": {{\"backend\":\"keymanager3\",\"key\":\"{APP}.ott.{namespace}.v1\",\"iv\":\"aXZpdmlyaXZpdmly\",\"data\":\"bGVnYWN5\"}} }}"))
}
fn verified(response: Response) {
    assert!(matches!(response, Response::OttControlCommit { status:CommitStatus::Committed,
        receipt:Some(ott::Receipt { verified:true, .. }) }));
}
fn source(b: &Backend<Bus>) -> control::Source {
    let state = stored(b);
    let text = state.control.as_deref().unwrap();
    let sha256 = state::digest_bytes(text.as_bytes());
    if control::is_protected(text) { control::Source::Protected { sha256 } }
    else { control::Source::Legacy { sha256 } }
}

#[test]
fn typed_control_writes_only_ciphertext_and_replay_has_no_crypto_or_put() {
    let mut b = backend();
    let value = config('a');
    let r = request(&b, 1, control::Source::Absent {}, value.clone());
    verified(b.dispatch(r.clone()));
    let text = stored(&b).control.unwrap();
    let envelope = control::Envelope::parse(&text, APP).unwrap();
    assert!(envelope.legacy.is_none());
    let disk = b.rpc.object.as_ref().unwrap().to_string();
    assert!(!disk.contains(&value.token.0));
    assert!(!disk.contains("home.example"));
    assert!(!disk.contains("dev_fixture"));
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::OttControlCommit { status:CommitStatus::Committed,
        receipt:Some(ott::Receipt { verified:false, .. }) }));
    assert_eq!(b.rpc.calls.len(), before + 1);
    assert_eq!(b.rpc.calls.last().unwrap().0, "luna://com.palm.db/get");
    assert!(matches!(b.dispatch(Request::OttControlLoad {}), Response::OttControlLoaded {
        value:control::Loaded::Protected { value:actual, .. }, .. } if actual == value));
}

#[test]
fn legacy_bytes_survive_migration_and_subsequent_updates_and_generic_state_edits() {
    let mut b = backend();
    let old = legacy("control");
    let state = legacy("state");
    let r = generic(&b, 1, ott::Mutation::Initialize { control:Some(old.clone()), state:Some(state.clone()), barrier:ott::Barrier::Open });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    assert!(matches!(b.dispatch(Request::OttControlLoad {}), Response::OttControlLoaded {
        value:control::Loaded::Legacy { envelope }, .. } if envelope.0 == old.0));
    let r = request(&b, 2, source(&b), config('a')); verified(b.dispatch(r));
    let first = stored(&b).control.unwrap();
    assert_eq!(control::Envelope::parse(&first, APP).unwrap().legacy.unwrap().0, old.0);
    let r = request(&b, 3, source(&b), config('b')); verified(b.dispatch(r));
    let second = stored(&b).control.unwrap();
    assert_ne!(first, second);
    assert_eq!(control::Envelope::parse(&second, APP).unwrap().legacy.unwrap().0, old.0);
    let r = generic(&b, 4, ott::Mutation::Put { namespace:ott::Namespace::State, value:state });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    let r = generic(&b, 5, ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    assert_eq!(stored(&b).control.as_deref(), Some(second.as_str()));
}

#[test]
fn wrong_source_stale_revision_and_bad_digest_are_refused_before_crypto() {
    let mut b = backend();
    let r = request(&b, 1, control::Source::Absent {}, config('a')); verified(b.dispatch(r));
    for candidate_source in [control::Source::Absent {}, control::Source::Legacy { sha256:"0".repeat(64) },
        control::Source::Protected { sha256:"0".repeat(64) }] {
        let r = request(&b, 2, candidate_source, config('b'));
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::OttControlCommit { status:CommitStatus::Conflict, .. }));
        assert_eq!(b.rpc.calls.len(), before + 1);
    }
    let mut r = request(&b, 2, source(&b), config('b'));
    if let Request::OttControlCommit { expected, .. } = &mut r { *expected = ott::Expected::Missing {}; }
    r = signed(r);
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::OttControlCommit { status:CommitStatus::Conflict, .. }));
    assert_eq!(b.rpc.calls.len(), before + 1);
    let mut r = request(&b, 2, source(&b), config('b'));
    if let Request::OttControlCommit { digest, .. } = &mut r { *digest = "0".repeat(64); }
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert_eq!(b.rpc.calls.len(), before);
}

#[test]
fn generic_control_paths_cannot_inject_or_downgrade_v2() {
    let mut b = backend();
    let r = request(&b, 1, control::Source::Absent {}, config('a')); verified(b.dispatch(r));
    let v2 = stored(&b).control.unwrap();
    for mutation in [
        ott::Mutation::Put { namespace:ott::Namespace::Control, value:legacy("control") },
        ott::Mutation::Put { namespace:ott::Namespace::Control, value:SecretString(v2.clone()) },
    ] {
        let r = generic(&b, 2, mutation);
        assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
        assert_eq!(stored(&b).control.as_deref(), Some(v2.as_str()));
    }
    let mut empty = backend();
    let r = generic(&empty, 1, ott::Mutation::Initialize { control:Some(SecretString(v2.clone())), state:None, barrier:ott::Barrier::Open });
    assert!(matches!(empty.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert!(empty.rpc.object.is_none());
}

#[test]
fn protected_key_denial_never_falls_back_to_plaintext_or_modifies_existing_state() {
    let mut b = backend();
    b.rpc.deny_generate = true;
    let r = request(&b, 1, control::Source::Absent {}, config('a'));
    assert!(matches!(b.dispatch(r), Response::KeymanagerError { failure, .. }
        if failure.service_code == Some(-1000)));
    assert!(b.rpc.object.is_none());
    assert!(!b.rpc.calls.iter().any(|(uri,_)| uri.ends_with("/put") || uri.ends_with("/removeKey")));
    b.rpc.deny_generate = false;
    let r = request(&b, 2, control::Source::Absent {}, config('a')); verified(b.dispatch(r));
    let original = b.rpc.object.clone();
    b.rpc.deny_decrypt = true;
    let before = b.rpc.calls.len();
    let r = request(&b, 3, source(&b), config('b'));
    assert!(matches!(b.dispatch(r), Response::KeymanagerError { .. }));
    assert_eq!(b.rpc.object, original);
    assert!(!b.rpc.calls[before..].iter().any(|(uri,_)| uri.ends_with("/generateKey") || uri.ends_with("/put")));
}

#[test]
fn lost_put_ack_can_verify_but_wrong_or_unreadable_decryption_is_uncertain() {
    let mut b = backend(); b.rpc.lose_ack = true;
    let r = request(&b, 1, control::Source::Absent {}, config('a')); verified(b.dispatch(r));
    for mismatch in [false, true] {
        let mut b = backend();
        b.rpc.fail_postwrite_decrypt = !mismatch;
        b.rpc.wrong_postwrite_plaintext = mismatch;
        let r = request(&b, 1, control::Source::Absent {}, config('a'));
        assert!(matches!(b.dispatch(r.clone()), Response::OttControlCommit { status:CommitStatus::Unavailable, receipt:None }));
        assert!(b.rpc.object.is_some());
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::OttControlCommit {
            status:CommitStatus::Committed, receipt:Some(ott::Receipt { verified:false, .. }) }));
        assert_eq!(b.rpc.calls.len(), before + 1);
    }
}

#[test]
fn changed_readback_candidate_never_earns_a_verified_receipt() {
    let mut b = backend(); b.rpc.alter_after_put = true;
    let r = request(&b, 1, control::Source::Absent {}, config('a'));
    assert!(matches!(b.dispatch(r), Response::OttControlCommit {
        status:CommitStatus::Committed, receipt:Some(ott::Receipt { verified:false, .. }) }));
}

#[test]
fn changed_preserved_legacy_bytes_invalidate_the_authenticated_context() {
    let mut b = backend();
    let r = generic(&b, 1, ott::Mutation::Initialize {
        control:Some(legacy("control")), state:None, barrier:ott::Barrier::Open });
    b.dispatch(r);
    let r = request(&b, 2, source(&b), config('a')); verified(b.dispatch(r));
    let object = b.rpc.object.as_mut().unwrap();
    let mut state: Value = serde_json::from_str(object["state"].as_str().unwrap()).unwrap();
    let mut envelope: Value = serde_json::from_str(state["control"].as_str().unwrap()).unwrap();
    envelope["legacy"] = json!(legacy("control").0 + " ");
    state["control"] = json!(serde_json::to_string(&envelope).unwrap());
    object["state"] = json!(serde_json::to_string(&state).unwrap());
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(Request::OttControlLoad {}), Response::Error { code:ErrorCode::Corrupt }));
    assert_eq!(b.rpc.calls.len(), before + 1);
}

#[test]
fn pending_erasure_and_incomplete_private_import_refuse_control_writes() {
    for mutation in [ott::Mutation::BeginErase {},
        ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) }] {
        let mut b = backend();
        let r = generic(&b, 1, mutation);
        assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
        let original = b.rpc.object.clone();
        let r = request(&b, 2, control::Source::Absent {}, config('a'));
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
        assert_eq!(b.rpc.calls.len(), before + 1);
        assert_eq!(b.rpc.object, original);
    }
}

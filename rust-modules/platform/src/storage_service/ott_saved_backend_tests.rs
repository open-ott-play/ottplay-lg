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
                assert_eq!(payload["name"], saved::KEY_NAME);
                if self.deny_generate { return Ok(json!({"returnValue":false,"errorCode":-1000,"errorText":"private-rejection"})); }
                if self.key_exists { return Ok(json!({"returnValue":false,"errorCode":-10002})); }
                self.key_exists = true;
                Ok(json!({"returnValue":true}))
            }
            "luna://com.webos.service.keymanager3/begin" => {
                assert_eq!(payload["name"], saved::KEY_NAME);
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
fn config(token: char) -> saved::Document {
    saved::Document::try_from(json!({"version":1,"settings":{"fixtureToken":token.to_string().repeat(32),
        "nativeCompanion":{"enabled":true,"cacheBytes":268435456},"nativeCompanionRevision":1},"favorites_selected":true})).unwrap()
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
        Request::OttStateCommit { digest: field, .. } | Request::OttCommit { digest: field, .. } => *field = digest,
        _ => panic!("unsigned request"),
    }
    request
}
fn request(b: &Backend<Bus>, n: u64, source: saved::Source, value: saved::Document) -> Request {
    signed(Request::OttStateCommit { expected:expected(b), operation_id:format!("{n:032x}"),
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
    assert!(matches!(response, Response::OttStateCommit { status:CommitStatus::Committed,
        receipt:Some(ott::Receipt { verified:true, .. }) }));
}
fn source(b: &Backend<Bus>) -> saved::Source {
    let state = stored(b);
    let text = state.state.as_deref().unwrap();
    let sha256 = state::digest_bytes(text.as_bytes());
    if saved::is_protected(text) { saved::Source::Protected { sha256 } }
    else { saved::Source::Legacy { sha256 } }
}

#[test]
fn typed_state_writes_only_ciphertext_and_replay_has_no_crypto_or_put() {
    let mut b = backend();
    let value = config('a');
    let r = request(&b, 1, saved::Source::Absent {}, value.clone());
    verified(b.dispatch(r.clone()));
    let text = stored(&b).state.unwrap();
    let envelope = saved::Envelope::parse(&text, APP).unwrap();
    assert!(envelope.legacy.is_none());
    let disk = b.rpc.object.as_ref().unwrap().to_string();
    assert!(!disk.contains(value.as_ref()["settings"]["fixtureToken"].as_str().unwrap()));
    assert!(!disk.contains("nativeCompanion"));
    assert!(!disk.contains("fixtureToken"));
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::OttStateCommit { status:CommitStatus::Committed,
        receipt:Some(ott::Receipt { verified:false, .. }) }));
    assert_eq!(b.rpc.calls.len(), before + 1);
    assert_eq!(b.rpc.calls.last().unwrap().0, "luna://com.palm.db/get");
    assert!(matches!(b.dispatch(Request::OttStateLoad {}), Response::OttStateLoaded {
        value:saved::Loaded::Protected { value:actual, .. }, .. } if actual == value));
}

#[test]
fn legacy_bytes_survive_migration_and_subsequent_updates_and_control_edits() {
    let mut b = backend();
    let old = legacy("state");
    let control = legacy("control");
    let r = generic(&b, 1, ott::Mutation::Initialize { control:Some(control.clone()), state:Some(old.clone()), barrier:ott::Barrier::Open });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    assert!(matches!(b.dispatch(Request::OttStateLoad {}), Response::OttStateLoaded {
        value:saved::Loaded::Legacy { envelope }, .. } if envelope.0 == old.0));
    let r = request(&b, 2, source(&b), config('a')); verified(b.dispatch(r));
    let first = stored(&b).state.unwrap();
    assert_eq!(saved::Envelope::parse(&first, APP).unwrap().legacy.unwrap().0, old.0);
    let r = request(&b, 3, source(&b), config('b')); verified(b.dispatch(r));
    let second = stored(&b).state.unwrap();
    assert_ne!(first, second);
    assert_eq!(saved::Envelope::parse(&second, APP).unwrap().legacy.unwrap().0, old.0);
    let r = generic(&b, 4, ott::Mutation::Put { namespace:ott::Namespace::Control, value:control.clone() });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    let r = generic(&b, 5, ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) });
    assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    assert_eq!(stored(&b).state.as_deref(), Some(second.as_str()));
    assert_eq!(stored(&b).control.as_deref(), Some(control.0.as_str()));
}

#[test]
fn wrong_source_stale_revision_and_bad_digest_are_refused_before_crypto() {
    let mut b = backend();
    let r = request(&b, 1, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
    for candidate_source in [saved::Source::Absent {}, saved::Source::Legacy { sha256:"0".repeat(64) },
        saved::Source::Protected { sha256:"0".repeat(64) }] {
        let r = request(&b, 2, candidate_source, config('b'));
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::OttStateCommit { status:CommitStatus::Conflict, .. }));
        assert_eq!(b.rpc.calls.len(), before + 1);
    }
    let mut r = request(&b, 2, source(&b), config('b'));
    if let Request::OttStateCommit { expected, .. } = &mut r { *expected = ott::Expected::Missing {}; }
    r = signed(r);
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::OttStateCommit { status:CommitStatus::Conflict, .. }));
    assert_eq!(b.rpc.calls.len(), before + 1);
    let mut r = request(&b, 2, source(&b), config('b'));
    if let Request::OttStateCommit { digest, .. } = &mut r { *digest = "0".repeat(64); }
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert_eq!(b.rpc.calls.len(), before);
}

#[test]
fn generic_state_paths_cannot_inject_or_downgrade_v2() {
    let mut b = backend();
    let r = request(&b, 1, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
    let v2 = stored(&b).state.unwrap();
    for mutation in [
        ott::Mutation::Put { namespace:ott::Namespace::State, value:legacy("state") },
        ott::Mutation::Put { namespace:ott::Namespace::State, value:SecretString(v2.clone()) },
    ] {
        let r = generic(&b, 2, mutation);
        assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
        assert_eq!(stored(&b).state.as_deref(), Some(v2.as_str()));
    }
    let mut empty = backend();
    let r = generic(&empty, 1, ott::Mutation::Initialize { control:None, state:Some(SecretString(v2.clone())), barrier:ott::Barrier::Open });
    assert!(matches!(empty.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert!(empty.rpc.object.is_none());
}

#[test]
fn protected_key_denial_never_falls_back_to_plaintext_or_modifies_existing_state() {
    let mut b = backend();
    b.rpc.deny_generate = true;
    let r = request(&b, 1, saved::Source::Absent {}, config('a'));
    assert!(matches!(b.dispatch(r), Response::KeymanagerError { failure, .. }
        if failure.service_code == Some(-1000)));
    assert!(b.rpc.object.is_none());
    assert!(!b.rpc.calls.iter().any(|(uri,_)| uri.ends_with("/put") || uri.ends_with("/removeKey")));
    b.rpc.deny_generate = false;
    let r = request(&b, 2, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
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
    let r = request(&b, 1, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
    for mismatch in [false, true] {
        let mut b = backend();
        b.rpc.fail_postwrite_decrypt = !mismatch;
        b.rpc.wrong_postwrite_plaintext = mismatch;
        let r = request(&b, 1, saved::Source::Absent {}, config('a'));
        assert!(matches!(b.dispatch(r.clone()), Response::OttStateCommit { status:CommitStatus::Unavailable, receipt:None }));
        assert!(b.rpc.object.is_some());
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::OttStateCommit {
            status:CommitStatus::Committed, receipt:Some(ott::Receipt { verified:false, .. }) }));
        assert_eq!(b.rpc.calls.len(), before + 1);
    }
}

#[test]
fn changed_readback_candidate_never_earns_a_verified_receipt() {
    let mut b = backend(); b.rpc.alter_after_put = true;
    let r = request(&b, 1, saved::Source::Absent {}, config('a'));
    assert!(matches!(b.dispatch(r), Response::OttStateCommit {
        status:CommitStatus::Committed, receipt:Some(ott::Receipt { verified:false, .. }) }));
}

#[test]
fn changed_preserved_legacy_bytes_invalidate_the_authenticated_context() {
    let mut b = backend();
    let r = generic(&b, 1, ott::Mutation::Initialize {
        control:None, state:Some(legacy("state")), barrier:ott::Barrier::Open });
    b.dispatch(r);
    let r = request(&b, 2, source(&b), config('a')); verified(b.dispatch(r));
    let object = b.rpc.object.as_mut().unwrap();
    let mut state: Value = serde_json::from_str(object["state"].as_str().unwrap()).unwrap();
    let mut envelope: Value = serde_json::from_str(state["state"].as_str().unwrap()).unwrap();
    envelope["legacy"] = json!(legacy("state").0 + " ");
    state["state"] = json!(serde_json::to_string(&envelope).unwrap());
    object["state"] = json!(serde_json::to_string(&state).unwrap());
    let before = b.rpc.calls.len();
    assert!(matches!(b.dispatch(Request::OttStateLoad {}), Response::Error { code:ErrorCode::Corrupt }));
    assert_eq!(b.rpc.calls.len(), before + 1);
}

#[test]
fn pending_erasure_and_incomplete_private_import_refuse_state_writes() {
    for mutation in [ott::Mutation::BeginErase {},
        ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) }] {
        let mut b = backend();
        let r = generic(&b, 1, mutation);
        assert!(matches!(b.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
        let original = b.rpc.object.clone();
        let r = request(&b, 2, saved::Source::Absent {}, config('a'));
        let before = b.rpc.calls.len();
        assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
        assert_eq!(b.rpc.calls.len(), before + 1);
        assert_eq!(b.rpc.object, original);
    }
}

#[test]
fn inspection_opens_only_stored_state_without_setup_write_or_diagnostic_changes() {
    let mut b = backend();
    let value = config('a');
    let r = request(&b, 1, saved::Source::Absent {}, value.clone()); verified(b.dispatch(r));
    let prior = keymanager::failure(wire::KeymanagerOperation::Seal,
        wire::KeymanagerStage::Generate, ErrorCode::Capability);
    b.keymanager_error = Some((prior, AuthPreservation::Unchanged));
    let before = b.rpc.calls.len();
    assert!(matches!(b.inspect(Request::OttStateLoad {}), Response::OttStateLoaded {
        value:saved::Loaded::Protected { value:actual, .. }, .. } if actual == value));
    assert_eq!(b.keymanager_error, Some((prior, AuthPreservation::Unchanged)));
    assert_eq!(b.rpc.calls[before..].iter().map(|(uri,_)|uri.as_str()).collect::<Vec<_>>(),
        ["luna://com.palm.db/get", "luna://com.webos.service.keymanager3/begin",
            "luna://com.webos.service.keymanager3/finish"]);
    b.rpc.deny_decrypt = true;
    let original = b.rpc.object.clone();
    assert!(matches!(b.inspect(Request::OttStateLoad {}), Response::Error { code:ErrorCode::Capability }));
    assert_eq!(b.keymanager_error, Some((prior, AuthPreservation::Unchanged)));
    assert_eq!(b.rpc.object, original);
}

#[test]
fn state_key_cannot_open_a_control_envelope_or_another_install() {
    let mut b = backend();
    let r = request(&b, 1, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
    let encoded = stored(&b).state.unwrap();
    assert!(saved::Envelope::parse(&encoded, "com.openottplay.player.native.debug").is_err());
    let mut envelope = saved::Envelope::parse(&encoded, APP).unwrap();
    let mut cipher: Value = serde_json::from_str(&envelope.cipher.0).unwrap();
    cipher["key_name"] = json!(wire::control::KEY_NAME);
    envelope.cipher.0 = serde_json::to_string(&cipher).unwrap();
    assert!(envelope.encode().is_err());
    assert!(wire::control::Envelope::parse(&encoded, APP).is_err());
}

#[test]
fn public_only_import_rejects_caller_supplied_v2_state_without_writing() {
    let mut b = backend();
    let r = request(&b, 1, saved::Source::Absent {}, config('a')); verified(b.dispatch(r));
    let encoded = stored(&b).state.unwrap();
    let mut empty = backend();
    let r = generic(&empty, 1, ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) });
    assert!(matches!(empty.dispatch(r), Response::OttCommit { status:CommitStatus::Committed, .. }));
    let original = empty.rpc.object.clone();
    let r = generic(&empty, 2, ott::Mutation::CompletePrivateImport { control:None,
        state:Some(SecretString(encoded)), barrier:ott::Barrier::Open });
    assert!(matches!(empty.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert_eq!(empty.rpc.object, original);
}

#[test]
fn invalid_document_is_refused_before_database_or_keymanager_access() {
    let mut b = backend();
    let malformed: saved::Document = serde_json::from_value(json!({"arbitrary":"secret"})).unwrap();
    let r = request(&b, 1, saved::Source::Absent {}, malformed);
    assert!(matches!(b.dispatch(r), Response::Error { code:ErrorCode::Invalid }));
    assert!(b.rpc.calls.is_empty());
}

//! Typed OTT State persistence; never accepts caller-selected keys or ciphertext to open.
use super::*;
use super::super::keymanager;
use wire::{saved, AuthPreservation};

fn install<R: Rpc>(backend: &Backend<R>) -> Result<&str, ErrorCode> {
    backend.service.strip_suffix(".storage")
        .filter(|app| Flavor::from_app_id(app) == Some(backend.flavor))
        .ok_or(ErrorCode::Invalid)
}

fn protected_open<R: Rpc>(backend: &mut Backend<R>, text: &str)
    -> Result<(saved::Envelope, saved::Document), ErrorCode>
{
    let envelope = saved::Envelope::parse(text, install(backend)?)?;
    let plain = keymanager::open_saved_detailed(&mut backend.rpc, &envelope.cipher.0, &envelope.context())
        .map_err(|failure| {
            backend.keymanager_error = Some((failure, AuthPreservation::Unchanged));
            failure.code
        })?;
    let value: saved::Document = serde_json::from_str(&plain).map_err(|_| ErrorCode::Corrupt)?;
    value.validate().map_err(|_| ErrorCode::Corrupt)?;
    Ok((envelope, value))
}

fn response(status: CommitStatus, receipt: Option<ott::Receipt>) -> Response {
    Response::OttStateCommit { status, receipt }
}

pub(super) fn dispatch<R: Rpc>(backend: &mut Backend<R>, request: Request) -> Result<Response, ErrorCode> {
    match request {
        Request::OttStateLoad {} => {
            let current = load(backend)?;
            if current.as_ref().is_some_and(|(_, state)| state.barrier == ott::Barrier::Pending) {
                return Err(ErrorCode::Unavailable);
            }
            let value = match current.as_ref().and_then(|(_, state)| state.state.as_deref()) {
                None => saved::Loaded::Missing {},
                Some(text) if saved::is_protected(text) => {
                    let (_, value) = protected_open(backend, text)?;
                    saved::Loaded::Protected { value, envelope_sha256: state::digest_bytes(text.as_bytes()) }
                }
                Some(text) => saved::Loaded::Legacy { envelope: SecretString(text.to_owned()) },
            };
            Ok(Response::OttStateLoaded { status: current.map(|(rev, state)| state.status(rev)), value })
        }
        request @ Request::OttStateCommit { .. } => commit(backend, request),
        _ => Err(ErrorCode::Protocol),
    }
}

fn commit<R: Rpc>(backend: &mut Backend<R>, request: Request) -> Result<Response, ErrorCode> {
    let bytes = wire::request_digest_bytes(&request)?;
    let Request::OttStateCommit { expected, operation_id, digest, source, value } = request else {
        return Err(ErrorCode::Protocol);
    };
    value.validate()?;
    source.validate()?;
    if !model::valid_hex(&operation_id, 32) || !model::valid_hex(&digest, 64)
        || state::digest_bytes(&bytes) != digest
    { return Err(ErrorCode::Invalid); }
    let current = load(backend)?;
    if let Some((_, state)) = &current {
        if let Some(receipt) = state.receipt(&operation_id, &digest, false)? {
            return Ok(response(CommitStatus::Committed, Some(receipt)));
        }
    }
    if !expected_matches(&expected, current.as_ref()) {
        return Ok(response(CommitStatus::Conflict, None));
    }
    if current.as_ref().is_some_and(|(_, state)| !state.private_initialized
        || state.barrier == ott::Barrier::Pending || state.revision == u64::MAX) {
        return Err(ErrorCode::Invalid);
    }
    // Only the value loaded from the canonical object may reach the decrypt operation.
    let legacy = match (&source, current.as_ref().and_then(|(_, state)| state.state.as_deref())) {
        (saved::Source::Absent {}, None) => None,
        (saved::Source::Legacy { sha256 }, Some(text))
            if !saved::is_protected(text) && *sha256 == state::digest_bytes(text.as_bytes()) => {
                Some(SecretString(text.to_owned()))
            }
        (saved::Source::Protected { sha256 }, Some(text))
            if saved::is_protected(text) && *sha256 == state::digest_bytes(text.as_bytes()) => {
                // Do not replace an already protected value that cannot be authenticated.
                protected_open(backend, text)?.0.legacy
            }
        _ => return Ok(response(CommitStatus::Conflict, None)),
    };
    let mut envelope = saved::Envelope {
        version: 2, install: install(backend)?.to_owned(), namespace: "state".into(),
        cipher: SecretString(String::new()), legacy,
    };
    let plain = serde_json::to_string(&value).map_err(|_| ErrorCode::Invalid)?;
    envelope.cipher = SecretString(keymanager::seal_saved_detailed(&mut backend.rpc, &plain, envelope.context())
        .map_err(|failure| {
            backend.keymanager_error = Some((failure, AuthPreservation::Unchanged));
            failure.code
        })?);
    let candidate = State::protected_candidate(current.as_ref().map(|(_, state)| state), backend.flavor, ott::Namespace::State,
        envelope.encode()?, &operation_id, &digest)?;
    let mut object = json!({"_id":model::object_id(backend.flavor),
        "_kind":format!("{}:1",backend.service),"state":candidate.encode()?});
    if let Some((rev, _)) = &current { object["_rev"] = json!(rev); }
    let payload = checked_put_payload(object)?;
    let result = backend.rpc.call("luna://com.palm.db/put", &payload);
    record_put_failure(&result);

    // Every result after this point may have published data: retain uncertainty on failure.
    let actual = match load(backend) {
        Ok(actual) => actual,
        Err(_) => return Ok(response(CommitStatus::Unavailable, None)),
    };
    let expectation_changed = !expected_matches(&expected, actual.as_ref());
    if let Some((_, stored)) = actual {
        if stored == candidate {
            let Some(text) = stored.state.as_deref() else {
                return Ok(response(CommitStatus::Unavailable, None));
            };
            if !matches!(protected_open(backend, text), Ok((_, opened)) if opened == value) {
                return Ok(response(CommitStatus::Unavailable, None));
            }
            return Ok(response(CommitStatus::Committed, stored.receipt(&operation_id, &digest, true)?));
        }
        match stored.receipt(&operation_id, &digest, false) {
            Ok(Some(receipt)) => return Ok(response(CommitStatus::Committed, Some(receipt))),
            Err(_) => return Ok(response(CommitStatus::Unavailable, None)),
            Ok(None) => (),
        }
    }
    if expectation_changed && matches!(result, Ok(ref reply) if reply["returnValue"] == false) {
        Ok(response(CommitStatus::Conflict, None))
    } else {
        Ok(response(CommitStatus::Unavailable, None))
    }
}

#[cfg(test)]
#[path = "ott_saved_backend_tests.rs"]
mod tests;

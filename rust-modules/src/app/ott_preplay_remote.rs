//! Opt-in, read-only preparation evidence. No source values or producer text cross this boundary.
use super::{preplay, input_policy, Invocation, Runtime};
use serde_json::{json, Value};

pub(super) const ACTION: &str = "playback_preparation_diagnostics";

pub(super) fn capability() -> Value { json!({"version":1}) }

fn valid_runtime(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn validate(params: &Value, runtime: &str) -> Result<(), &'static str> {
    let object = params.as_object().ok_or("invalid_request")?;
    if object.len() != 2 || object.keys().any(|key| !["version", "runtime"].contains(&key.as_str()))
        || params["version"].as_u64() != Some(1)
    { return Err("invalid_request"); }
    let requested = params["runtime"].as_str().filter(|value| valid_runtime(value))
        .ok_or("invalid_request")?;
    if requested != runtime { return Err("runtime_mismatch"); }
    Ok(())
}

pub(super) fn failure(runtime: &str, code: &'static str) -> Value {
    json!({"status":"rejected","data":{"version":1,"runtime":runtime,"error":code}})
}

fn retry(pending: Option<super::continuity::PendingRetry<'_>>, generation: u64, now_ms: u64) -> Value {
    match pending.filter(|pending| pending.generation == generation) {
        Some(pending) => json!({"pending":true,
            "failures":pending.failures.min(u32::MAX as usize),
            "retry_after_ms":pending.not_before_ms.saturating_sub(now_ms).min(30_000)}),
        None => json!({"pending":false,"failures":null,"retry_after_ms":null}),
    }
}

pub(super) fn command(runtime: &mut Runtime, inv: &Invocation) -> Value {
    let token = runtime.remote.runtime().to_owned();
    if let Err(code) = validate(&inv.params, &token) { return failure(&token, code); }
    if runtime.now_ms() >= inv.deadline_ms { return failure(&token, "invalid_request"); }
    if !runtime.remote.invocation_is_current(inv) { return failure(&token, "unavailable"); }
    if input_policy::remote(runtime, inv).is_err() { return failure(&token, "policy_denied"); }
    let mut data = serde_json::to_value(preplay::diagnostic(runtime))
        .expect("closed preparation diagnostic is serializable");
    data["version"] = json!(1);
    data["runtime"] = json!(token);
    data["retry"] = retry(runtime.continuity.pending_retry(), runtime.generation, runtime.now_ms());
    json!({"status":"ok","data":data})
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_preplay_remote_tests.rs"]
mod tests;

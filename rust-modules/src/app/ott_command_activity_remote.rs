//! Opt-in remote ingress correlation; no arguments, raw labels or sender attribution.
use super::{input_policy, Invocation, Runtime};
use serde_json::{json, Value};

pub(super) const ACTION: &str = "command_activity_diagnostics";

pub(super) fn capability() -> Value { json!({"version":2}) }

fn valid_runtime(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn validate(params: &Value, runtime: &str) -> Result<(), &'static str> {
    let object = params.as_object().ok_or("invalid_request")?;
    if object.len() != 2 || object.keys().any(|key| !["version", "runtime"].contains(&key.as_str()))
        || params["version"].as_u64() != Some(2)
    { return Err("invalid_request"); }
    let requested = params["runtime"].as_str().filter(|value| valid_runtime(value))
        .ok_or("invalid_request")?;
    if requested != runtime { return Err("runtime_mismatch"); }
    Ok(())
}

pub(super) fn failure(runtime: &str, code: &'static str) -> Value {
    json!({"status":"rejected","data":{"version":2,"runtime":runtime,"error":code}})
}

pub(super) fn command(runtime: &mut Runtime, inv: &Invocation) -> Value {
    let token = runtime.remote.runtime().to_owned();
    if let Err(code) = validate(&inv.params, &token) { return failure(&token, code); }
    if runtime.now_ms() >= inv.deadline_ms { return failure(&token, "invalid_request"); }
    if !runtime.remote.invocation_is_current(inv) { return failure(&token, "unavailable"); }
    if input_policy::remote(runtime, inv).is_err() { return failure(&token, "policy_denied"); }
    let Some(data) = runtime.remote.command_activity_v2(inv, runtime.now_ms()) else {
        return failure(&token, "unavailable");
    };
    json!({"status":"ok","data":data})
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_command_activity_remote_tests.rs"]
mod tests;

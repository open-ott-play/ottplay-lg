//! Installation-bound compositor settings. Public receipts contain metadata only.
use super::{Saved, Value};
use serde_json::json;
use crate::app::ott_runtime::multiview_service::ServiceBinding;

pub(super) const KEY: &str = "nativeMultiviewService";
const MAX_BYTES: usize = 4096;

fn parse(value: &Value) -> Result<Option<ServiceBinding>, &'static str> {
    let raw = value.as_str().filter(|raw| raw.len() <= MAX_BYTES
        && !raw.chars().any(char::is_control)).ok_or("invalid_value")?;
    ServiceBinding::from_protected_json(raw).map_err(|_| "invalid_value")
}

pub(super) fn valid(value: &Value) -> bool { parse(value).is_ok() }

pub(super) fn candidate(saved: &Saved, value: &Value, installation: Option<&str>)
    -> Result<Saved, &'static str>
{
    let stored = if let Some(binding) = parse(value)? {
        binding.bind_for_installation(installation.ok_or("invalid_value")?)
            .map_err(|_| "invalid_value")?;
        Value::String(binding.protected_json().map_err(|_| "invalid_value")?)
    } else {
        Value::String(String::new())
    };
    let mut candidate = saved.clone();
    candidate.settings.insert(KEY.into(), stored);
    Ok(candidate)
}

pub(super) fn snapshot(saved: &Saved) -> Value {
    let raw = saved.settings.get(KEY);
    let parsed = raw.map(parse);
    json!({"id":KEY,"redacted":true,
        "configured":matches!(parsed.as_ref(), Some(Ok(Some(_)))),
        "storage":if raw.is_none() {"default"} else if matches!(parsed.as_ref(), Some(Ok(_))) {"stored"} else {"invalid"}})
}

pub(super) fn descriptor() -> Value {
    json!({"id":KEY,"type":"string","scope":"application","sensitive":true,
        "access":{"read":true,"write":true},"constraints":{"max_utf8_bytes":MAX_BYTES},
        "apply":"next_stream",
        "description":"Private installation-bound compositor configuration. Empty disables it. A compact JSON string is required; reads return only redacted metadata. Changes cancel an active multi-view session; the next explicit session uses the accepted configuration."})
}

//! One installation receive policy for text and camera notifications.
//! The public preference CAS is independent of provider/profile exports.
use super::*;

pub(super) fn failure(params: &Value, code: &str) -> Value {
    json!({"status":"rejected","data":{"version":1,
        "runtime":params["runtime"].as_str().filter(|value| valid_runtime(value)).unwrap_or(""),
        "operation":params["operation"].as_str().filter(|value| matches!(*value,"get"|"set")),
        "error":code}})
}
fn valid_runtime(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
fn parse(params: &Value, runtime: &str) -> Result<Option<bool>, &'static str> {
    let object = params.as_object().ok_or("invalid_request")?;
    let set = match params["operation"].as_str() {
        Some("get") => false,
        Some("set") => true,
        _ => return Err("invalid_request"),
    };
    if params["version"] != 1
        || object.len() != if set { 4 } else { 3 }
        || !object.keys().all(|key| {
            matches!(key.as_str(), "version" | "operation" | "runtime") || (set && key == "enabled")
        })
        || !params["runtime"].as_str().is_some_and(valid_runtime)
    {
        return Err("invalid_request");
    }
    if params["runtime"] != runtime {
        return Err("runtime_mismatch");
    }
    if set {
        params["enabled"]
            .as_bool()
            .map(Some)
            .ok_or("invalid_request")
    } else {
        Ok(None)
    }
}
pub(super) fn command(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    let next = match parse(&inv.params, runtime.remote.runtime()) {
        Ok(next) => next,
        Err(code) => return Some(failure(&inv.params, code)),
    };
    if runtime.now_ms() >= inv.deadline_ms {
        return Some(failure(&inv.params, "invalid_request"));
    }
    let Some(enabled) = public_preferences::notifications_enabled(&runtime.public_preferences)
    else {
        return Some(failure(&inv.params, "storage_unavailable"));
    };
    let response = ok(json!({"version":1,"runtime":runtime.remote.runtime(),
        "operation":if next.is_some() {"set"} else {"get"},"enabled":next.unwrap_or(enabled),"persisted":true}));
    let Some(next) = next else {
        return Some(response);
    };
    // Candidate is unchanged: a public installation policy must never enter Saved.
    match runtime.save_public_preference(
        runtime.saved.clone(),
        public_preferences::Patch::NotificationsEnabled(next),
        Some(inv.clone()),
        response,
    ) {
        Ok(()) => None,
        Err(_) => Some(failure(&inv.params, "storage_unavailable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_notifications_v1_fixture_keeps_requests_and_receipts_compatible() {
        let fixture: Value =
            serde_json::from_str(include_str!("ott_remote/notifications-v1.json")).unwrap();
        assert_eq!(
            parse(&fixture["get_request"]["params"], "page-123"),
            Ok(None)
        );
        assert_eq!(
            parse(&fixture["set_request"]["params"], "page-123"),
            Ok(Some(false))
        );
        assert_eq!(
            failure(&fixture["set_request"]["params"], "storage_unavailable")["data"],
            fixture["error_result"]
        );
    }
    #[test]
    fn notification_preference_parser_requires_exact_current_runtime_and_boolean() {
        let get = json!({"version":1,"runtime":"native-test","operation":"get"});
        assert_eq!(parse(&get, "native-test"), Ok(None));
        assert_eq!(parse(&get, "replacement"), Err("runtime_mismatch"));
        let mut set =
            json!({"version":1,"runtime":"native-test","operation":"set","enabled":false});
        assert_eq!(parse(&set, "native-test"), Ok(Some(false)));
        set["enabled"] = json!(0);
        assert_eq!(parse(&set, "native-test"), Err("invalid_request"));
        set["enabled"] = json!(true);
        set["profile"] = json!("unrelated");
        assert_eq!(parse(&set, "native-test"), Err("invalid_request"));
        assert_eq!(
            failure(&get, "runtime_mismatch")["data"]["runtime"],
            "native-test"
        );
    }
}

//! Closed wire vocabulary. Values here never identify filesystem objects.
use serde_json::{json, Value};
pub(super) const MAX_ROWS: usize = 64;
pub(super) const MAX_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Invalid,
    Unsupported,
    Busy,
    Stale,
    Changed,
    Unavailable,
    Restricted,
    Expired,
    Cancelled,
    Limit,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_request",
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::Stale => "stale",
            Self::Changed => "source_changed",
            Self::Unavailable => "unavailable",
            Self::Restricted => "restricted",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Limit => "limit_exceeded",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Roots,
    Entry(String),
    Cursor(String),
    More(String),
    Select(String),
}
impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Roots => "roots",
            Self::Entry(_) | Self::Cursor(_) => "list",
            Self::More(_) => "more",
            Self::Select(_) => "select",
        }
    }
}
#[derive(Clone)]
pub(super) struct Request {
    pub runtime: String,
    pub generation: Option<String>,
    pub op: Op,
}
pub(super) fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
pub(super) fn name(s: &str) -> bool {
    plx_data::ott::valid_local_source_label(s)
}
impl Request {
    pub fn parse(v: &Value) -> Result<Self, Error> {
        let p = v.as_object().ok_or(Error::Invalid)?;
        if p.get("version").and_then(Value::as_u64) != Some(1) {
            return Err(Error::Invalid);
        }
        let runtime = p
            .get("runtime")
            .and_then(Value::as_str)
            .filter(|s| token(s))
            .ok_or(Error::Invalid)?
            .to_owned();
        let op = p.get("op").and_then(Value::as_str).ok_or(Error::Invalid)?;
        let text = |key: &str| {
            p.get(key)
                .and_then(Value::as_str)
                .filter(|s| token(s))
                .map(str::to_owned)
                .ok_or(Error::Invalid)
        };
        let allowed: &[&str] = match op {
            "roots" => &["version", "runtime", "op"],
            "list" => &[
                "version",
                "runtime",
                "op",
                "generation",
                "entry_id",
                "cursor",
            ],
            "more" | "select" => &["version", "runtime", "op", "generation", "select_id"],
            _ => return Err(Error::Invalid),
        };
        if p.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(Error::Invalid);
        }
        let generation = if op == "roots" {
            None
        } else {
            Some(text("generation")?)
        };
        let op = match op {
            "roots" => Op::Roots,
            "list" => match (p.contains_key("entry_id"), p.contains_key("cursor")) {
                (true, false) => Op::Entry(text("entry_id")?),
                (false, true) => Op::Cursor(text("cursor")?),
                _ => return Err(Error::Invalid),
            },
            "more" => Op::More(text("select_id")?),
            "select" => Op::Select(text("select_id")?),
            _ => unreachable!(),
        };
        Ok(Self {
            runtime,
            generation,
            op,
        })
    }
}
pub(super) fn failure(runtime: &str, params: &Value, error: Error) -> Value {
    // The controller correlates a reply with the requested receiver runtime,
    // including typed stale errors after a receiver restart.
    let runtime = params["runtime"].as_str().filter(|value| token(value)).unwrap_or(runtime);
    let op = match params["op"].as_str() {
        Some("roots") => "roots",
        Some("list") => "list",
        Some("more") => "more",
        Some("select") => "select",
        _ => "unknown",
    };
    json!({"status":"rejected","data":{"version":1,"runtime":runtime,"op":op,"state":"error","error":error.code()}})
}
pub(super) fn bounded(value: &Value) -> bool {
    // This value already contains status + data. Delivery adds only a validated
    // 32-byte hexadecimal request ID (40 JSON bytes); reserve 1024 bytes so the
    // whole emitted response remains below the controller's 64 KiB envelope cap.
    serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= MAX_BYTES - 1024)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_requests_and_opaque_tokens() {
        assert!(Request::parse(&json!({"version":1,"runtime":"r","op":"roots"})).is_ok());
        for extra in [
            json!({"path":"/tmp/usb/x"}),
            json!({"limit":65}),
            json!({"source_handle":"x"}),
        ] {
            let mut v = json!({"version":1,"runtime":"r","op":"roots"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(matches!(Request::parse(&v), Err(Error::Invalid)));
        }
        assert!(!token("../path"));
        assert!(!token(&"a".repeat(257)));
        assert!(name("folder\\name"));
        assert!(!name("x\ny"));
    }
    #[test]
    fn list_and_expansion_are_distinct() {
        for op in ["list", "more", "select"] {
            let mut v = json!({"version":1,"runtime":"r","op":op,"generation":"g"});
            assert!(Request::parse(&v).is_err());
            v[if op == "list" { "cursor" } else { "select_id" }] = json!("c");
            assert!(Request::parse(&v).is_ok());
            v["entry_id"] = json!("e");
            assert!(Request::parse(&v).is_err());
        }
    }
}

#[cfg(test)]
mod shared_wire_fixtures {
    use super::*;
    #[test]
    fn remote_fixture_requests_and_display_names_match_native_contract() {
        let fixtures: Value =
            serde_json::from_str(include_str!("ott_runtime_tests/local_browser_v1.json")).unwrap();
        for case in fixtures["browser_cases"].as_array().unwrap() {
            let request = Request::parse(&case["request"]).unwrap();
            assert_eq!(request.op.name(), case["data"]["op"].as_str().unwrap());
            if let Some(entries) = case["data"]["entries"].as_array() {
                for entry in entries {
                    assert!(name(entry["name"].as_str().unwrap()), "{}", case["name"]);
                    assert!(token(entry["id"].as_str().unwrap()));
                }
            }
            assert!(bounded(
                &json!({"status":case["status"],"data":case["data"]})
            ));
        }
    }
}

#[cfg(test)] mod error_correlation_tests {
    use super::*;
    #[test] fn errors_echo_valid_request_runtime_including_early_rejections() {
        for error in [Error::Stale,Error::Expired,Error::Restricted,Error::Busy,Error::Changed] {
            let params=json!({"version":1,"runtime":"old-runtime","op":"select","generation":"old-view","select_id":"old-entry"});
            let response=failure("new-runtime",&params,error);
            assert_eq!(response["data"]["runtime"],"old-runtime");assert_eq!(response["data"]["error"],error.code());
        }
        let bad=json!({"runtime":"x".repeat(257),"op":"roots"});assert_eq!(failure("current",&bad,Error::Invalid)["data"]["runtime"],"current");
    }
}

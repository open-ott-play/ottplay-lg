//! Closed managed-server wire types. This module grants no source authority.
//! The app owner must admit the exact retained context and observe applied managed
//! runtime/profile publication before encoding a successful selection response.
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidRequest, Unsupported, Busy, Stale, SourceChanged, AuthorizationConflict,
    Unavailable, Restricted, Expired, Cancelled, LimitExceeded, ActivationFailed, OutcomeUnknown,
}
impl Error {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request", Self::Unsupported => "unsupported",
            Self::Busy => "busy", Self::Stale => "stale", Self::SourceChanged => "source_changed",
            Self::AuthorizationConflict => "authorization_conflict", Self::Unavailable => "unavailable",
            Self::Restricted => "restricted", Self::Expired => "expired", Self::Cancelled => "cancelled",
            Self::LimitExceeded => "limit_exceeded", Self::ActivationFailed => "activation_failed",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op { List, Select }
impl Op {
    fn text(self) -> &'static str { match self { Self::List => "list", Self::Select => "select" } }
}

// Never derive Debug for a request/correlation/row: opaque context is not diagnostic data.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Correlation { runtime: String, op: Op }
impl Correlation {
    pub(super) fn from_params(params: &Value) -> Option<Self> {
        let runtime = params.get("runtime")?.as_str().filter(|v| runtime_id(v))?;
        let op = match params.get("op")?.as_str()? { "list" => Op::List, "select" => Op::Select, _ => return None };
        Some(Self { runtime: runtime.into(), op })
    }
    pub(crate) fn runtime(&self) -> &str { &self.runtime }
    pub(crate) fn rejected(&self, error: Error) -> Value {
        json!({"status":if error == Error::Unsupported { "unsupported" } else { "rejected" },
            "data":{"version":1,"runtime":self.runtime,"op":self.op.text(),"state":"error","error":error.code()}})
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Operation {
    List,
    Select { server_id: String, context: String },
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Request { correlation: Correlation, operation: Operation }
pub(crate) struct DecodeError { pub(crate) correlation: Option<Correlation>, pub(crate) error: Error }
impl DecodeError {
    pub(crate) fn response(&self) -> Value {
        self.correlation.as_ref().map_or_else(
            || json!({"status":"rejected","data":{"error":"Invalid managed-server request"}}),
            |correlation| correlation.rejected(self.error))
    }
}
impl Request {
    /// Direct request decoding is bounded by received bytes before JSON projection.
    /// Native poll admission supplies an action/params projection, not the original
    /// controller POST body. Its independent 16-KiB bound cannot attest to that body.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let fail = |correlation, error| DecodeError { correlation, error };
        if bytes.len() > 16 * 1024 {
            return Err(fail(None, Error::LimitExceeded));
        }
        let value = super::ingress::decode_unique(bytes)
            .map_err(|_| fail(None, Error::InvalidRequest))?;
        let object = value.as_object().ok_or_else(|| fail(None, Error::InvalidRequest))?;
        if object.get("action").and_then(Value::as_str) != Some("plex_servers") {
            return Err(fail(None, Error::InvalidRequest));
        }
        let params = object.get("params").ok_or_else(|| fail(None, Error::InvalidRequest))?;
        let correlation = Correlation::from_params(params);
        let invalid = || fail(correlation.clone(), Error::InvalidRequest);
        if !keys(object, &["action", "params"]) { return Err(invalid()); }
        let fields = params.as_object().ok_or_else(invalid)?;
        if fields.get("version").and_then(Value::as_u64) != Some(1) { return Err(invalid()); }
        let bound = correlation.clone().ok_or_else(invalid)?;
        let operation = match bound.op {
            Op::List => {
                if !keys(fields, &["version", "runtime", "op"]) { return Err(invalid()); }
                Operation::List
            }
            Op::Select => {
                if !keys(fields, &["version", "runtime", "op", "server_id", "context"]) { return Err(invalid()); }
                let server_id = fields.get("server_id").and_then(Value::as_str)
                    .filter(|v| machine_id(v)).ok_or_else(invalid)?;
                let context = fields.get("context").and_then(Value::as_str)
                    .filter(|v| context_id(v)).ok_or_else(invalid)?;
                Operation::Select { server_id: server_id.into(), context: context.into() }
            }
        };
        Ok(Self { correlation: bound, operation })
    }
    pub(crate) fn correlation(&self) -> &Correlation { &self.correlation }
    pub(crate) fn operation(&self) -> &Operation { &self.operation }
    pub(crate) fn list_response(&self, rows: &[Row]) -> Result<Value, Error> {
        if !matches!(self.operation, Operation::List) { return Err(Error::InvalidRequest); }
        if rows.len() > 1 { return Err(Error::LimitExceeded); }
        Ok(json!({"status":"ok","data":{"version":1,"runtime":self.correlation.runtime,
            "op":"list","state":"ready","servers":rows.iter().map(Row::value).collect::<Vec<_>>()}}))
    }
    /// Wire encoding only. Calling this is not an activation receipt. The shared app
    /// owner must first correlate the original invocation and applied publication.
    pub(crate) fn applied_selection_response(&self) -> Result<Value, Error> {
        let Operation::Select { server_id, context } = &self.operation else { return Err(Error::InvalidRequest); };
        Ok(json!({"status":"ok","data":{"version":1,"runtime":self.correlation.runtime,
            "op":"select","state":"ready","server_id":server_id,"context":context,
            "activated":true,"persisted":false,"playback_confirmed":false}}))
    }
}

pub(crate) struct Row { server_id: String, name: String, context: String, expires_in_ms: u32 }
impl Row {
    pub(crate) fn new(server_id: String, name: String, context: String, expires_in_ms: u32) -> Result<Self, Error> {
        if !machine_id(&server_id) || !context_id(&context) || name.is_empty() || name.len() > 512
            || name.chars().any(|c| c.is_control()) || !(1..=120000).contains(&expires_in_ms) {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { server_id, name, context, expires_in_ms })
    }
    fn value(&self) -> Value {
        json!({"server_id":self.server_id,"name":self.name,"context":self.context,"expires_in_ms":self.expires_in_ms})
    }
}

fn keys(value: &Map<String, Value>, required: &[&str]) -> bool {
    value.len() == required.len() && required.iter().all(|key| value.contains_key(*key))
}
fn runtime_id(value: &str) -> bool {
    (1..=64).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn machine_id(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn context_id(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
}

#[cfg(test)]
#[path = "plex_servers_tests.rs"]
mod tests;

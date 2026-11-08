//! Pure global Favorites transfer protocol. IO, clocks, UUID generation, profile
//! resolution and durable commit receipts belong to the application adapter.
//! A Commit effect is never an acknowledgement: reply only after finish_commit.

use super::favorites::{Document, Favorite, Locator};
use plx_base::{b64, sha256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

const MAX_DOCUMENT: usize = 1_048_576;
const KINDS: [&str; 5] = ["channel", "channel-name", "local-media", "plex-media", "vportal-media"];
const OPS: [&str; 8] = ["describe", "export", "begin", "append", "preview", "commit", "status", "abort"];

pub fn hash(bytes: &[u8]) -> String {
    sha256::sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
pub fn revision(document: &Document) -> Result<String> {
    Ok(hash(&document.encode().map_err(|_| "invalid_document")?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution { Resolved, Missing, Ambiguous, Unsupported }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Verified, NotApplied, Unknown }

/// This receipt must be persisted in the SAME atomic state write as the document.
/// A runner may expose it as verified only after durable read-back succeeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub changed: bool,
    pub context_revision: String,
    pub counts: Counts,
    pub expected_revision: String,
    pub input_document_sha256: String,
    pub new_revision: String,
    pub operation_id: String,
    pub operation_runtime: String,
    pub outcome: Outcome,
    pub persistence: String,
    pub plan_digest: String,
    pub required_kinds: Vec<String>,
    pub result_document_sha256: String,
    pub subject: String,
    pub transaction_id: String,
    pub version: u8,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub before: usize, pub after: usize, pub added: usize, pub removed: usize,
    pub retained: usize, pub updated: usize, pub moved: usize, pub duplicates_skipped: usize,
}

/// Persist a pending/unknown status before mutating storage. It prevents an
/// interrupted writer from replaying an operation as if it never started.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableStatus {
    pub operation_id: String,
    pub operation_runtime: Option<String>,
    pub transaction_id: Option<String>,
    pub outcome: Outcome,
    pub state: String,
    pub receipt: Option<Receipt>,
}
impl DurableStatus {
    pub fn verified(receipt: Receipt) -> Self {
        Self { operation_id: receipt.operation_id.clone(), operation_runtime: Some(receipt.operation_runtime.clone()),
            transaction_id: Some(receipt.transaction_id.clone()), outcome: Outcome::Verified,
            state: "committed".into(), receipt: Some(receipt) }
    }
    pub fn pending(intent: &CommitIntent) -> Self {
        Self { operation_id: intent.receipt.operation_id.clone(), operation_runtime: Some(intent.receipt.operation_runtime.clone()),
            transaction_id: Some(intent.receipt.transaction_id.clone()), outcome: Outcome::Unknown,
            state: "unknown".into(), receipt: None }
    }
}

/// A single immutable observation at command admission. Resolver must only
/// inspect this observation; it must not change profiles or initiate playback.
pub struct Context<'a> {
    pub runtime: &'a str,
    /// Stable local timeline used for every protocol deadline comparison.
    pub now_ms: u64,
    /// Current UTC sample, used only when rendering externally visible timestamps.
    pub wall_now_ms: u64,
    pub revision: &'a str,
    pub context_revision: &'a str,
    pub document: &'a Document,
    pub authorized: bool,
    pub write_ready: bool,
    pub write_blocker: Option<&'a str>,
    /// Fresh UUIDv4 values supplied by the runner; never generated in a machine.
    pub fresh_ids: [&'a str; 2],
    pub resolve: &'a dyn Fn(&Favorite) -> Resolution,
    pub durable_status: &'a [DurableStatus],
}

#[derive(Clone, Debug)]
pub struct CommitIntent {
    pub document: Document,
    pub receipt: Receipt,
    pub deadline_ms: u64,
}
#[derive(Clone, Debug)]
pub enum Fx { Reply(Value), Commit(CommitIntent) }

struct Snapshot { id: String, bytes: Vec<u8>, hash: String, revision: String, expires: u64 }
struct Admission { id: String, runtime: String, revision: String, context: String, expires: u64 }
struct Chunk { hash: String, offset: usize, size: usize }
struct Plan { intent: CommitIntent, ready: bool, rows: Vec<Value>, resolution: Value }
struct Stage {
    id: String, runtime: String, args: Value, bytes: Vec<u8>, chunks: Vec<Chunk>,
    absolute: u64, idle: u64, begin: Value, plan: Option<Plan>, started: bool,
}
#[derive(Default)]
pub struct Protocol {
    snapshot: Option<Snapshot>, admission: Option<Admission>, stage: Option<Stage>,
    terminal: Option<DurableStatus>,
}

type Result<T> = std::result::Result<T, &'static str>;
fn exact(v: &Value, fields: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == fields.len() && fields.iter().all(|k| o.contains_key(*k)))
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> { v[k].as_str().ok_or("invalid_request") }
fn integer(v: &Value, k: &str, min: u64, max: u64) -> Result<u64> {
    v[k].as_u64().or_else(|| v[k].as_f64().filter(|n| n.is_finite() && n.fract() == 0.0
        && *n >= min as f64 && *n <= max as f64).map(|n| n as u64))
        .filter(|n| *n >= min && *n <= max).ok_or("invalid_request")
}
fn is_hash(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.as_bytes()[14] == b'4' && b"89ab".contains(&s.as_bytes()[19])
        && s.bytes().enumerate().all(|(i,b)| if [8,13,18,23].contains(&i) { b == b'-' } else { b.is_ascii_digit() || (b'a'..=b'f').contains(&b) })
}
fn kinds(doc: &Document) -> Vec<String> {
    doc.entries().iter().map(|f| match f.locator {
        Locator::Channel {..} => "channel", Locator::ChannelName {..} => "channel-name", Locator::Local {..} => "local-media",
        Locator::Plex {..} => "plex-media", Locator::VPortal {..} => "vportal-media",
    }.to_owned()).collect::<BTreeSet<_>>().into_iter().collect()
}
fn response(op: &str, runtime: &str, mut data: Value) -> Value {
    let object = data.as_object_mut().expect("protocol response is an object");
    object.insert("op".into(), json!(op)); object.insert("runtime".into(), json!(runtime));
    object.insert("subject".into(), json!("global")); object.insert("version".into(), json!(1));
    data
}
fn metadata_response(op: &str, runtime: &str, data: Value) -> Value {
    let reply = response(op, runtime, data);
    if reply.to_string().len() > 131072 {
        response(op, runtime, json!({"error":"limits_exceeded"}))
    } else { reply }
}
// JSON numbers have one Number representation on the web. Normalize the
// admitted integer fields before storing/comparing retries (1, 1.0 and 1e0).
fn begin_args(p: &Value) -> Value {
    let mut args = p.clone();
    for (key, max) in [("size_bytes", MAX_DOCUMENT as u64), ("chunk_count", 256), ("budget_ms", 600000)] {
        if let Ok(value) = integer(p, key, 1, max) { args[key] = json!(value); }
    }
    args
}
fn durable_reply(status: &DurableStatus) -> Value {
    // A pending record carries private recovery intent, not a commit receipt.
    // Expose attestation only after the storage adapter has verified durability.
    let receipt = status.receipt.as_ref().filter(|receipt|
        status.outcome == Outcome::Verified && receipt.outcome == Outcome::Verified
            && receipt.operation_id == status.operation_id
            && status.operation_runtime.as_ref() == Some(&receipt.operation_runtime)
            && status.transaction_id.as_ref() == Some(&receipt.transaction_id));
    json!({"operation_id":status.operation_id,
        "operation_runtime":receipt.map(|r| &r.operation_runtime),
        "outcome":status.outcome,"receipt":receipt,"state":status.state,
        "transaction_id":receipt.map(|r| &r.transaction_id)})
}
fn limits() -> Value { json!({"absolute_ttl_seconds":600,"idle_ttl_seconds":120,"max_active_transactions":1,
    "max_append_bytes":8192,"max_chunks":256,"max_document_bytes":1048576,"max_entries":1000,
    "max_envelope_bytes":16384,"max_export_bytes":65536,"max_metadata_bytes":131072,
    "max_preview_rows":64,"max_receipt_ledger_bytes":65536,"max_receipts":32}) }

impl Protocol {
    pub fn capability() -> Value { json!({"version":1}) }
    /// The legacy list and item/recovery extensions have different request
    /// schemas. Only the global document transfer action belongs to this parser.
    pub fn accepts_action(action: &str) -> bool { action == "favorites_global" }
    fn can_write(&self, cx: &Context<'_>) -> bool {
        cx.write_ready && !cx.durable_status.iter().any(|s| s.outcome == Outcome::Unknown)
            && !self.terminal.as_ref().is_some_and(|s| s.outcome == Outcome::Unknown)
    }
    /// Call on timer ticks too, so abandoned stage bytes do not wait for a new request.
    pub fn expire(&mut self, now_ms: u64) {
        if self.snapshot.as_ref().is_some_and(|s| now_ms >= s.expires) { self.snapshot = None; }
        if self.admission.as_ref().is_some_and(|a| now_ms >= a.expires) { self.admission = None; }
        if self.stage.as_ref().is_some_and(|s| !s.started && (now_ms >= s.idle || now_ms >= s.absolute)) {
            let s = self.stage.take().unwrap();
            self.terminal = Some(DurableStatus { operation_id: s.args["operation_id"].as_str().unwrap_or("").into(),
                operation_runtime: Some(s.runtime), transaction_id: Some(s.id), outcome: Outcome::NotApplied,
                state: "expired".into(), receipt: None });
        }
    }
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.stage.as_ref().filter(|s| !s.started).map(|s| s.idle.min(s.absolute))
            .into_iter().chain(self.snapshot.as_ref().map(|s| s.expires)).chain(self.admission.as_ref().map(|a| a.expires)).min()
    }
    pub fn execute(&mut self, params: &Value, envelope_bytes: usize, expires_at_ms: u64, cx: &Context<'_>) -> Fx {
        self.expire(cx.now_ms);
        let op = params["op"].as_str().unwrap_or("");
        let runtime = params["runtime"].as_str().unwrap_or(cx.runtime);
        let result = (|| {
            if !(1..=16384).contains(&envelope_bytes) { return Err("envelope_too_large"); }
            if params["runtime"].as_str().is_none() || !OPS.contains(&op) { return Err("invalid_request"); }
            if runtime != cx.runtime { return Err("runtime_changed"); }
            if expires_at_ms <= cx.now_ms { return Err("expired"); }
            if !cx.authorized { return Err("policy_denied"); }
            if !is_hash(cx.revision) || !is_hash(cx.context_revision) { return Err("context_changed"); }
            self.operation(params, expires_at_ms.min(cx.now_ms.saturating_add(30000)), cx)
        })();
        match result {
            Ok(Fx::Reply(data)) => Fx::Reply(metadata_response(op, runtime, data)),
            Ok(commit @ Fx::Commit(_)) => commit,
            Err(error) => Fx::Reply(response(op, runtime, json!({"error":error}))),
        }
    }
    fn checked_stage<'a>(&'a mut self, p: &Value, cx: &Context<'_>) -> Result<&'a mut Stage> {
        let stage = self.stage.as_mut().filter(|s| Some(s.id.as_str()) == p["transaction_id"].as_str()).ok_or("transaction_not_found")?;
        if stage.runtime != cx.runtime { return Err("runtime_changed"); }
        if cx.now_ms >= stage.idle || cx.now_ms >= stage.absolute { return Err("expired"); }
        if stage.args["expected_context_revision"] != cx.context_revision { return Err("context_changed"); }
        if !stage.started && stage.args["expected_revision"] != cx.revision { return Err("revision_conflict"); }
        stage.idle = stage.absolute.min(cx.now_ms.saturating_add(120000));
        Ok(stage)
    }
    fn operation(&mut self, p: &Value, deadline: u64, cx: &Context<'_>) -> Result<Fx> {
        let op = string(p, "op")?;
        let data = match op {
            "describe" if exact(p, &["op","runtime"]) => {
                let (descriptor, export_error) = match cx.document.encode() {
                    Ok(bytes) if is_uuid(cx.fresh_ids[0]) => {
                        let snapshot = Snapshot { id: cx.fresh_ids[0].into(), hash: hash(&bytes), bytes,
                            revision: cx.revision.into(), expires: cx.now_ms.saturating_add(600000) };
                        let descriptor = json!({"document_sha256":snapshot.hash,"entries":cx.document.entries().len(),
                            "expires_at":wall_expiry(cx,snapshot.expires),"size_bytes":snapshot.bytes.len(),"snapshot_id":snapshot.id});
                        self.snapshot = Some(snapshot);
                        (descriptor, None)
                    }
                    Ok(_) => (Value::Null, Some("storage_unavailable")),
                    Err(_) => (Value::Null, Some("unportable_export")),
                };
                let ready = self.stage.is_none() && self.can_write(cx);
                if ready && !is_uuid(cx.fresh_ids[1]) { return Err("unsupported"); }
                self.admission = ready.then(|| Admission { id:cx.fresh_ids[1].into(), runtime:cx.runtime.into(),
                    revision:cx.revision.into(), context:cx.context_revision.into(), expires:cx.now_ms.saturating_add(10000) });
                json!({"admission":self.admission.as_ref().map(|a| json!({"operation_id":a.id,"ttl_ms":10000})),
                    "revision":cx.revision,"context_revision":cx.context_revision,"write_ready":ready,
                    "write_blocker":if ready { None } else if self.stage.is_some() { Some("busy") }
                        else if cx.durable_status.iter().any(|s| s.outcome == Outcome::Unknown)
                            || self.terminal.as_ref().is_some_and(|s| s.outcome == Outcome::Unknown) { Some("reconciliation_required") }
                        else { cx.write_blocker.or(Some("unsupported_persistence")) },
                    "export":descriptor,"export_error":export_error,"limits":limits(),"locator_kinds":KINDS})
            }
            "export" if exact(p, &["op","runtime","snapshot_id","offset","max_bytes"]) => {
                let s = self.snapshot.as_ref().filter(|s| Some(s.id.as_str()) == p["snapshot_id"].as_str()).ok_or("snapshot_not_found")?;
                let offset = integer(p,"offset",0,s.bytes.len().saturating_sub(1) as u64)? as usize;
                let max = integer(p,"max_bytes",1,65536)? as usize;
                let end = (offset+max).min(s.bytes.len()); let bytes = &s.bytes[offset..end];
                json!({"chunk_sha256":hash(bytes),"data_base64":b64::encode(bytes),"document_sha256":s.hash,
                    "eof":end==s.bytes.len(),"next_offset":end,"offset":offset,"revision":s.revision,
                    "size_bytes":s.bytes.len(),"snapshot_id":s.id})
            }
            "begin" if exact(p,&["op","runtime","operation_id","mode","expected_revision","expected_context_revision",
                "document_sha256","size_bytes","chunk_count","budget_ms","required_kinds"]) => {
                if let Some(s) = &self.stage {
                    if s.args["operation_id"] == p["operation_id"] {
                        if !s.started && s.args == begin_args(p) && s.runtime == cx.runtime {
                            if s.args["expected_context_revision"] != cx.context_revision { return Err("context_changed"); }
                            if s.args["expected_revision"] != cx.revision { return Err("revision_conflict"); }
                            return Ok(Fx::Reply(s.begin.clone()));
                        }
                        return Err("operation_conflict");
                    }
                    return Err("busy");
                }
                if !self.can_write(cx) { return Err("reconciliation_required"); }
                let id = string(p,"operation_id")?;
                if !is_uuid(id) || !["merge","replace"].contains(&string(p,"mode")?)
                    || !["expected_revision","expected_context_revision","document_sha256"].iter().all(|k| p[*k].as_str().is_some_and(is_hash)) { return Err("invalid_request"); }
                if cx.durable_status.iter().any(|s| s.operation_id == id) { return Err("operation_conflict"); }
                let size = integer(p,"size_bytes",1,MAX_DOCUMENT as u64)? as usize;
                let count = integer(p,"chunk_count",1,256)?;
                let budget = integer(p,"budget_ms",1,600000)?;
                if size > count as usize * 8192 || budget < 1500 * (count + 5) + 10000 { return Err("invalid_request"); }
                let requested = p["required_kinds"].as_array().ok_or("unsupported")?;
                if requested.len() > 5 || requested.iter().enumerate().any(|(i,v)| {
                    !v.as_str().is_some_and(|s| KINDS.contains(&s)) || (i > 0 && requested[i-1].as_str() >= v.as_str())
                }) { return Err("unsupported"); }
                let a = self.admission.as_ref().ok_or("operation_conflict")?;
                if a.id != id || a.runtime != cx.runtime || a.revision != p["expected_revision"] || a.context != p["expected_context_revision"]
                    || cx.now_ms >= a.expires { return Err("operation_conflict"); }
                if p["expected_revision"] != cx.revision { return Err("revision_conflict"); }
                if p["expected_context_revision"] != cx.context_revision { return Err("context_changed"); }
                if !is_uuid(cx.fresh_ids[0]) { return Err("unsupported"); }
                let absolute = cx.now_ms.saturating_add(budget); let idle = cx.now_ms.saturating_add(budget.min(120000));
                let begin = json!({"absolute_expires_at":wall_expiry(cx,absolute),"chunk_count":count,"context_revision":cx.context_revision,
                    "document_sha256":p["document_sha256"],"expected_revision":cx.revision,"idle_expires_at":wall_expiry(cx,idle),
                    "next_seq":0,"operation_id":id,"received_bytes":0,"required_kinds":requested,"size_bytes":size,"transaction_id":cx.fresh_ids[0]});
                self.admission = None;
                self.stage = Some(Stage { id:cx.fresh_ids[0].into(),runtime:cx.runtime.into(),args:begin_args(p),bytes:Vec::with_capacity(size),
                    chunks:Vec::new(),absolute,idle,begin:begin.clone(),plan:None,started:false });
                begin
            }
            "status" if exact(p,&["op","runtime","operation_id"]) => {
                let id = string(p,"operation_id")?; if !is_uuid(id) { return Err("invalid_request"); }
                if let Some(s) = self.stage.as_ref().filter(|s| s.args["operation_id"] == id && s.runtime == cx.runtime) {
                    json!({"operation_id":id,"operation_runtime":s.runtime,"outcome":if s.started {"unknown"} else {"not_applied"},
                        "receipt":null,"state":if s.started {"applying"} else if s.plan.is_some() {"ready"} else {"receiving"},"transaction_id":s.id})
                } else if let Some(s) = self.terminal.as_ref().filter(|s| s.operation_id == id && ["aborted","expired"].contains(&s.state.as_str())) {
                    serde_json::to_value(s).map_err(|_|"storage_unavailable")?
                } else if let Some(s) = cx.durable_status.iter().find(|s| s.operation_id == id)
                    .or_else(|| self.terminal.as_ref().filter(|s| s.operation_id == id)) {
                    durable_reply(s)
                } else { json!({"operation_id":id,"operation_runtime":null,"outcome":"not_applied","receipt":null,"state":"absent","transaction_id":null}) }
            }
            "append" if exact(p,&["op","runtime","transaction_id","seq","offset","chunk_sha256","data_base64"]) => {
                let s = self.checked_stage(p,cx)?;
                let seq = integer(p,"seq",0,255)? as usize; let offset = integer(p,"offset",0,1048575)? as usize;
                let encoded = string(p,"data_base64")?;
                if encoded.len() > 10924 { return Err("invalid_request"); }
                let bytes = b64::decode(encoded).ok_or("invalid_request")?;
                if bytes.is_empty() || bytes.len()>8192 || b64::encode(&bytes) != encoded { return Err("invalid_request"); }
                let h = string(p,"chunk_sha256")?;
                if !is_hash(h) { return Err("invalid_request"); }
                if hash(&bytes) != h { return Err("hash_mismatch"); }
                let duplicate = seq < s.chunks.len();
                if duplicate {
                    let c=&s.chunks[seq]; if c.offset!=offset || c.size!=bytes.len() || c.hash!=h { return Err("chunk_conflict"); }
                } else {
                    if seq != s.chunks.len() || offset != s.bytes.len() { return Err("out_of_order"); }
                    if s.started || s.plan.is_some() || seq >= s.args["chunk_count"].as_u64().unwrap_or(0) as usize
                        || s.bytes.len()+bytes.len() > s.args["size_bytes"].as_u64().unwrap_or(0) as usize { return Err("limits_exceeded"); }
                    s.chunks.push(Chunk { hash:h.into(),offset,size:bytes.len() }); s.bytes.extend_from_slice(&bytes);
                }
                json!({"chunk_sha256":h,"duplicate":duplicate,"next_seq":s.chunks.len(),"offset":offset,
                    "operation_id":s.args["operation_id"],"received_bytes":s.bytes.len(),"seq":seq,"transaction_id":s.id})
            }
            "preview" if exact(p,&["op","runtime","transaction_id","page_offset","max_entries"]) => {
                let s = self.checked_stage(p,cx)?;
                let offset=integer(p,"page_offset",0,1000)? as usize; let max=integer(p,"max_entries",1,64)? as usize;
                if s.chunks.len()!=s.args["chunk_count"].as_u64().unwrap_or(0) as usize
                    || s.bytes.len()!=s.args["size_bytes"].as_u64().unwrap_or(0) as usize { return Err("incomplete"); }
                if s.plan.is_none() { s.plan=Some(make_plan(s,cx)?); }
                let plan=s.plan.as_ref().unwrap(); if offset>plan.rows.len() { return Err("invalid_request"); }
                let end=(offset+max).min(plan.rows.len()); let r=&plan.intent.receipt;
                json!({"blocker":if plan.ready {None} else {Some("metadata_conflict")},"context_revision":cx.context_revision,
                    "counts":r.counts,"expected_revision":r.expected_revision,"input_document_sha256":r.input_document_sha256,
                    "legacy_effects":{"linked_removed":0,"unmapped_retained":0},"next_offset":if end<plan.rows.len(){Some(end)}else{None},
                    "operation_id":r.operation_id,"page_offset":offset,"plan_digest":r.plan_digest,"ready":plan.ready,
                    "required_kinds":r.required_kinds,"resolution_counts":plan.resolution,"result_document_sha256":r.result_document_sha256,
                    "rows":plan.rows[offset..end],"transaction_id":s.id})
            }
            "commit" if exact(p,&["op","runtime","transaction_id","operation_id","plan_digest"]) => {
                let writable = self.can_write(cx);
                let s = self.checked_stage(p,cx)?;
                let plan=s.plan.as_mut().ok_or("not_ready")?;
                if p["operation_id"]!=s.args["operation_id"] || !plan.ready || p["plan_digest"]!=plan.intent.receipt.plan_digest { return Err("not_ready"); }
                if s.started { return Err("reconciliation_required"); }
                if !writable { return Err("reconciliation_required"); }
                s.started=true; plan.intent.deadline_ms=deadline.min(s.absolute);
                return Ok(Fx::Commit(plan.intent.clone()));
            }
            "abort" if exact(p,&["op","runtime","transaction_id","operation_id"]) => {
                let s=self.checked_stage(p,cx)?;
                if p["operation_id"]!=s.args["operation_id"] { return Err("operation_conflict"); }
                if s.started { return Err("reconciliation_required"); }
                let result=json!({"operation_id":s.args["operation_id"],"outcome":"not_applied","state":"aborted","transaction_id":s.id});
                self.terminal=Some(DurableStatus {operation_id:string(&s.args,"operation_id")?.into(),operation_runtime:Some(s.runtime.clone()),
                    transaction_id:Some(s.id.clone()),outcome:Outcome::NotApplied,state:"aborted".into(),receipt:None});
                self.stage=None; result
            }
            _ => { self.checked_stage(p,cx)?; return Err("invalid_request"); }
        };
        Ok(Fx::Reply(data))
    }
    /// Must be called only with the runner's actual durable verification result.
    pub fn finish_commit(&mut self, operation_id: &str, outcome: Outcome) -> Value {
        let Some(stage)=self.stage.as_ref().filter(|s| s.started && s.args["operation_id"]==operation_id) else {
            return response("commit","",json!({"error":"transaction_not_found"}));
        };
        let receipt=stage.plan.as_ref().unwrap().intent.receipt.clone();
        let runtime=stage.runtime.clone(); let transaction=stage.id.clone();
        self.terminal=Some(DurableStatus {operation_id:operation_id.into(),operation_runtime:Some(runtime.clone()),
            transaction_id:Some(transaction.clone()),outcome,state:match outcome {Outcome::Verified=>"committed",Outcome::NotApplied=>"aborted",Outcome::Unknown=>"unknown"}.into(),
            receipt:(outcome==Outcome::Verified).then_some(receipt.clone())});
        self.stage=None;
        metadata_response("commit",&runtime, match outcome {
            Outcome::Verified=>json!({"operation_id":operation_id,"receipt":receipt,"transaction_id":transaction}),
            Outcome::NotApplied=>json!({"error":"revision_conflict"}),
            Outcome::Unknown=>json!({"error":"reconciliation_required"}),
        })
    }
}

fn make_plan(s: &Stage, cx: &Context<'_>) -> Result<Plan> {
    if hash(&s.bytes)!=s.args["document_sha256"] { return Err("hash_mismatch"); }
    let input=Document::parse(&s.bytes).map_err(|_|"invalid_document")?;
    let required=kinds(&input); if json!(required)!=s.args["required_kinds"] { return Err("unsupported"); }
    let merge=s.args["mode"]=="merge";
    let previous:HashMap<String,(usize,&Favorite)>=cx.document.entries().iter().enumerate().map(|(i,f)|(f.identity(),(i,f))).collect();
    let conflicts:HashMap<String,usize>=input.entries().iter().enumerate().filter(|(_,f)|previous.get(&f.identity()).is_some_and(|(_,p)|p.title!=f.title))
        .map(|(i,f)|(f.identity(),i)).collect();
    let mut result=if merge {cx.document.clone()} else {input.clone()};
    if merge {result.merge(&input).map_err(|_|"limits_exceeded")?;}
    let mut counts=Counts {before:previous.len(),after:result.entries().len(),..Counts::default()};
    let mut rows=Vec::new(); let mut resolution=json!({"ambiguous":0,"missing":0,"resolved":0,"unsupported":0});
    let ready=!merge || conflicts.is_empty();
    for (i,f) in result.entries().iter().enumerate() {
        if let Some((old,p))=previous.get(&f.identity()) {
            counts.retained+=1; counts.updated+=usize::from(p.title!=f.title); counts.moved+=usize::from(*old!=i);
        } else {counts.added+=1;}
        let status=(cx.resolve)(f); let name=match status {Resolution::Resolved=>"resolved",Resolution::Missing=>"missing",Resolution::Ambiguous=>"ambiguous",Resolution::Unsupported=>"unsupported"};
        resolution[name]=json!(resolution[name].as_u64().unwrap()+1);
        let mut row=json!({"entry_index":i,"state":status});
        if !ready { if let Some(index)=conflicts.get(&f.identity()) {row["metadata_conflict"]=json!({"incoming_entry_index":index,"reason":"title_mismatch"});} }
        rows.push(row);
    }
    counts.removed=counts.before-counts.retained;
    if merge {counts.duplicates_skipped=input.entries().iter().filter(|f|previous.contains_key(&f.identity())).count();}
    let result_hash=revision(&result)?;
    let digest=hash(json!({"domain":"ott-native-favorites-plan-v1","operation_id":s.args["operation_id"],"transaction_id":s.id,
        "context_revision":cx.context_revision,"expected_revision":cx.revision,"input_document_sha256":s.args["document_sha256"],
        "result_document_sha256":result_hash,"counts":counts}).to_string().as_bytes());
    let receipt=Receipt {changed:result_hash!=cx.revision,context_revision:cx.context_revision.into(),counts,expected_revision:cx.revision.into(),
        input_document_sha256:string(&s.args,"document_sha256")?.into(),new_revision:result_hash.clone(),operation_id:string(&s.args,"operation_id")?.into(),
        operation_runtime:cx.runtime.into(),outcome:Outcome::Verified,persistence:"persistent".into(),plan_digest:digest,required_kinds:required,
        result_document_sha256:result_hash,subject:"global".into(),transaction_id:s.id.clone(),version:1};
    Ok(Plan {intent:CommitIntent{document:result,receipt,deadline_ms:s.absolute},ready,rows,resolution})
}

// Gregorian UTC conversion, independent of host timezone or a process clock.
fn wall_expiry(cx:&Context<'_>,expiry:u64)->String {
    iso(cx.wall_now_ms.saturating_add(expiry.saturating_sub(cx.now_ms)))
}

fn iso(ms:u64)->String {
    let seconds=ms/1000; let z=(seconds/86400) as i64+719468;
    let era=z/146097; let doe=z-era*146097; let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;
    let y=yoe+era*400; let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153;
    let day=doy-(153*mp+2)/5+1; let month=mp+if mp<10 {3}else{-9}; let year=y+i64::from(month<=2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",seconds/3600%24,seconds/60%60,seconds%60,ms%1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    const UUID1:&str="20000000-0000-4000-8000-000000000001";
    const UUID2:&str="20000000-0000-4000-8000-000000000002";
    const CONTEXT:&str="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn document()->Document {
        Document::parse(br#"{"version":1,"entries":[{"title":"Film","origin":{"provider":"plex","profile":"Home","subprofile":null,"sourceAlias":"Media"},"locator":{"kind":"plex-media","serverId":"server-1","ratingKey":"42"}}]}"#).unwrap()
    }
    struct Fixture { api:Protocol, now:u64, wall:Option<u64>, doc:Document, context:String, durable:Vec<DurableStatus> }
    impl Fixture {
        fn new()->Self {Self{api:Protocol::default(),now:1_000_000,wall:None,doc:Document::default(),context:CONTEXT.into(),durable:Vec::new()}}
        fn call(&mut self, mut p:Value)->Fx {
            p["runtime"]=json!("runtime-one"); self.raw_call(&p)
        }
        fn raw_call(&mut self,p:&Value)->Fx {
            let rev=revision(&self.doc).unwrap();
            self.api.execute(p,512,self.now+30000,&Context{runtime:"runtime-one",now_ms:self.now,wall_now_ms:self.wall.unwrap_or(self.now),revision:&rev,
                context_revision:&self.context,document:&self.doc,authorized:true,write_ready:true,write_blocker:None,
                fresh_ids:[UUID1,UUID2],resolve:&|_|Resolution::Missing,durable_status:&self.durable})
        }
        fn reply(&mut self,p:Value)->Value {match self.call(p){Fx::Reply(v)=>v,Fx::Commit(_)=>panic!("expected reply")}}
        fn begin(&mut self,doc:&Document)->(String,Vec<u8>){
            let describe=self.reply(json!({"op":"describe"})); let bytes=doc.encode().unwrap();
            let begin=self.reply(json!({"op":"begin","operation_id":describe["admission"]["operation_id"],"mode":"merge",
                "expected_revision":describe["revision"],"expected_context_revision":describe["context_revision"],
                "document_sha256":hash(&bytes),"size_bytes":bytes.len(),"chunk_count":1,"budget_ms":30000,"required_kinds":kinds(doc)}));
            assert!(begin.get("error").is_none(),"{begin}");
            (begin["transaction_id"].as_str().unwrap().into(),bytes)
        }
        fn append(&mut self,id:&str,bytes:&[u8])->Value {self.reply(json!({"op":"append","transaction_id":id,
            "seq":0,"offset":0,"chunk_sha256":hash(bytes),"data_base64":b64::encode(bytes)}))}
        fn preview(&mut self,id:&str)->Value {self.reply(json!({"op":"preview","transaction_id":id,"page_offset":0,"max_entries":64}))}
    }
    #[test]
    fn only_global_document_action_is_dispatched_to_this_protocol(){
        assert!(Protocol::accepts_action("favorites_global"));
        for action in ["favorites", "favorites_global_items", "favorites_global_recovery", "Favorites_global", ""] {
            assert!(!Protocol::accepts_action(action), "{action}");
        }
    }
    #[test]
    fn export_matches_canonical_browser_document_and_is_frozen(){
        let mut f=Fixture::new(); f.doc=document(); let describe=f.reply(json!({"op":"describe"}));
        let expected=f.doc.encode().unwrap(); f.doc=Document::default();
        let chunk=f.reply(json!({"op":"export","snapshot_id":describe["export"]["snapshot_id"],"offset":0,"max_bytes":65536}));
        assert_eq!(b64::decode(chunk["data_base64"].as_str().unwrap()).unwrap(),expected);
        assert_eq!(chunk["document_sha256"],hash(&expected)); assert_eq!(chunk["eof"],true);
        let canonical=String::from_utf8(expected).unwrap();
        assert!(canonical.starts_with("{\"entries\":[{\"locator\":{\"kind\":\"plex-media\",\"ratingKey\":\"42\",\"serverId\":\"server-1\"}"));
        f.now+=600001; assert_eq!(f.reply(json!({"op":"export","snapshot_id":UUID1,"offset":0,"max_bytes":65536}))["error"],"snapshot_not_found");
    }
    #[test]
    fn staged_import_does_not_acknowledge_or_mutate_until_durable_commit(){
        let mut f=Fixture::new(); let (id,bytes)=f.begin(&document());
        let append=f.append(&id,&bytes); assert_eq!(append["duplicate"],false);
        assert_eq!(f.append(&id,&bytes)["duplicate"],true);
        let preview=f.preview(&id); assert_eq!(preview["ready"],true); assert_eq!(preview["resolution_counts"]["missing"],1);
        assert!(f.doc.entries().is_empty());
        let Fx::Commit(intent)=f.call(json!({"op":"commit","transaction_id":id,"operation_id":UUID2,"plan_digest":preview["plan_digest"]})) else {panic!("commit was acknowledged before storage")};
        assert!(f.doc.entries().is_empty()); assert_eq!(intent.document,document());
        assert_eq!(f.reply(json!({"op":"status","operation_id":UUID2}))["state"],"applying");
        f.doc=intent.document; f.durable.push(DurableStatus::verified(intent.receipt.clone()));
        let done=f.api.finish_commit(UUID2,Outcome::Verified); assert_eq!(done["receipt"]["persistence"],"persistent");
        f.api=Protocol::default(); let status=f.reply(json!({"op":"status","operation_id":UUID2}));
        assert_eq!(status["outcome"],"verified"); assert_eq!(status["receipt"]["new_revision"],revision(&f.doc).unwrap());
    }
    #[test]
    fn invalid_chunks_and_changed_context_never_prepare_commit(){
        let mut f=Fixture::new(); let (id,bytes)=f.begin(&document());
        assert_eq!(f.preview(&id)["error"],"incomplete");
        assert_eq!(f.reply(json!({"op":"append","transaction_id":id,"seq":1,"offset":0,"chunk_sha256":hash(&bytes),"data_base64":b64::encode(&bytes)}))["error"],"out_of_order");
        assert_eq!(f.reply(json!({"op":"append","transaction_id":id,"seq":0,"offset":0,"chunk_sha256":CONTEXT,"data_base64":b64::encode(&bytes)}))["error"],"hash_mismatch");
        f.append(&id,&bytes); f.context="c".repeat(64);
        assert_eq!(f.preview(&id)["error"],"context_changed"); assert!(f.doc.entries().is_empty());
    }
    #[test]
    fn expiry_and_abort_drop_the_stage_and_unknown_commit_blocks_new_writes(){
        let mut f=Fixture::new(); let (id,_)=f.begin(&document()); f.now+=30001;
        assert_eq!(f.preview(&id)["error"],"transaction_not_found");
        assert_eq!(f.reply(json!({"op":"status","operation_id":UUID2}))["state"],"expired");
        let (id,bytes)=f.begin(&document()); f.append(&id,&bytes); let plan=f.preview(&id);
        let Fx::Commit(intent)=f.call(json!({"op":"commit","transaction_id":id,"operation_id":UUID2,"plan_digest":plan["plan_digest"]})) else {panic!()};
        f.durable.push(DurableStatus::pending(&intent));
        assert_eq!(f.api.finish_commit(UUID2,Outcome::Unknown)["error"],"reconciliation_required");
        assert_eq!(f.reply(json!({"op":"describe"}))["write_ready"],false);
        f.api=Protocol::default(); assert_eq!(f.reply(json!({"op":"describe"}))["write_ready"],false);
    }
    #[test]
    fn mismatched_titles_are_previewed_without_silent_merge(){
        let mut f=Fixture::new(); f.doc=document(); let mut encoded=document().encode().unwrap();
        let changed=String::from_utf8(std::mem::take(&mut encoded)).unwrap().replace("\"Film\"","\"Renamed\"");
        let incoming=Document::parse(changed.as_bytes()).unwrap(); let (id,bytes)=f.begin(&incoming);
        f.append(&id,&bytes); let preview=f.preview(&id);
        assert_eq!(preview["ready"],false); assert_eq!(preview["blocker"],"metadata_conflict");
        assert_eq!(f.reply(json!({"op":"commit","transaction_id":id,"operation_id":UUID2,"plan_digest":preview["plan_digest"]}))["error"],"not_ready");
        assert_eq!(f.reply(json!({"op":"abort","transaction_id":id,"operation_id":UUID2}))["state"],"aborted");
    }
    #[test]
    fn integral_json_number_spellings_preserve_begin_replay_and_append(){
        // Matches the real web protocol's Number parsing of 1, 1.0 and 1e0.
        let mut f=Fixture::new(); let d=f.reply(json!({"op":"describe"}));
        let bytes=document().encode().unwrap();
        let mut args=json!({"op":"begin","operation_id":d["admission"]["operation_id"],"mode":"merge",
            "expected_revision":d["revision"],"expected_context_revision":d["context_revision"],
            "document_sha256":hash(&bytes),"size_bytes":bytes.len() as f64,"chunk_count":1.0,
            "budget_ms":30000.0,"required_kinds":kinds(&document())});
        let begin=f.reply(args.clone()); assert!(begin.get("error").is_none(),"{begin}");
        args["size_bytes"]=json!(bytes.len()); args["chunk_count"]=json!(1); args["budget_ms"]=json!(30000);
        assert_eq!(f.reply(args),begin);
        let id=begin["transaction_id"].as_str().unwrap();
        let append=f.reply(json!({"op":"append","transaction_id":id,"seq":0.0,"offset":-0.0,
            "chunk_sha256":hash(&bytes),"data_base64":b64::encode(&bytes)}));
        assert_eq!(append["received_bytes"],bytes.len()); assert_eq!(append["duplicate"],false);
        assert_eq!(f.preview(id)["ready"],true);
    }
    #[test]
    fn malformed_requests_match_web_rejections_without_issuing_admission(){
        let mut f=Fixture::new();
        for runtime in [Value::Null,json!(3),json!(false)] {
            let Fx::Reply(reply)=f.raw_call(&json!({"op":"describe","runtime":runtime})) else {panic!()};
            assert_eq!(reply["error"],"invalid_request"); assert!(f.api.admission.is_none());
        }
        assert_eq!(f.reply(json!({"op":"unknown"}))["error"],"invalid_request");
        assert_eq!(f.reply(json!({"op":"describe","extra":true}))["error"],"transaction_not_found");
        assert_eq!(f.reply(json!({"op":"append"}))["error"],"transaction_not_found");
        let (id,_)=f.begin(&document());
        assert_eq!(f.reply(json!({"op":"append","transaction_id":id,"extra":true}))["error"],"invalid_request");
    }
    #[test]
    fn receiptless_durable_status_does_not_claim_verified_operation_context(){
        let mut f=Fixture::new();
        f.durable.push(DurableStatus{operation_id:UUID2.into(),operation_runtime:Some("prior-runtime".into()),
            transaction_id:Some(UUID1.into()),outcome:Outcome::Unknown,state:"unknown".into(),receipt:None});
        let status=f.reply(json!({"op":"status","operation_id":UUID2}));
        assert_eq!(status["outcome"],"unknown"); assert_eq!(status["state"],"unknown");
        assert!(status["operation_runtime"].is_null()); assert!(status["transaction_id"].is_null());
        assert!(status["receipt"].is_null());
    }
    #[test]
    fn recovery_intent_is_not_exposed_as_a_verified_receipt(){
        let mut f=Fixture::new(); let (id,bytes)=f.begin(&document());
        f.append(&id,&bytes); let preview=f.preview(&id);
        let Fx::Commit(intent)=f.call(json!({"op":"commit","transaction_id":id,
            "operation_id":UUID2,"plan_digest":preview["plan_digest"]})) else {panic!()};
        let mut pending=DurableStatus::pending(&intent);
        let mut receipt=intent.receipt.clone(); receipt.outcome=Outcome::Unknown;
        pending.receipt=Some(receipt); f.durable.push(pending); f.api=Protocol::default();
        let status=f.reply(json!({"op":"status","operation_id":UUID2}));
        assert_eq!(status["outcome"],"unknown"); assert!(status["receipt"].is_null());
        assert!(status["operation_runtime"].is_null()); assert!(status["transaction_id"].is_null());
        f.durable[0]=DurableStatus::verified(intent.receipt);
        let status=f.reply(json!({"op":"status","operation_id":UUID2}));
        assert_eq!(status["outcome"],"verified"); assert!(status["receipt"].is_object());
        assert_eq!(status["operation_runtime"],"runtime-one"); assert_eq!(status["transaction_id"],id);
    }
    #[test]
    fn terminal_expiry_has_the_same_precedence_as_the_web_protocol(){
        let mut f=Fixture::new(); f.begin(&document()); f.now+=30001;
        f.durable.push(DurableStatus{operation_id:UUID2.into(),operation_runtime:None,
            transaction_id:None,outcome:Outcome::Unknown,state:"unknown".into(),receipt:None});
        let status=f.reply(json!({"op":"status","operation_id":UUID2}));
        assert_eq!(status["state"],"expired"); assert_eq!(status["outcome"],"not_applied");
        assert_eq!(status["operation_runtime"],"runtime-one"); assert_eq!(status["transaction_id"],UUID1);
    }
    #[test]
    fn commit_rechecks_unknown_durable_operations_after_preview(){
        let mut f=Fixture::new(); let (id,bytes)=f.begin(&document()); f.append(&id,&bytes); let preview=f.preview(&id);
        f.durable.push(DurableStatus{operation_id:UUID1.into(),operation_runtime:None,
            transaction_id:None,outcome:Outcome::Unknown,state:"unknown".into(),receipt:None});
        let reply=f.reply(json!({"op":"commit","transaction_id":id,"operation_id":UUID2,"plan_digest":preview["plan_digest"]}));
        assert_eq!(reply["error"],"reconciliation_required");
        assert!(!f.api.stage.as_ref().unwrap().started); assert!(f.doc.entries().is_empty());
    }
    #[test]
    fn describe_reports_unportable_export_without_hiding_capabilities(){
        let mut api=Protocol::default();
        let invalid:Document=serde_json::from_value(json!({"version":2,"entries":[]})).unwrap();
        let Fx::Reply(reply)=api.execute(&json!({"op":"describe","runtime":"runtime-one"}),512,1030000,
            &Context{runtime:"runtime-one",now_ms:1000000,wall_now_ms:1000000,revision:CONTEXT,context_revision:CONTEXT,document:&invalid,
                authorized:true,write_ready:false,write_blocker:Some("storage_unavailable"),fresh_ids:[UUID1,"unused"],
                resolve:&|_|Resolution::Missing,durable_status:&[]}) else {panic!()};
        assert_eq!(reply["export_error"],"unportable_export"); assert!(reply["export"].is_null());
        assert_eq!(reply["write_blocker"],"storage_unavailable"); assert_eq!(reply["locator_kinds"],json!(KINDS));
        assert_eq!(reply["limits"]["max_document_bytes"],MAX_DOCUMENT); assert!(reply.get("error").is_none());
    }
    #[test]
    fn metadata_limit_counts_the_complete_wire_envelope(){
        let data=json!({"payload":"x".repeat(131000)});
        assert!(data.to_string().len()<131072);
        let reply=metadata_response("describe","runtime-with-a-long-enough-name",data);
        assert_eq!(reply["error"],"limits_exceeded");
        assert_eq!(metadata_response("describe","runtime-one",json!({"payload":"ok"}))["payload"],"ok");
    }
    #[test]
    fn reported_expiry_uses_current_utc_without_changing_internal_deadlines(){
        for wall in [500_000,2_000_000] {
            let mut f=Fixture::new();f.wall=Some(wall);
            let describe=f.reply(json!({"op":"describe"}));
            assert_eq!(describe["export"]["expires_at"],iso(wall+600_000));
            assert_eq!(f.api.snapshot.as_ref().unwrap().expires,f.now+600_000);
            f.begin(&document());
            let stage=f.api.stage.as_ref().unwrap();
            assert_eq!(stage.begin["absolute_expires_at"],iso(wall+30_000));
            assert_eq!(stage.begin["idle_expires_at"],iso(wall+30_000));
            assert_eq!(stage.absolute,f.now+30_000);
            assert_eq!(stage.idle,f.now+30_000);
            f.now+=30_000;f.wall=Some(wall/2);
            let status=f.reply(json!({"op":"status","operation_id":UUID2}));
            assert_eq!(status["state"],"expired");
            assert!(f.api.stage.is_none());
        }
    }
    #[test]
    fn calendar_is_utc_and_handles_leap_days(){
        assert_eq!(iso(0),"1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951827696123),"2000-02-29T12:34:56.123Z");
    }
}

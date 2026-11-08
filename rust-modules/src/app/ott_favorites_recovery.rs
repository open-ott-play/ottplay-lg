//! Read-only recovery evidence for the native sealed namespace. No acknowledgement
//! or write is supported: both physical revisions name the SAME inspected value.
use super::*;
use plx_data::ott::favorites_wire::{self as wire, DurableStatus, Outcome, Receipt};
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 16;
const RECORD_LIMIT: usize = 32;
const JOURNAL_BYTES: usize = 65_536;
const METADATA_BYTES: usize = 131_072;
const ENVELOPE_BYTES: usize = 16_384;
const KINDS: [&str; 5] = [
    "channel",
    "channel-name",
    "local-media",
    "plex-media",
    "vportal-media",
];
const ABSENT_SHA256: &str = "74234e98afe7498fb5daf1f36ac2d78acc339464f950703b8c019892f982b90b";
type Result<T> = std::result::Result<T, &'static str>;

pub(super) fn capability() -> Value {
    json!({"version":1})
}

#[derive(Clone)]
struct Cursor {
    after: String,
    ledger: String,
    store: String,
    context: String,
    backend: Option<String>,
}
#[derive(Clone)]
enum Query {
    Page(Option<Cursor>),
    Selected(String),
    Acknowledge,
}

fn exact(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}
fn hash_string(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes()[14] == b'4'
        && b"89ab".contains(&value.as_bytes()[19])
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
fn runtime_name(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn text<'a>(value: &'a Value, key: &str, valid: fn(&str) -> bool) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|value| valid(value))
        .ok_or("invalid_request")
}
fn parse(params: &Value) -> Result<Query> {
    // The raw poll decoder already rejects duplicate keys and nonfinite numbers.
    // This closed schema bounds every valid request well below the existing
    // envelope limit; the reserve matches the existing global transfer adapter.
    if params.to_string().len().saturating_add(160) > ENVELOPE_BYTES {
        return Err("envelope_too_large");
    }
    text(params, "runtime", runtime_name)?;
    if params["subject"] != "global" {
        return Err("invalid_request");
    }
    match params["op"].as_str() {
        Some("inspect") if exact(params, &["op", "runtime", "subject", "operation_id"]) => {
            Ok(Query::Selected(text(params, "operation_id", uuid)?.into()))
        }
        Some("inspect") if exact(params, &["op", "runtime", "subject", "cursor"]) => {
            let value = &params["cursor"];
            if value.is_null() {
                return Ok(Query::Page(None));
            }
            if !exact(
                value,
                &[
                    "after_operation_id",
                    "ledger_revision",
                    "store_revision",
                    "context_revision",
                    "backend_proof",
                ],
            ) {
                return Err("invalid_request");
            }
            let backend = if value["backend_proof"].is_null() {
                None
            } else {
                Some(text(value, "backend_proof", hash_string)?.into())
            };
            let ledger = text(value, "ledger_revision", hash_string)?;
            if ledger == ABSENT_SHA256 {
                return Err("invalid_request");
            }
            Ok(Query::Page(Some(Cursor {
                after: text(value, "after_operation_id", uuid)?.into(),
                ledger: ledger.into(),
                store: text(value, "store_revision", hash_string)?.into(),
                context: text(value, "context_revision", hash_string)?.into(),
                backend,
            })))
        }
        Some("acknowledge") => {
            validate_ack(params)?;
            Ok(Query::Acknowledge)
        }
        _ => Err("invalid_request"),
    }
}

fn validate_ack(params: &Value) -> Result<()> {
    if !exact(
        params,
        &[
            "op",
            "runtime",
            "subject",
            "operation_id",
            "operation_runtime",
            "original_entry_sha256",
            "expected_ledger_revision",
            "expected_store_revision",
            "expected_context_revision",
            "backend_proof",
            "reason",
            "acknowledgement_id",
            "after_ledger_sha256",
            "plan_sha256",
            "accept_unknown",
        ],
    ) || params["reason"] != "accept-unknown-no-replay"
        || params["accept_unknown"] != true
    {
        return Err("invalid_request");
    }
    text(params, "operation_id", uuid)?;
    text(params, "operation_runtime", runtime_name)?;
    for key in [
        "original_entry_sha256",
        "expected_ledger_revision",
        "expected_store_revision",
        "expected_context_revision",
        "backend_proof",
        "acknowledgement_id",
        "after_ledger_sha256",
        "plan_sha256",
    ] {
        text(params, key, hash_string)?;
    }
    if params["expected_ledger_revision"] == ABSENT_SHA256
        || params["after_ledger_sha256"] == ABSENT_SHA256
        || params["after_ledger_sha256"] == params["expected_ledger_revision"]
    {
        return Err("invalid_request");
    }
    let values = vec![
        params["runtime"].clone(),
        json!("global"),
        params["operation_id"].clone(),
        params["operation_runtime"].clone(),
        params["original_entry_sha256"].clone(),
        params["expected_ledger_revision"].clone(),
        params["expected_store_revision"].clone(),
        params["expected_context_revision"].clone(),
        params["backend_proof"].clone(),
        params["reason"].clone(),
    ];
    let mut ack = vec![json!("ottplay-favorites-global-recovery-ack-v1")];
    ack.extend(values.iter().cloned());
    if wire::hash(json!(ack).to_string().as_bytes()) != params["acknowledgement_id"] {
        return Err("invalid_request");
    }
    let mut plan = vec![json!("ottplay-favorites-global-recovery-plan-v1")];
    plan.extend(values);
    plan.push(params["acknowledgement_id"].clone());
    plan.push(params["after_ledger_sha256"].clone());
    if wire::hash(json!(plan).to_string().as_bytes()) != params["plan_sha256"] {
        return Err("invalid_request");
    }
    Ok(())
}

pub(super) fn failure(inv: &Invocation, error: &'static str) -> Value {
    let status = if error == "unsupported_persistence" {
        "unsupported"
    } else {
        "rejected"
    };
    json!({"status":status,"data":{"version":1,"runtime":inv.params["runtime"],"op":inv.params["op"],"subject":"global","error":error}})
}

fn context(app_id: &str, receiver: &str) -> String {
    wire::hash(
        json!([
            "ottplay-native-favorites-recovery-context-v1",
            "namespace-value-v1",
            "ott-state",
            1,
            app_id,
            receiver
        ])
        .to_string()
        .as_bytes(),
    )
}

/// Compare only recovery-relevant persisted state. Live browse/category/resume
/// drift cannot hide records or make recovery depend on an active media source.
/// Optional item UUID metadata, when present, must also name this same snapshot;
/// legacy absence is valid and never triggers migration from a read request.
fn relevant(state: &Saved) -> Result<Value> {
    let value = serde_json::to_value(state).map_err(|_| "storage_unavailable")?;
    Ok(
        json!({"version":value["version"],"favorites":value["favorites"],"receipts":value["receipts"],
        "favorite_items":value.get("favorite_items").unwrap_or(&Value::Null)}),
    )
}

fn validate_receipt(status: &DurableStatus, receipt: &Receipt) -> Result<()> {
    let count = &receipt.counts;
    if receipt.version != 1
        || receipt.subject != "global"
        || receipt.persistence != "persistent"
        || receipt.operation_id != status.operation_id
        || status.operation_runtime.as_deref() != Some(receipt.operation_runtime.as_str())
        || status.transaction_id.as_deref() != Some(receipt.transaction_id.as_str())
        || receipt.outcome != status.outcome
        || receipt.new_revision != receipt.result_document_sha256
        || receipt.changed != (receipt.expected_revision != receipt.new_revision)
        || [
            count.before,
            count.after,
            count.added,
            count.removed,
            count.retained,
            count.updated,
            count.moved,
            count.duplicates_skipped,
        ]
        .iter()
        .any(|n| *n > 1000)
        || count.before != count.removed.saturating_add(count.retained)
        || count.after != count.added.saturating_add(count.retained)
        || count.updated > count.retained
        || count.moved > count.retained
        || (!receipt.changed
            && [count.added, count.removed, count.updated, count.moved]
                .iter()
                .any(|n| *n != 0))
    {
        return Err("storage_unavailable");
    }
    for digest in [
        &receipt.context_revision,
        &receipt.expected_revision,
        &receipt.input_document_sha256,
        &receipt.new_revision,
        &receipt.plan_digest,
        &receipt.result_document_sha256,
    ] {
        if !hash_string(digest) {
            return Err("storage_unavailable");
        }
    }
    if receipt.required_kinds.len() > KINDS.len()
        || receipt
            .required_kinds
            .iter()
            .any(|kind| !KINDS.contains(&kind.as_str()))
        || receipt
            .required_kinds
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err("storage_unavailable");
    }
    Ok(())
}

/// Validate ALL original rows before projecting unresolved rows. Successful or
/// aborted records still contribute to the physical hash and cannot hide damage.
fn records(state: &Saved) -> Result<Vec<Value>> {
    if state.receipts.len() > RECORD_LIMIT
        || serde_json::to_vec(&state.receipts)
            .map_err(|_| "storage_unavailable")?
            .len()
            > JOURNAL_BYTES
    {
        return Err("limits_exceeded");
    }
    let mut ids = BTreeSet::new();
    let mut projected = Vec::new();
    for row in &state.receipts {
        if !uuid(&row.operation_id)
            || !ids.insert(&row.operation_id)
            || !row.operation_runtime.as_deref().is_some_and(runtime_name)
            || !row.transaction_id.as_deref().is_some_and(uuid)
        {
            return Err("storage_unavailable");
        }
        let valid_state = match row.outcome {
            Outcome::Verified => row.state == "committed" && row.receipt.is_some(),
            Outcome::NotApplied => {
                matches!(row.state.as_str(), "aborted" | "expired") && row.receipt.is_none()
            }
            Outcome::Unknown => matches!(row.state.as_str(), "pending" | "unknown"),
        };
        if !valid_state {
            return Err("storage_unavailable");
        }
        if let Some(receipt) = &row.receipt {
            validate_receipt(row, receipt)?;
        }
        if row.outcome == Outcome::Unknown {
            let bytes = serde_json::to_vec(row).map_err(|_| "storage_unavailable")?;
            projected.push(json!({"operation_id":row.operation_id,"operation_runtime":row.operation_runtime,"subject":"global",
                "original_state":row.state,"original_outcome":"unknown","original_entry_sha256":wire::hash(&bytes),"acknowledgement":null}));
        }
    }
    projected.sort_by(|a, b| a["operation_id"].as_str().cmp(&b["operation_id"].as_str()));
    Ok(projected)
}

struct Observation {
    relevant: Value,
    data: Value,
}

/// The caller supplies one authenticated, immutable storage observation. The
/// optional digest is a physical namespace digest, never recomputed from Value.
fn inspect_value(
    value: Option<&Value>,
    digest: Option<&str>,
    query: &Query,
    context: &str,
) -> Result<Observation> {
    let (state, hash) = match (value, digest) {
        (None, None) => (Saved::default(), ABSENT_SHA256),
        (Some(value), Some(digest)) if hash_string(digest) && digest != ABSENT_SHA256 => {
            if value["version"] != 1 {
                return Err("unsupported_persistence");
            }
            (
                serde_json::from_value::<Saved>(value.clone())
                    .map_err(|_| "storage_unavailable")?,
                digest,
            )
        }
        _ => return Err("storage_unavailable"),
    };
    // Journal-specific limits/semantics are checked before generic state checks
    // so malformed terminal rows can never disappear from an apparently empty list.
    let records = records(&state)?;
    state.validate().map_err(|_| "storage_unavailable")?;
    let mut data = json!({"ledger_format":1,"ledger_revision":hash,"store_revision":hash,"context_revision":context,"backend_proof":null});
    match query {
        Query::Page(cursor) => {
            let start = if let Some(cursor) = cursor {
                if cursor.ledger != hash || cursor.store != hash {
                    return Err("revision_conflict");
                }
                if cursor.context != context || cursor.backend.is_some() {
                    return Err("context_changed");
                }
                if records.len() <= PAGE_SIZE
                    || records[PAGE_SIZE - 1]["operation_id"] != cursor.after
                {
                    return Err("invalid_request");
                }
                PAGE_SIZE
            } else {
                0
            };
            let end = records.len().min(start.saturating_add(PAGE_SIZE));
            data["total"] = json!(records.len());
            data["records"] = json!(&records[start..end]);
            data["next_cursor"] = if end < records.len() {
                json!({"after_operation_id":records[end - 1]["operation_id"],"ledger_revision":hash,"store_revision":hash,"context_revision":context,"backend_proof":null})
            } else {
                Value::Null
            };
        }
        Query::Selected(id) => {
            data["operation_id"] = json!(id);
            data["record"] = records
                .iter()
                .find(|record| record["operation_id"] == *id)
                .cloned()
                .unwrap_or(Value::Null);
            data["acknowledge_allowed"] = json!(false);
            for key in ["acknowledgement_id", "after_ledger_sha256", "plan_sha256"] {
                data[key] = Value::Null;
            }
        }
        Query::Acknowledge => return Err("unsupported_persistence"),
    }
    if data.to_string().len().saturating_add(256) > METADATA_BYTES {
        return Err("limits_exceeded");
    }
    Ok(Observation {
        relevant: relevant(&state)?,
        data,
    })
}

struct Work {
    invocation: Invocation,
    query: Query,
    expected: Value,
    receiver: String,
    context: String,
    deadline: plx_base::task::time::Deadline,
}
pub(super) struct Completion {
    work: Work,
    result: Result<Observation>,
}

fn readiness(runtime: &Runtime) -> Result<()> {
    if runtime.saving || runtime.state_load == StateLoad::Loading {
        return Err("busy");
    }
    if runtime.persistence_uncertain {
        return Err("outcome_unknown");
    }
    if !runtime.loaded || !runtime.write_permit.is_valid() {
        return Err("storage_unavailable");
    }
    Ok(())
}

pub(super) fn command(runtime: &mut Runtime, invocation: &Invocation) -> Option<Value> {
    command_with(runtime, invocation, |work, done, tx| {
        super::super::ott_worker::spawn("ott-favorites-inspect", done, move |off| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if work.deadline.expired(off) {
                    return Err("expired");
                }
                let snapshot =
                    private::inspect(Namespace::OttState).map_err(|error| match error {
                        private::InspectError::LegacyOnly => "unsupported_persistence",
                        private::InspectError::Storage(_) => "storage_unavailable",
                    })?;
                if work.deadline.expired(off) {
                    return Err("expired");
                }
                inspect_value(
                    snapshot.as_ref().map(|value| value.value()),
                    snapshot.as_ref().map(|value| value.namespace_sha256()),
                    &work.query,
                    &work.context,
                )
            }))
            .unwrap_or(Err("storage_unavailable"));
            let _ = tx.send(Job::FavoriteRecovery(Completion { work, result }));
            plx_machine::idle::invalidate();
        })
    })
}

fn command_with(
    runtime: &mut Runtime,
    invocation: &Invocation,
    spawn: impl FnOnce(Work, Arc<AtomicBool>, Sender<Job>) -> bool,
) -> Option<Value> {
    let prepared = (|| {
        let query = parse(&invocation.params)?;
        if invocation.params["runtime"] != runtime.remote.runtime()
            || !runtime.remote.invocation_is_current(invocation)
        {
            return Err("runtime_changed");
        }
        if runtime.now_ms() >= invocation.deadline_ms {
            return Err("expired");
        }
        // Unsupported ACK cannot enter the reader, writer, or receipt state.
        if matches!(query, Query::Acknowledge) {
            return Err("unsupported_persistence");
        }
        readiness(runtime)?;
        if !runtime.favorites_recovery_done.load(Ordering::Acquire) {
            return Err("busy");
        }
        let receiver = runtime
            .remote
            .persisted_device_id()
            .ok_or("storage_unavailable")?
            .to_owned();
        let context = context(plx_base::paths::app_id(), &receiver);
        let deadline = runtime
            .clock_sample
            .ok_or("storage_unavailable")?
            .deadline_at_unix_ms(invocation.deadline_ms);
        Ok(Work {
            invocation: invocation.clone(),
            query,
            expected: relevant(&runtime.saved)?,
            receiver,
            context,
            deadline,
        })
    })();
    match prepared {
        Err(error) => Some(failure(invocation, error)),
        Ok(work) => {
            runtime
                .favorites_recovery_done
                .store(false, Ordering::Release);
            if spawn(
                work,
                runtime.favorites_recovery_done.clone(),
                runtime.tx.clone(),
            ) {
                None
            } else {
                runtime
                    .favorites_recovery_done
                    .store(true, Ordering::Release);
                Some(failure(invocation, "storage_unavailable"))
            }
        }
    }
}

pub(super) fn finish(runtime: &mut Runtime, completion: Completion) {
    let Work {
        invocation,
        expected,
        receiver,
        context: captured_context,
        ..
    } = completion.work;
    let result = (|| {
        if invocation.params["runtime"] != runtime.remote.runtime()
            || !runtime.remote.invocation_is_current(&invocation)
        {
            return Err("runtime_changed");
        }
        if runtime.now_ms() >= invocation.deadline_ms {
            return Err("expired");
        }
        readiness(runtime)?;
        if runtime.remote.persisted_device_id() != Some(receiver.as_str())
            || context(plx_base::paths::app_id(), &receiver) != captured_context
        {
            return Err("context_changed");
        }
        let observation = completion.result?;
        if observation.relevant != expected || relevant(&runtime.saved)? != expected {
            return Err("revision_conflict");
        }
        let mut data = observation.data;
        data["version"] = json!(1);
        data["runtime"] = invocation.params["runtime"].clone();
        data["op"] = invocation.params["op"].clone();
        data["subject"] = json!("global");
        Ok(ok(data))
    })();
    // The shared worker releases occupancy only after this actual read exits;
    // neither stale delivery nor a UI lifecycle transition starts another read.
    runtime.remote.complete(
        invocation.clone(),
        result.unwrap_or_else(|error| failure(&invocation, error)),
    );
}

#[cfg(test)]
#[path = "ott_favorites_recovery_tests.rs"]
mod tests;

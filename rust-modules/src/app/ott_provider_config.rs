//! Versioned private provider configuration. Only opaque one-use handles leave
//! this owner; credentials/source fingerprints are never response fields.
use super::*;
use std::io::Read;
const MAX_ENVELOPE: usize = 16 * 1024;
const RECEIPT_LIMIT: usize = 64;
const RECEIPT_TTL: u64 = 120_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    InvalidRequest,
    Unsupported,
    PolicyRestricted,
    RuntimeChanged,
    SourceChanged,
    RevisionConflict,
    InvalidValue,
    StorageFailed,
    Busy,
    Expired,
}
impl Reason {
    fn text(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unsupported => "unsupported",
            Self::PolicyRestricted => "policy_restricted",
            Self::RuntimeChanged => "runtime_changed",
            Self::SourceChanged => "source_changed",
            Self::RevisionConflict => "revision_conflict",
            Self::InvalidValue => "invalid_value",
            Self::StorageFailed => "storage_failed",
            Self::Busy => "busy",
            Self::Expired => "expired",
        }
    }
}
fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
}
fn kind(params: &Value) -> Option<ProviderKind> {
    params["provider"]
        .as_str()
        .and_then(ProviderKind::from_id)
        .filter(|kind| matches!(kind, ProviderKind::Edem | ProviderKind::Operator(_)))
}
pub(super) fn handles(inv: &Invocation) -> bool {
    matches!(
        inv.action.as_str(),
        "settings_compare" | "provider_settings"
    ) && (inv.params.get("version").is_some() || kind(&inv.params).is_some())
}
pub(super) fn capability() -> Value {
    let mut providers = vec!["edem"];
    providers.extend(
        plx_data::ott::OperatorId::ALL
            .iter()
            .map(|id| id.canonical_id()),
    );
    json!({"version":1,"operations":["compare","set"],"providers":providers})
}
fn identity(params: &Value) -> Value {
    json!({"version":1,"runtime":params["runtime"].as_str().filter(|s|token(s)).unwrap_or(""),
        "provider":params["provider"].as_str().filter(|s|s.len()<=64 && !s.chars().any(char::is_control)).unwrap_or(""),
        "number":params["number"].as_u64().filter(|n|(1..=15).contains(n)).unwrap_or(0)})
}
fn rejected_reason(inv: &Invocation, reason: Reason) -> Value {
    let mut data = identity(&inv.params);
    data["reason"] = json!(reason.text());
    json!({"status":if reason==Reason::Unsupported {"unsupported"} else {"rejected"},"data":data})
}
pub(super) fn failure(inv: &Invocation, reason: &str) -> Option<Value> {
    if !handles(inv) {
        return None;
    }
    let reason = match reason {
        "policy_restricted" => Reason::PolicyRestricted,
        "runtime_changed" => Reason::RuntimeChanged,
        "source_changed" => Reason::SourceChanged,
        "revision_conflict" => Reason::RevisionConflict,
        "invalid_value" => Reason::InvalidValue,
        "busy" => Reason::Busy,
        "expired" => Reason::Expired,
        "unsupported" => Reason::Unsupported,
        "invalid_request" => Reason::InvalidRequest,
        _ => Reason::StorageFailed,
    };
    Some(rejected_reason(inv, reason))
}
#[derive(Clone, PartialEq, Eq)]
struct Binding {
    runtime: String,
    authority: u64,
    kind: ProviderKind,
    number: usize,
    source: String,
    active: String,
    edit_epoch: u64,
    navigation_epoch: u64,
}
impl Binding {
    fn capture(runtime: &Runtime, kind: ProviderKind, number: usize) -> Self {
        Self {
            runtime: runtime.remote.runtime().into(),
            authority: runtime.remote.authority_generation(),
            kind,
            number,
            source: revision(&runtime.saved, kind, number),
            active: hash(&json!([
                runtime.saved.profiles.active_profile,
                runtime.active(),
                wire_provider(runtime.provider)
            ])),
            edit_epoch: runtime.profile_edit_epoch,
            navigation_epoch: runtime.navigation_epoch,
        }
    }
    fn current(&self, runtime: &Runtime) -> Result<(), Reason> {
        if self.runtime != runtime.remote.runtime()
            || self.authority != runtime.remote.authority_generation()
        {
            return Err(Reason::RuntimeChanged);
        }
        if self.edit_epoch == 0 || *self != Self::capture(runtime, self.kind, self.number) {
            return Err(Reason::SourceChanged);
        }
        Ok(())
    }
}
struct Receipt {
    context: String,
    revision: String,
    binding: Binding,
    patch: String,
    expires: u64,
}
struct Pending {
    id: String,
    receipt: Receipt,
    candidate: String,
    fields: Vec<String>,
    active: bool,
    generation: u64,
    deadline: u64,
    refusal: Option<Reason>,
}
#[derive(Default)]
pub(super) struct State {
    rows: Vec<Receipt>,
    pending: Option<Pending>,
}
fn random_handle() -> Result<String, Reason> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| Reason::Busy)?;
    use std::fmt::Write;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(&mut result, "{byte:02x}");
    }
    Ok(result)
}
fn patch_valid(kind: ProviderKind, patch: &Value) -> bool {
    let Some(map) = patch.as_object().filter(|m| !m.is_empty()) else {
        return false;
    };
    map.iter()
        .all(|(field, value)| match (kind, field.as_str()) {
            (ProviderKind::Edem, "key") => value.as_str().is_some_and(|s| {
                !s.is_empty()
                    && s.len() <= 512
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            }),
            (ProviderKind::Edem, "portal")
            | (
                ProviderKind::Operator(_),
                "server" | "username" | "password" | "playlist" | "mac",
            ) => value
                .as_str()
                .is_some_and(|s| s.len() <= 8192 && !s.chars().any(char::is_control)),
            (ProviderKind::Edem | ProviderKind::Operator(_), "list") => {
                value.as_u64().is_some_and(|n| n <= 3)
            }
            (ProviderKind::Operator(_), "mode") => value.as_u64().is_some_and(|n| n <= 2),
            _ => false,
        })
}
fn canonical(kind: ProviderKind, source: &str) -> Result<Value, Reason> {
    match kind {
        ProviderKind::Edem => serde_json::to_value(
            plx_data::ott::edem::Config::parse(source).map_err(|_| Reason::InvalidValue)?,
        ),
        ProviderKind::Operator(id) => serde_json::to_value(
            plx_data::ott::operators::Config::parse(id, source)
                .map_err(|_| Reason::InvalidValue)?,
        ),
        _ => return Err(Reason::Unsupported),
    }
    .map_err(|_| Reason::InvalidValue)
}
fn prepare(
    runtime: &Runtime,
    kind: ProviderKind,
    number: usize,
    patch: &Value,
) -> Result<(Saved, bool, String, bool), Reason> {
    if !patch_valid(kind, patch) {
        return Err(Reason::InvalidValue);
    }
    let current =
        profile_index(&runtime.saved, kind, number).map(|i| &runtime.saved.profiles.profiles[i]);
    let prior = current
        .filter(|p| !p.source.is_empty())
        .map(|p| canonical(kind, &p.source))
        .transpose()?;
    let mut value = prior.clone().unwrap_or_else(|| {
        if kind == ProviderKind::Edem {
            json!({"key":"","list":0,"portal":""})
        } else {
            json!({})
        }
    });
    for (key, item) in patch.as_object().ok_or(Reason::InvalidValue)? {
        value[key] = item.clone();
    }
    let value = canonical(kind, &value.to_string())?;
    let matched = prior.as_ref() == Some(&value);
    let mut candidate = runtime.saved.clone();
    let index = insert_profile(&mut candidate, kind, number, value.to_string(), None)
        .map_err(|_| Reason::InvalidValue)?;
    let active =
        candidate.profiles.active_profile.as_ref() == Some(&candidate.profiles.profiles[index].id);
    let signature = revision(&candidate, kind, number);
    Ok((candidate, matched, signature, active))
}
fn parse(inv: &Invocation) -> Result<(ProviderKind, usize), Reason> {
    let raw = inv
        .provider_settings_projection()
        .filter(|raw| raw.len() <= MAX_ENVELOPE)
        .ok_or(Reason::InvalidRequest)?;
    let envelope: Value = serde_json::from_slice(raw).map_err(|_| Reason::InvalidRequest)?;
    if envelope["id"] != inv.id
        || envelope["action"] != inv.action
        || envelope["params"] != inv.params
        || !inv.expects_response()
    {
        return Err(Reason::InvalidRequest);
    }
    let params = &inv.params;
    let expected = if inv.action == "settings_compare" {
        &["version", "runtime", "provider", "number", "settings"][..]
    } else if inv.action == "provider_settings" {
        &[
            "version", "runtime", "provider", "number", "settings", "context", "revision",
        ][..]
    } else {
        return Err(Reason::Unsupported);
    };
    if !fields(params, expected)
        || params["version"].as_u64() != Some(1)
        || !params["runtime"].as_str().is_some_and(token)
    {
        return Err(Reason::InvalidRequest);
    }
    let kind = kind(params).ok_or(Reason::Unsupported)?;
    if params["provider"] != kind.canonical_id() {
        return Err(Reason::InvalidRequest);
    }
    let number = number(params).map_err(|_| Reason::InvalidRequest)?;
    if inv.action == "provider_settings"
        && (!params["context"].as_str().is_some_and(token)
            || !params["revision"].as_str().is_some_and(token))
    {
        return Err(Reason::InvalidRequest);
    }
    if !params["settings"]
        .as_object()
        .is_some_and(|p| !p.is_empty())
    {
        return Err(Reason::InvalidRequest);
    }
    Ok((kind, number))
}
fn admitted(runtime: &Runtime, inv: &Invocation, now: u64) -> Result<(), Reason> {
    if inv.params["runtime"] != runtime.remote.runtime()
        || !runtime.remote.invocation_is_current(inv)
    {
        return Err(Reason::RuntimeChanged);
    }
    if now >= inv.deadline_ms {
        return Err(Reason::Expired);
    }
    if runtime.persistence_uncertain {
        return Err(Reason::StorageFailed);
    }
    if !runtime.loaded
        || runtime.saving
        || runtime.command_receipts.native_provider.pending.is_some()
    {
        return Err(Reason::Busy);
    }
    if runtime.profile_edit_epoch == 0 {
        return Err(Reason::SourceChanged);
    }
    if !matches!(
        runtime.track_selection_kiosk(),
        Ok(super::super::super::ott_kiosk_policy::State::Off)
    ) {
        return Err(Reason::PolicyRestricted);
    }
    Ok(())
}
fn success(
    inv: &Invocation,
    receipt: &Receipt,
    fields: &[String],
    reloaded: bool,
    reload_required: bool,
) -> Value {
    let mut data = identity(&inv.params);
    data["context"] = json!(receipt.context);
    data["revision"] = json!(receipt.revision);
    data["saved"] = json!(true);
    data["fields"] = json!(fields);
    data["reloaded"] = json!(reloaded);
    data["reload_required"] = json!(reload_required);
    ok(data)
}
pub(super) fn command(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    let now = runtime.now_ms();
    command_with(
        runtime,
        inv,
        now,
        |runtime, saved, inv, reload, response| runtime.save_reply(saved, inv, reload, response),
    )
}
fn command_with(
    runtime: &mut Runtime,
    inv: &Invocation,
    now: u64,
    save: impl FnOnce(&mut Runtime, Saved, Invocation, bool, Value) -> Result<(), &'static str>,
) -> Option<Value> {
    let admission_started = std::time::Instant::now();
    let process = || -> Result<(ProviderKind, usize), Reason> {
        let result = parse(inv)?;
        admitted(runtime, inv, now)?;
        Ok(result)
    };
    let (kind, number) = match process() {
        Ok(v) => v,
        Err(e) => return Some(rejected_reason(inv, e)),
    };
    if inv.action == "settings_compare" {
        let (_, matched, _, _) = match prepare(runtime, kind, number, &inv.params["settings"]) {
            Ok(v) => v,
            Err(e) => return Some(rejected_reason(inv, e)),
        };
        let context = match random_handle() {
            Ok(v) => v,
            Err(e) => return Some(rejected_reason(inv, e)),
        };
        let revision = match random_handle() {
            Ok(v) => v,
            Err(e) => return Some(rejected_reason(inv, e)),
        };
        // ClockSample is fixed for this frame; include all admission and OS entropy time
        // before publishing authority, and recheck the invocation generation.
        let published_at = now.saturating_add(
            u64::try_from(admission_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
        if let Err(reason) = admitted(runtime, inv, published_at) {
            return Some(rejected_reason(inv, reason));
        }
        let receipt = Receipt {
            context: context.clone(),
            revision: revision.clone(),
            binding: Binding::capture(runtime, kind, number),
            patch: hash(&inv.params["settings"]),
            expires: published_at.saturating_add(RECEIPT_TTL),
        };
        let rows = &mut runtime.command_receipts.native_provider.rows;
        rows.retain(|r| r.expires > published_at);
        if rows.len() >= RECEIPT_LIMIT {
            rows.remove(0);
        }
        rows.push(receipt);
        let mut data = identity(&inv.params);
        data["context"] = json!(context);
        data["revision"] = json!(revision);
        data["matched"] = json!(matched);
        return Some(ok(data));
    }
    let rows = &mut runtime.command_receipts.native_provider.rows;
    let Some(index) = rows
        .iter()
        .position(|r| inv.params["revision"] == r.revision)
    else {
        return Some(rejected_reason(inv, Reason::RevisionConflict));
    };
    let receipt = rows.remove(index);
    let guard = || -> Result<(), Reason> {
        if receipt.expires <= now {
            return Err(Reason::Expired);
        }
        if inv.params["context"] != receipt.context
            || receipt.binding.kind != kind
            || receipt.binding.number != number
            || receipt.patch != hash(&inv.params["settings"])
        {
            return Err(Reason::RevisionConflict);
        }
        receipt.binding.current(runtime)
    };
    if let Err(e) = guard() {
        return Some(rejected_reason(inv, e));
    }
    let (candidate, matched, signature, active) =
        match prepare(runtime, kind, number, &inv.params["settings"]) {
            Ok(v) => v,
            Err(e) => return Some(rejected_reason(inv, e)),
        };
    let fields = inv.params["settings"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    if matched {
        return Some(success(inv, &receipt, &fields, false, false));
    }
    let response = success(inv, &receipt, &fields, false, active);
    runtime.command_receipts.native_provider.pending = Some(Pending {
        id: inv.id.clone(),
        receipt,
        candidate: signature,
        fields,
        active,
        generation: runtime.generation,
        deadline: inv.deadline_ms,
        refusal: None,
    });
    match save(runtime, candidate, inv.clone(), active, response) {
        Ok(()) => None,
        Err(_) => {
            runtime.command_receipts.native_provider.pending = None;
            Some(rejected_reason(inv, Reason::StorageFailed))
        }
    }
}
/// Called before publishing a Saved image or allowing active-source reload.
pub(super) fn before_saved(runtime: &mut Runtime, inv: Option<&Invocation>) -> bool {
    let Some(inv) = inv else {
        return true;
    };
    let mut state = std::mem::take(&mut runtime.command_receipts.native_provider);
    let result = if let Some(pending) = state.pending.as_mut().filter(|p| p.id == inv.id) {
        pending.refusal = if !runtime.remote.invocation_is_current(inv) {
            Some(Reason::RuntimeChanged)
        } else if runtime.now_ms() >= pending.deadline {
            Some(Reason::Expired)
        } else if !matches!(
            runtime.track_selection_kiosk(),
            Ok(super::super::super::ott_kiosk_policy::State::Off)
        ) {
            Some(Reason::PolicyRestricted)
        } else {
            pending.receipt.binding.current(runtime).err()
        };
        pending.refusal.is_none()
    } else {
        true
    };
    runtime.command_receipts.native_provider = state;
    result
}
/// Called only after the existing verified persistence/effect settlement.
pub(super) fn settled(
    runtime: &mut Runtime,
    inv: &Invocation,
    durable: bool,
    applied: bool,
) -> Option<Value> {
    if !runtime
        .command_receipts
        .native_provider
        .pending
        .as_ref()
        .is_some_and(|p| p.id == inv.id)
    {
        return None;
    }
    let pending = runtime
        .command_receipts
        .native_provider
        .pending
        .take()
        .unwrap();
    if !durable {
        return Some(rejected_reason(inv, Reason::StorageFailed));
    }
    if let Some(reason) = pending.refusal {
        return Some(rejected_reason(inv, reason));
    }
    if !applied {
        return Some(rejected_reason(inv, Reason::StorageFailed));
    }
    if runtime.now_ms() >= pending.deadline {
        return Some(rejected_reason(inv, Reason::Expired));
    }
    if runtime.remote.runtime() != pending.receipt.binding.runtime
        || !runtime.remote.invocation_is_current(inv)
    {
        return Some(rejected_reason(inv, Reason::RuntimeChanged));
    }
    if revision(
        &runtime.saved,
        pending.receipt.binding.kind,
        pending.receipt.binding.number,
    ) != pending.candidate
    {
        return Some(rejected_reason(inv, Reason::SourceChanged));
    }
    let reloaded = pending.active && runtime.generation != pending.generation;
    Some(success(
        inv,
        &pending.receipt,
        &pending.fields,
        reloaded,
        pending.active && !reloaded,
    ))
}
pub(super) fn uncertain(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    settled(runtime, inv, false, false)
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_provider_config_tests.rs"]
mod tests;

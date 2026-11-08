//! Authenticated kiosk commands. Saved remains the only durable policy owner.
//! Pending is an operation fence, never an alternative effective policy.
use super::super::kiosk_source;
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Status,
    On,
    Set,
    Off,
}
struct Request {
    mode: Mode,
    query: Option<String>,
    strict: Option<bool>,
}
const USAGE: &str = "Use kiosk status, on [CHANNEL], set CHANNEL or off.";

fn trim_space(c: char) -> bool {
    matches!(c, '\u{09}'..='\u{0d}' | '\u{20}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn parse(value: &Value) -> Result<Request, &'static str> {
    let object = value.as_object().ok_or(USAGE)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "mode" | "query" | "strict"))
    {
        return Err(USAGE);
    }
    let mode = match object.get("mode").and_then(Value::as_str) {
        Some("status") => Mode::Status,
        Some("on") => Mode::On,
        Some("set") => Mode::Set,
        Some("off") => Mode::Off,
        _ => return Err(USAGE),
    };
    let query = object
        .get("query")
        .map(|value| {
            let text = value.as_str().ok_or(USAGE)?;
            if text.len() > 1024 || text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
                return Err(USAGE);
            }
            let text = text.trim_matches(trim_space);
            if text.is_empty() {
                return Err(USAGE);
            }
            Ok(text.to_owned())
        })
        .transpose()?;
    let strict = object
        .get("strict")
        .map(|value| value.as_bool().ok_or(USAGE))
        .transpose()?;
    if (matches!(mode, Mode::Status | Mode::Off) && (query.is_some() || strict.is_some()))
        || (mode == Mode::Set && query.is_none())
    {
        return Err(USAGE);
    }
    Ok(Request {
        mode,
        query,
        strict,
    })
}

/// One accepted direct command write. Kept until its actual writer settlement,
/// even when the transport/foreground/source retires its activation authority.
pub(super) struct Pending {
    invocation: Invocation,
    expected: String,
    prepared: Option<kiosk_source::Prepared>,
    playback_epoch: u64,
    navigation_epoch: u64,
}
impl Pending {
    fn matches(&self, invocation: &Invocation) -> bool {
        self.invocation.id == invocation.id
            && self.invocation.action == invocation.action
            && self.invocation.params == invocation.params
            && self.invocation.deadline_ms == invocation.deadline_ms
    }
}
fn fingerprint(settings: &BTreeMap<String, Value>) -> String {
    // Absence and a present null are distinct. The latter is invalid, not Off.
    hash(&json!([
        settings.contains_key(stored::KEY),
        settings.get(stored::KEY)
    ]))
}
pub(super) fn pending(runtime: &Runtime) -> bool {
    runtime.command_receipts.kiosk.is_some()
}

fn wire_provider_name(stored: &Stored) -> &str {
    match stored.origin().provider.as_str() {
        "local" => "offline",
        provider => provider,
    }
}
fn snapshot(runtime: &Runtime, app: &App) -> Result<Value, &'static str> {
    if !runtime.loaded || runtime.persistence_uncertain {
        return Err("Kiosk policy is unavailable until protected settings are verified.");
    }
    let policy =
        stored::read(&runtime.saved.settings).map_err(|_| "Invalid stored kiosk policy.")?;
    let health = runtime
        .kiosk_health(app)
        .map_err(|_| "Kiosk playback health is unavailable.")?;
    let mut value = json!({
        "enabled":policy.is_some(), "state":"off", "channel":null, "provider":null,
        "strict":false, "retry_seconds":health.retry_seconds, "retries":health.retries,
        "health":health.health.as_str(), "screen_suspended":health.screen_suspended,
    });
    if let Some(policy) = policy {
        value["state"] = json!(match policy.target() {
            TargetRef::Armed => "waiting",
            _ => "locked",
        });
        // The source owner supplies a validated public projection from actual
        // item/queue/folder ownership. No locator, URL or private path is returned.
        let target =
            kiosk_source::status_target(runtime, app, &policy).map_err(|error| error.message())?;
        let target = target
            .as_object()
            .ok_or("Kiosk target status is unavailable.")?;
        for (key, field) in target {
            if !matches!(
                key.as_str(),
                "channel" | "media" | "folder" | "startup_grace_seconds"
            ) {
                return Err("Kiosk target status is unavailable.");
            }
            value[key.as_str()] = field.clone();
        }
        value["provider"] = json!(wire_provider_name(&policy));
        value["strict"] = json!(policy.strict());
    }
    Ok(value)
}
fn status(runtime: &Runtime, app: &App) -> Value {
    match snapshot(runtime, app) {
        Ok(value) => ok(value),
        Err(error) => rejected(error),
    }
}

pub(super) fn command(
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
) -> Option<Value> {
    if invocation.action != "kiosk" {
        return Some(unsupported("Unsupported kiosk operation."));
    }
    if !invocation.expects_response()
        || !runtime.remote.invocation_is_current(invocation)
        || runtime.now_ms() >= invocation.deadline_ms
    {
        return Some(rejected(
            "Kiosk request is expired or no longer authorized.",
        ));
    }
    let request = match parse(&invocation.params) {
        Ok(request) => request,
        Err(error) => return Some(rejected(error)),
    };
    if request.mode == Mode::Status {
        return Some(status(runtime, app));
    }
    if pending(runtime) {
        return Some(rejected("A kiosk policy write is still in progress."));
    }
    if let Some(error) = runtime.settings_write_refusal() {
        return Some(rejected(error));
    }
    let current = match stored::read(&runtime.saved.settings) {
        Ok(policy) => policy,
        Err(()) => return Some(rejected("Invalid stored kiosk policy.")),
    };
    if request.mode == Mode::Set && current.is_none() {
        return Some(rejected("Enable kiosk mode first."));
    }
    let mut candidate = runtime.saved.clone();
    let mut prepared = None;
    if request.mode == Mode::Off {
        candidate.settings.remove(stored::KEY);
    } else if request.mode == Mode::On && current.is_some() && request.query.is_none() {
        let current = current.as_ref().unwrap();
        let strict = request.strict.unwrap_or(current.strict());
        if strict == current.strict() {
            return Some(status(runtime, app));
        }
        let value = match current.with_strict(strict).encode() {
            Ok(value) => value,
            Err(()) => return Some(rejected("Could not encode kiosk policy.")),
        };
        candidate.settings.insert(stored::KEY.into(), value);
    } else {
        let strict = request
            .strict
            .unwrap_or_else(|| current.as_ref().is_some_and(Stored::strict));
        let selected = match kiosk_source::prepare(runtime, app, request.query.as_deref(), strict) {
            Ok(selected) => selected,
            Err(error) => return Some(rejected(error.message())),
        };
        if let Err(error) = kiosk_source::authorize_designation(runtime, app, &selected) {
            return Some(rejected(error));
        }
        candidate = match kiosk_source::save_candidate(runtime, &selected) {
            Ok(candidate) => candidate,
            Err(error) => return Some(rejected(error.message())),
        };
        prepared = Some(selected);
    }
    if candidate.settings == runtime.saved.settings && prepared.is_none() {
        return Some(status(runtime, app));
    }
    let next = Pending {
        invocation: invocation.clone(),
        expected: fingerprint(&candidate.settings),
        prepared,
        playback_epoch: runtime.playback_epoch,
        navigation_epoch: runtime.navigation_epoch,
    };
    // Never return this placeholder as a success receipt. The owned completion
    // hook replaces it after durable publication and actual effect processing.
    let response = rejected("Kiosk policy write has not settled.");
    match runtime.save_reply(candidate, invocation.clone(), false, response) {
        Ok(()) => {
            runtime.command_receipts.kiosk = Some(next);
            None
        }
        Err(error) => Some(rejected(error)),
    }
}

#[derive(Clone, Copy)]
pub(in super::super) enum Settlement {
    /// Called after Saved publication and ordinary settings application, BEFORE
    /// source kiosk enforcement. This function captures activation authority,
    /// enforces the saved policy, then optionally activates in one owner turn.
    Saved {
        settings_applied: bool,
        target_changed: bool,
    },
    NotApplied,
    Unknown,
}
fn saved_failure(message: &str) -> Value {
    json!({"status":"rejected","data":{"saved":true,"error":message}})
}
pub(super) fn settled(
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
    outcome: Settlement,
) -> Option<Value> {
    if invocation.action != "kiosk" {
        return None;
    }
    let Some(pending) = runtime.command_receipts.kiosk.as_ref() else {
        if matches!(
            outcome,
            Settlement::Saved {
                target_changed: true,
                ..
            }
        ) {
            kiosk_source::apply_durable_policy(runtime, app);
        }
        // An owned writer reply must never fall through to an optimistic generic
        // success if its operation fence was erroneously lost.
        return Some(match outcome {
            Settlement::Saved { .. } => saved_failure("Kiosk policy was saved, but its operation owner is unavailable. Inspect kiosk status."),
            Settlement::NotApplied => rejected("Could not save kiosk policy."),
            Settlement::Unknown => rejected("Kiosk write could not be verified. Reconcile protected settings before retrying."),
        });
    };
    if !pending.matches(invocation) {
        if matches!(
            outcome,
            Settlement::Saved {
                target_changed: true,
                ..
            }
        ) {
            kiosk_source::apply_durable_policy(runtime, app);
        }
        return Some(match outcome {
            Settlement::Saved { .. } => saved_failure("Kiosk policy was saved, but completion ownership changed. Inspect kiosk status."),
            Settlement::NotApplied => rejected("Kiosk completion does not match the pending operation."),
            Settlement::Unknown => rejected("Kiosk write could not be verified and completion ownership changed. Reconcile protected settings."),
        });
    }
    let pending = runtime.command_receipts.kiosk.take().unwrap();
    match outcome {
        Settlement::NotApplied => Some(rejected("Could not save kiosk policy.")),
        Settlement::Unknown => Some(rejected(
            "Kiosk write could not be verified. Reconcile protected settings before retrying.",
        )),
        Settlement::Saved {
            settings_applied,
            target_changed,
        } => {
            // Snapshot authority BEFORE enforcement retires a mismatched old
            // decoder and advances its playback epoch. No input can interleave
            // these synchronous owner steps; activate rechecks source proof.
            let may_activate = runtime.remote.invocation_is_current(&pending.invocation)
                && runtime.now_ms() < pending.invocation.deadline_ms
                && runtime.playback_epoch == pending.playback_epoch
                && runtime.navigation_epoch == pending.navigation_epoch
                && app.running
                && super::super::playback_allowed(app)
                && pending
                    .prepared
                    .as_ref()
                    .is_none_or(|prepared| kiosk_source::still_current(runtime, app, prepared));
            if target_changed {
                kiosk_source::apply_durable_policy(runtime, app);
            }
            if fingerprint(&runtime.saved.settings) != pending.expected {
                return Some(saved_failure(
                    "Kiosk policy was saved, but its current state changed. Inspect kiosk status.",
                ));
            }
            if !settings_applied {
                return Some(saved_failure(
                    "Kiosk policy was saved, but could not be applied. Inspect kiosk status.",
                ));
            }
            if may_activate {
                if let Some(prepared) = pending.prepared.as_ref() {
                    match kiosk_source::activate(runtime, app, prepared) {
                        Ok(Some(attempt)) => runtime.note_kiosk_attempt(attempt),
                        Ok(None) => {},
                        Err(_) => return Some(saved_failure("Kiosk policy was saved, but playback could not start. Inspect kiosk status.")),
                    }
                }
            }
            Some(match snapshot(runtime, app) {
                Ok(value) => ok(value),
                Err(_) => saved_failure(
                    "Kiosk policy was saved, but its status is unavailable. Inspect kiosk status.",
                ),
            })
        }
    }
}

#[cfg(test)]
#[path = "ott_kiosk_commands_tests.rs"]
mod tests;

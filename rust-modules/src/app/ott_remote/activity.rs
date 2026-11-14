//! Passive command ingress evidence, scoped to the current control authority.
//! This is neither an actor audit nor proof that a requested mutation executed.
use super::Invocation;
use serde_json::{json, Value};
use std::collections::VecDeque;

const LIMIT: usize = 8;
#[derive(Default)]
pub(super) struct Activity {
    sequence: u64,
    entries: VecDeque<Entry>,
}
struct Entry {
    id: String,
    sequence: u64,
    action: &'static str,
    at: u64,
    result: &'static str,
}
// Closed labels prevent arbitrary action text from reaching diagnostics. Some
// families include read operations: receipt alone must never imply playback.
fn action(value: &str) -> Option<&'static str> {
    Some(match value {
        "play" => "play", "play_catalog" => "play_catalog",
        "playback" => "playback", "restart" => "restart",
        "provider" => "provider", "profile" => "profile",
        "input" => "input", "command" => "command",
        "local_media" => "local_media", "plex_queue" => "plex_queue",
        "plex_library" => "plex_library", "lifecycle" => "lifecycle",
        "kiosk" => "kiosk", _ => return None,
    })
}
impl Activity {
    pub(super) fn received(&mut self, inv: &Invocation, now: u64) {
        let Some(action) = action(&inv.action) else { return; };
        if self.entries.iter().any(|entry| entry.id == inv.id) { return; }
        self.sequence = self.sequence.saturating_add(1);
        if self.entries.len() == LIMIT { self.entries.pop_front(); }
        self.entries.push_back(Entry { id: inv.id.clone(), sequence: self.sequence,
            action, at: now, result: "pending" });
    }
    pub(super) fn completed(&mut self, inv: &Invocation, result: &Value) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == inv.id) {
            entry.result = match result["status"].as_str() {
                Some("ok") => "ok", Some("rejected") => "rejected",
                Some("unsupported") => "unsupported", _ => "failed",
            };
        }
    }
    pub(super) fn snapshot(&self, now: u64) -> Value {
        json!({"version":1,"scope":"remote_ingress","entries":self.entries.iter().map(|entry|
            json!({"sequence":entry.sequence,"action":entry.action,
                "age_ms":now.saturating_sub(entry.at),"result":entry.result}))
            .collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ott_remote::Remote;

    #[test]
    fn command_activity_is_bounded_redacted_and_read_queries_do_not_evict_evidence() {
        let mut remote = Remote::disabled_for_test("test", "runtime");
        let mut inv = remote.request_for_test("play", json!({"url":"secret-url"}));
        for serial in 0..12 {
            inv.id = format!("private-id-{serial}");
            remote.note_command(&inv, 100 + serial);
        }
        let before = remote.command_activity(200);
        assert_eq!(before["entries"].as_array().unwrap().len(), LIMIT);
        assert_eq!(before["entries"][0]["sequence"], 5);
        assert_eq!(before["entries"][7]["age_ms"], 89);
        for label in ["status", "state", "capabilities", "secret-action"] {
            inv.action = label.into();
            inv.id = format!("read-{label}");
            remote.note_command(&inv, 150);
        }
        assert_eq!(remote.command_activity(200), before);
        let wire = before.to_string();
        assert!(!wire.contains("secret") && !wire.contains("private-id"));
    }

    #[test]
    fn command_activity_tracks_completion_and_rejects_retired_authority() {
        let mut remote = Remote::disabled_for_test("test", "runtime");
        let inv = remote.request_for_test("provider", json!({}));
        remote.note_command(&inv, 100);
        remote.note_command(&inv, 110);
        assert_eq!(remote.command_activity(120)["entries"].as_array().unwrap().len(), 1);
        remote.complete(inv.clone(), json!({"status":"rejected","error":"secret"}));
        assert_eq!(remote.command_activity(120)["entries"][0]["result"], "rejected");
        remote.invalidate();
        remote.note_command(&inv, 130);
        remote.complete(inv, json!({"status":"ok"}));
        assert!(remote.command_activity(140)["entries"].as_array().unwrap().is_empty());

        let mut erased = Remote::disabled_for_test("test", "runtime");
        let current = erased.request_for_test("play", json!({}));
        erased.note_command(&current, 100);
        erased.retire_for_erasure();
        erased.note_command(&current, 110);
        assert!(erased.command_activity(120)["entries"].as_array().unwrap().is_empty(),
            "an erased control authority must not repopulate diagnostic history");
    }
}

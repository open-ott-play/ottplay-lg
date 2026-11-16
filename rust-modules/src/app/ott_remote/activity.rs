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
    /// Separate opt-in projection: the legacy snapshot below stays byte-shape compatible.
    pub(super) fn snapshot_v2(&self, runtime: &str, now: u64) -> Value {
        const MAX_SAFE: u64 = 9_007_199_254_740_991;
        json!({"version":2,"runtime":runtime,"scope":"remote_ingress",
            "entries":self.entries.iter().map(|entry| {
                let request_id = (entry.id.len() == 32 && entry.id.bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
                    .then_some(entry.id.as_str());
                json!({"sequence":entry.sequence.min(MAX_SAFE),"action":entry.action,
                    "age_ms":now.saturating_sub(entry.at).min(MAX_SAFE),
                    "result":entry.result,"request_id":request_id})
            }).collect::<Vec<_>>()})
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

    fn contract() -> Value {
        serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
            "/../ci/ottplay-integration/command-activity-v2.json"))).unwrap()
    }

    #[test]
    fn v2_matches_exact_fixture_and_preserves_v1_projection() {
        let fixture = contract();
        let mut activity = Activity::default();
        activity.entries.push_back(Entry { id:"11111111111111111111111111111111".into(),
            sequence:1, action:"plex_library", at:0, result:"ok" });
        assert_eq!(activity.snapshot_v2("native_123", 1000), fixture["example"]);
        assert_eq!(activity.snapshot(1000), json!({"version":1,"scope":"remote_ingress",
            "entries":[{"sequence":1,"action":"plex_library","age_ms":1000,"result":"ok"}]}));
        activity.entries.clear();
        activity.entries.push_back(Entry { id:"private arbitrary id".into(),
            sequence:2, action:"input", at:1000, result:"pending" });
        assert_eq!(activity.snapshot_v2("native_123", 1000), fixture["legacy_example"]);
        for id in ["", "0123456789ABCDEF0123456789abcdef", "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0", "g123456789abcdef0123456789abcdef",
            "https://user:password@private.invalid"] {
            activity.entries[0].id = id.into();
            assert_eq!(activity.snapshot_v2("native_123", 1000)["entries"][0]["request_id"], Value::Null);
        }
    }

    #[test]
    fn v2_clamps_integers_allows_repeated_max_safe_and_keeps_v1_values() {
        const MAX_SAFE: u64 = 9_007_199_254_740_991;
        let mut activity = Activity::default();
        activity.entries.push_back(Entry { id:"33333333333333333333333333333333".into(),
            sequence:MAX_SAFE, action:"input", at:0, result:"ok" });
        activity.entries.push_back(Entry { id:"44444444444444444444444444444444".into(),
            sequence:u64::MAX, action:"play", at:1, result:"pending" });
        assert_eq!(activity.snapshot_v2("native_123", 1), contract()["clamped_example"]);
        assert_eq!(activity.snapshot_v2("native_123", u64::MAX)["entries"][0]["age_ms"], MAX_SAFE);
        assert_eq!(activity.snapshot(1)["entries"][1]["sequence"], u64::MAX);
        assert_eq!(activity.snapshot_v2("native_123", 0)["entries"][1]["age_ms"], 0);
    }

    #[test]
    fn v2_contract_actions_and_results_are_closed() {
        let fixture = contract();
        for label in fixture["actions"].as_array().unwrap() {
            assert_eq!(action(label.as_str().unwrap()), label.as_str());
        }
        assert_eq!(fixture["actions"].as_array().unwrap().len(), 13);
        assert_eq!(action("command_activity_diagnostics"), None);
        assert_eq!(action("private raw action"), None);
        assert_eq!(fixture["results"], json!(["pending","ok","rejected","unsupported","failed"]));
        let mut remote = Remote::disabled_for_test("test", "native_123");
        let inv = remote.request_for_test("play", json!({"secret":"private"}));
        remote.note_command(&inv, 0);
        for (status, expected) in [("ok","ok"),("rejected","rejected"),
            ("unsupported","unsupported"),("private raw error","failed")] {
            remote.complete(inv.clone(), json!({"status":status,"error":"private raw error"}));
            assert_eq!(remote.command_activity_v2(&inv, 1).unwrap()["entries"][0]["result"], expected);
        }
    }

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

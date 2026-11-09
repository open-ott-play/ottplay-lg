//! Receiver-contract regression through the real settings command, without IO.
//! Vocabulary/bounds mirror Controller v1 (requests_player_settings.go and
//! cli/player_settings.py). The exported default pages also let that unchanged
//! receiver validate actual Native JSON instead of a hand-authored substitute.
use super::*;
use std::collections::BTreeSet;

// Existing descriptor order is the receiver contract; selection policy only
// appends two fields so saved pagination and established aliases stay stable.
const EXPECTED_IDS: [&str; 45] = [
    "uiFontScale", "players", "interfaceMode", "nativeSubtitleFont", "nativeSubtitleColor",
    "nativeSubtitleOutline", "nativeSubtitleBackground", "menuShowProviders", "menuShowProfiles",
    "menuShowCatalog", "menuShowCategories", "menuShowFavorites", "menuShowSearch",
    "menuShowDescription", "menuShowPlay", "menuShowFavorite", "menuShowRefresh", "menuShowAudio",
    "menuShowSubtitles", "menuShowQuality", "menuShowSpeed", "menuShowStats", "m3uMediaUrl",
    "nativeSubtitleSize", "nativeSubtitlePosition", "nativeSubtitleTone", "volumeStep", "sleepTimeout",
    "infoTimeout", "osdOpacity", "permanentTime", "pageSize", "listPosition", "seek13Duration",
    "arrowFun", "showNumber", "showName", "showProgram", "showDescription", "showProgress", "uiColorConfig",
    "nativeSubtitleMode", "nativeSubtitleLanguage", "nativeEpgApiV1", "nativeInputProbeV1",
];

fn keys(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|object| required.iter().all(|key| object.contains_key(*key))
        && object.keys().all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str())))
}

fn text(value: &Value, maximum: usize) -> bool {
    value.as_str().is_some_and(|value| value.len() <= maximum
        && value.chars().all(|ch| ch >= ' ' && ch != '\u{7f}'))
}

fn token(value: &Value) -> bool {
    value.as_str().is_some_and(|value| (1..=128).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)))
}

fn integer(value: &Value) -> bool {
    value.as_i64().is_some_and(|value| (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value))
}

fn typed(value: &Value, kind: &str) -> bool {
    match kind {
        "integer" => integer(value),
        "string" => text(value, 4096),
        "string_array" => value.as_array().is_some_and(|values| values.len() <= 64
            && values.iter().all(|value| text(value, 4096))),
        _ => false,
    }
}

fn descriptor_valid(row: &Value) -> bool {
    if !keys(row, &["id", "type", "scope", "sensitive", "access", "constraints", "apply"],
        &["default_value", "choices", "description", "unit"])
        || !row["id"].as_str().is_some_and(|id| (1..=64).contains(&id.len())
            && id.as_bytes()[0].is_ascii_alphabetic()
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        || !matches!(row["type"].as_str(), Some("integer" | "string" | "string_array"))
        || !matches!(row["scope"].as_str(), Some("application" | "provider"))
        || !row["sensitive"].is_boolean()
        || !matches!(row["apply"].as_str(), Some("immediate" | "next_stream" | "manual" | "after_ack")) {
        return false;
    }
    let kind = row["type"].as_str().unwrap();
    let access = &row["access"];
    if !keys(access, &["read", "write"], &["reason"])
        || !access["read"].is_boolean() || !access["write"].is_boolean()
        || access.get("reason").is_some_and(|reason| !matches!(reason.as_str(), Some(
            "invalid_request" | "atomic_patch_required" | "unknown_setting" | "invalid_value"
            | "runtime_changed" | "source_changed" | "revision_conflict" | "policy_restricted"
            | "local_only" | "unsupported" | "storage_failed" | "effect_failed"
            | "device_forced_auto" | "lifecycle_required" | "local_action_required"))) {
        return false;
    }
    let constraints = &row["constraints"];
    if !keys(constraints, &[], &["minimum", "maximum", "values", "max_utf8_bytes", "max_items"]) {
        return false;
    }
    for key in ["minimum", "maximum"] {
        if constraints.get(key).is_some_and(|value| kind != "integer" || !integer(value)) {
            return false;
        }
    }
    if let (Some(minimum), Some(maximum)) = (constraints["minimum"].as_i64(), constraints["maximum"].as_i64()) {
        if minimum > maximum { return false; }
    }
    for (key, maximum) in [("max_utf8_bytes", 4096), ("max_items", 64)] {
        if constraints.get(key).is_some_and(|value| !value.as_u64().is_some_and(|n| n <= maximum)) {
            return false;
        }
    }
    if constraints.get("values").is_some_and(|values| !values.as_array().is_some_and(|values|
        values.len() <= 256 && values.iter().all(|value| !value.is_array() && typed(value, kind))))
        || row.get("default_value").is_some_and(|value| row["sensitive"] == true || !typed(value, kind))
        || row.get("description").is_some_and(|value| !text(value, 512))
        || row.get("unit").is_some_and(|value| !matches!(value.as_str(), Some("seconds" | "percent"))) {
        return false;
    }
    if let Some(choices) = row.get("choices") {
        let Some(choices) = choices.as_array().filter(|choices| choices.len() <= 256) else { return false; };
        let mut seen = BTreeSet::new();
        for choice in choices {
            if !keys(choice, &["value", "label", "aliases"], &[])
                || !typed(&choice["value"], kind) || !text(&choice["label"], 160) {
                return false;
            }
            let Some(aliases) = choice["aliases"].as_array().filter(|aliases| aliases.len() <= 16) else { return false; };
            for alias in aliases {
                let Some(alias) = alias.as_str() else { return false; };
                if !(1..=64).contains(&alias.len())
                    || !alias.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
                    || !seen.insert(alias.to_ascii_lowercase()) {
                    return false;
                }
            }
        }
    }
    true
}

fn page_valid(data: &Value, request: &Value) -> bool {
    let context = &data["context"];
    let provider_valid = context["provider"].is_null() || context["provider"].as_str().is_some_and(|id|
        (1..=32).contains(&id.len()) && (id.as_bytes()[0].is_ascii_lowercase() || id.as_bytes()[0].is_ascii_digit())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
        && !id.contains("..") && !id.contains("//") && !id.ends_with('/'));
    if !keys(data, &["version", "op", "runtime", "context", "settings", "next_cursor"], &[])
        || data["version"].as_u64() != Some(1) || data["op"] != "schema" || data["runtime"] != request["runtime"]
        || serde_json::to_vec(data).unwrap().len() > 65_536
        || !keys(context, &["token", "provider", "profile"], &[]) || !token(&context["token"]) || !provider_valid
        || !(context["profile"].is_null() || context["profile"].as_u64().is_some_and(|n| (1..=15).contains(&n))) {
        return false;
    }
    let Some(rows) = data["settings"].as_array() else { return false; };
    let mut ids = BTreeSet::new();
    rows.len() <= request["limit"].as_u64().unwrap_or(20) as usize
        && rows.iter().all(|row| descriptor_valid(row) && ids.insert(row["id"].as_str().unwrap()))
        && (data["next_cursor"].is_null() || (!rows.is_empty() && token(&data["next_cursor"])))
}

#[test]
fn every_schema_page_obeys_controller_v1() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(ready_app_fixture());
    let mut runtime = app.ott.take().unwrap();
    let mut failures = BTreeSet::new();
    let mut expected = Vec::new();
    // Default pagination plus every accepted explicit page size. No providers,
    // protected state, sockets, rendering, or media workers are started here.
    for requested_limit in std::iter::once(None).chain((1..=20).map(Some)) {
        let mut request = json!({"op":"schema","runtime":"fixture"});
        if let Some(limit) = requested_limit { request["limit"] = json!(limit); }
        let mut rows = Vec::new();
        let mut context = None;
        let mut cursors = BTreeSet::new();
        let mut complete = false;
        for _ in 0..EXPECTED_IDS.len() {
            // request_for_test uses one fixed request ID; each independent
            // command probe gets a fresh receiver, retaining the runtime token.
            let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
            let mut invocation = remote.request_for_test("player_settings", request.clone());
            invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
            let response = commands::command(&mut runtime, &mut app, &invocation).expect("synchronous schema response");
            assert_eq!(response["status"], "ok", "{response}");
            // Round-trip the actual response before checking the receiving wire contract.
            let data: Value = serde_json::from_slice(&serde_json::to_vec(&response["data"]).unwrap()).unwrap();
            if requested_limit.is_none() {
                println!("SETTINGS_SCHEMA_V1 {}", json!({"request":request,"data":data}));
            }
            if !page_valid(&data, &request) { failures.insert(format!("page limit {requested_limit:?}")); }
            let page = data["settings"].as_array().expect("schema array");
            assert!(!page.is_empty());
            for row in page {
                if !descriptor_valid(row) { failures.insert(row["id"].as_str().unwrap_or("invalid id").to_owned()); }
            }
            rows.extend(page.iter().cloned());
            if let Some(previous) = &context { assert_eq!(previous, &data["context"]); }
            context = Some(data["context"].clone());
            if data["next_cursor"].is_null() { complete = true; break; }
            let cursor = data["next_cursor"].as_str().unwrap();
            assert!(cursors.insert(cursor.to_owned()), "repeated cursor");
            request["cursor"] = json!(cursor);
        }
        assert!(complete, "schema pagination must terminate");
        assert_eq!(rows.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(),
            EXPECTED_IDS, "every descriptor must be reachable in its established order");
        assert_eq!(rows.iter().map(|row| row["id"].as_str().unwrap()).collect::<BTreeSet<_>>().len(), rows.len());
        if requested_limit.is_none() { expected = rows; } else { assert_eq!(rows, expected); }
    }
    let palette = expected.iter().find(|row| row["id"] == "uiColorConfig").unwrap();
    assert!(descriptor_valid(palette));
    assert_eq!(palette["type"], "string");
    assert_eq!(palette["default_value"], "");
    assert_eq!(palette["constraints"]["max_utf8_bytes"], 2048);
    // Exercise the regression oracle itself using valid published metadata.
    let player = expected.iter().find(|row| row["id"] == "players").unwrap();
    assert!(descriptor_valid(player));
    let mut missing_aliases = player.clone();
    missing_aliases["choices"][0].as_object_mut().unwrap().remove("aliases");
    assert!(!descriptor_valid(&missing_aliases));
    let mut duplicate_alias = player.clone();
    duplicate_alias["choices"][0]["aliases"] = json!(["native", "NATIVE"]);
    assert!(!descriptor_valid(&duplicate_alias));
    let mut invalid_unit = player.clone();
    invalid_unit["unit"] = json!("rows");
    assert!(!descriptor_valid(&invalid_unit));
    assert!(failures.is_empty(), "Controller v1 rejects emitted schema metadata: {failures:?}");
}

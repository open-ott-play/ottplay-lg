//! Credential adapters for the established Xtream and Stalker wire requests.
//! Validation uses the native provider codecs; updates retain provider-specific
//! identity fields and are committed before a catalogue reload is requested.
use super::*;

fn active_slot(saved: &Saved, kind: ProviderKind) -> usize {
    (1..=15)
        .find(|number| {
            profile_index(saved, kind, *number).is_some_and(|index| {
                Some(&saved.profiles.profiles[index].id) == saved.profiles.active_profile.as_ref()
            })
        })
        .unwrap_or(1)
}

fn source_value(kind: ProviderKind, source: &str) -> Result<Value, &'static str> {
    let valid = match kind {
        ProviderKind::Xtream => plx_data::ott::xtream::Config::parse(source).is_ok(),
        ProviderKind::Stalker => plx_data::ott::stalker::Config::parse(source).is_ok(),
        _ => false,
    };
    if !valid {
        return Err("Invalid provider credentials");
    }
    serde_json::from_str(source).map_err(|_| "Invalid provider credentials")
}

fn name_key(profile: &ProviderProfile) -> String {
    format!("wireName:{}", profile.id)
}

fn source_patch(
    kind: ProviderKind,
    current: Option<&ProviderProfile>,
    settings: &Value,
) -> Result<String, &'static str> {
    let mut value = match current {
        Some(profile) => source_value(kind, &profile.source)?,
        None => match kind {
            ProviderKind::Xtream => {
                json!({"server":"","username":"","password":"","output":"m3u8"})
            }
            ProviderKind::Stalker => json!({"portal":"","mac":""}),
            _ => return Err("Unsupported provider settings"),
        },
    };
    for (field, item) in settings.as_object().ok_or("Invalid provider settings")? {
        if matches!(field.as_str(), "name" | "profile") {
            continue;
        }
        let text = text(item, 8192, true).ok_or("Invalid provider credentials")?;
        let target = match (kind, field.as_str()) {
            (ProviderKind::Xtream, "server" | "username" | "password") => field.as_str(),
            (ProviderKind::Stalker, "server") => "portal",
            (ProviderKind::Stalker, "mac") => "mac",
            _ => return Err("Unsupported provider settings fields"),
        };
        value[target] = json!(text);
    }
    let serialized = serde_json::to_string(&value).map_err(|_| "Invalid provider credentials")?;
    source_value(kind, &serialized)?;
    // Keep entered bytes for settings_compare; the codec normalizes endpoints
    // and MAC addresses when opening a source, not when storing credentials.
    Ok(serialized)
}

fn compare(saved: &Saved, kind: ProviderKind, params: &Value) -> Value {
    if kind != ProviderKind::Stalker {
        return unsupported("Settings comparison is unavailable for this provider");
    }
    if !fields(params, &["provider", "number", "settings"])
        || !fields(&params["settings"], &["mac", "name", "server"])
    {
        return rejected("Settings comparison is unavailable or invalid");
    }
    let number = match number(params) {
        Ok(number) => number,
        Err(error) => return rejected(error),
    };
    let expected = &params["settings"];
    if text(&expected["name"], 512, true).is_none()
        || text(&expected["server"], 8192, true).is_none()
        // Compare the entered value, which may include codec-accepted padding.
        // Its bound is the enclosing ProviderProfile source, not normalized MAC length.
        || text(&expected["mac"], 16384, true).is_none()
    {
        return rejected("Settings comparison is unavailable or invalid");
    }
    let actual = profile_index(saved, kind, number).map(|index| &saved.profiles.profiles[index]);
    let (portal, mac, name) = if let Some(profile) = actual {
        let value = match source_value(kind, &profile.source) {
            Ok(value) => value,
            Err(error) => return rejected(error),
        };
        (
            value["portal"].clone(),
            value["mac"].clone(),
            saved
                .settings
                .get(&name_key(profile))
                .cloned()
                .unwrap_or_else(|| json!(profile.name)),
        )
    } else {
        (json!(""), json!(""), json!(""))
    };
    ok(json!({"provider":"stalker","number":number,
        "matched":portal==expected["server"] && mac==expected["mac"] && name==expected["name"]}))
}

/// Called only for provider_settings/settings_compare targeting these providers,
/// or profile_settings while one of them is selected.
pub(super) fn command(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    if super::provider_config::handles(inv) {
        return super::provider_config::command(runtime, inv);
    }
    if inv.action == "profile_settings" {
        return Some(unsupported(
            "Numbered profile editing is unavailable for this provider; use provider settings",
        ));
    }
    let params = &inv.params;
    let kind = match params["provider"].as_str() {
        Some("xtream") => ProviderKind::Xtream,
        Some("stalker") => ProviderKind::Stalker,
        _ => return Some(rejected("Unsupported provider settings")),
    };
    if !runtime.loaded || runtime.saving || inv.deadline_ms <= runtime.now_ms() {
        return Some(rejected(
            "Provider settings are not ready or the request expired",
        ));
    }
    if inv.action == "settings_compare" {
        return Some(compare(&runtime.saved, kind, params));
    }
    if inv.action != "provider_settings" {
        return Some(unsupported("Unsupported provider settings operation"));
    }
    if runtime.provider != kind {
        return Some(rejected(
            "Select this provider before changing its settings",
        ));
    }
    if !fields(params, &["provider", "settings"]) {
        return Some(rejected("Invalid provider settings parameters"));
    }
    let settings = &params["settings"];
    let active = active_slot(&runtime.saved, kind);
    let numbered = kind == ProviderKind::Stalker && settings.get("profile").is_some();
    let number = if numbered {
        if !fields(settings, &["profile", "name", "server", "mac"]) {
            return Some(rejected("Unsupported Stalker profile settings fields"));
        }
        match settings["profile"]
            .as_u64()
            .filter(|number| (1..=15).contains(number))
        {
            Some(number) => number as usize,
            None => return Some(rejected("Choose a profile number from 1 to 15")),
        }
    } else {
        let allowed = if kind == ProviderKind::Xtream {
            &["server", "username", "password"][..]
        } else {
            &["server", "mac"][..]
        };
        if !subset(settings, allowed) {
            return Some(rejected("Unsupported provider settings fields"));
        }
        active
    };
    let name = match settings.get("name") {
        Some(value) => match text(value, 256, true) {
            Some(value) => Some(value.to_owned()),
            None => return Some(rejected("Invalid profile name")),
        },
        None => None,
    };
    let current = profile_index(&runtime.saved, kind, number)
        .map(|index| &runtime.saved.profiles.profiles[index]);
    let source = match source_patch(kind, current, settings) {
        Ok(source) => source,
        Err(error) => return Some(rejected(error)),
    };
    let mut candidate = runtime.saved.clone();
    let index = match insert_profile(&mut candidate, kind, number, source, name.clone()) {
        Ok(index) => index,
        Err(error) => return Some(rejected(error)),
    };
    if let Some(name) = name {
        candidate
            .settings
            .insert(name_key(&candidate.profiles.profiles[index]), json!(name));
    }
    if number == active {
        candidate.profiles.active_profile = Some(candidate.profiles.profiles[index].id.clone());
    }
    let changed = candidate.profiles != runtime.saved.profiles
        || candidate.settings != runtime.saved.settings;
    let reload = changed
        && candidate.profiles.active_profile.as_ref()
            == Some(&candidate.profiles.profiles[index].id);
    let mut result = json!({"provider":wire_provider(kind),"saved":true,
        "fields":settings.as_object().map(|settings|settings.keys().collect::<Vec<_>>()).unwrap_or_default()});
    if numbered {
        result["profile"] = json!(number);
    }
    defer(runtime, inv, candidate, reload, ok(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(kind: ProviderKind, source: Value) -> ProviderProfile {
        ProviderProfile {
            id: "test".into(),
            name: "Test".into(),
            kind,
            source: source.to_string(),
            source_alias: String::new(),
            enabled: true,
        }
    }

    #[test]
    fn xtream_patch_preserves_output_and_credentials_not_in_patch() {
        let original = profile(
            ProviderKind::Xtream,
            json!({"server":"https://example.test","username":"user","password":"secret","output":"ts"}),
        );
        let patched = source_patch(
            ProviderKind::Xtream,
            Some(&original),
            &json!({"password":"updated"}),
        )
        .unwrap();
        let config = plx_data::ott::xtream::Config::parse(&patched).unwrap();
        assert_eq!(config.output, "ts");
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "updated");
        assert!(source_patch(
            ProviderKind::Xtream,
            Some(&original),
            &json!({"output":"m3u8"})
        )
        .is_err());
    }

    #[test]
    fn stalker_wire_aliases_preserve_device_identity_and_exact_entered_bytes() {
        let original = profile(
            ProviderKind::Stalker,
            json!({"portal":"https://example.test/c/","mac":"00:1a:79:00:00:01","sn":"serial","device_id":"identity","stb_type":"MAG254","language":"en","timezone":"UTC","include_vod":false}),
        );
        let patched = source_patch(
            ProviderKind::Stalker,
            Some(&original),
            &json!({"server":"https://new.example.test/c/","mac":"00:1a:79:00:00:02"}),
        )
        .unwrap();
        let config = plx_data::ott::stalker::Config::parse(&patched).unwrap();
        assert_eq!(config.sn, "serial");
        assert_eq!(config.device_id, "identity");
        assert!(!config.include_vod);
        let raw: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(raw["portal"], "https://new.example.test/c/");
        assert_eq!(raw["mac"], "00:1a:79:00:00:02");
        assert!(raw.get("server").is_none());
        assert!(source_patch(
            ProviderKind::Stalker,
            Some(&original),
            &json!({"username":"00:1A:79:00:00:03"})
        )
        .is_err());
    }

    #[test]
    fn source_patch_rejects_incomplete_credentials_and_wrong_types() {
        assert!(source_patch(
            ProviderKind::Xtream,
            None,
            &json!({"server":"https://example.test"})
        )
        .is_err());
        assert!(source_patch(
            ProviderKind::Stalker,
            None,
            &json!({"server":"https://example.test","mac":1})
        )
        .is_err());
        assert!(source_patch(
            ProviderKind::Stalker,
            None,
            &json!({"server":"https://example.test","mac":"invalid"})
        )
        .is_err());
        assert!(source_patch(
            ProviderKind::Xtream,
            None,
            &json!({"server":"https://example.test","username":"user","password":"secret"})
        )
        .is_ok());
    }

    #[test]
    fn compare_uses_numbered_stalker_and_returns_no_credentials() {
        let mut saved = Saved::default();
        saved.profiles.profiles.push(profile(
            ProviderKind::Stalker,
            json!({"portal":"https://example.test/c/","mac":"00:1a:79:00:00:01"}),
        ));
        saved.settings.insert("slot:test".into(), json!(9));
        let params = json!({"provider":"stalker","number":9,"settings":{"server":"https://example.test/c/","mac":"00:1a:79:00:00:01","name":"Test"}});
        let response = compare(&saved, ProviderKind::Stalker, &params);
        assert_eq!(response["data"]["matched"], true);
        assert!(!response.to_string().contains("example.test"));
        assert!(!response.to_string().contains("00:1a"));
        assert_eq!(
            compare(&saved, ProviderKind::Xtream, &params)["status"],
            "unsupported"
        );
    }


    #[test]
    fn stalker_compare_accepts_valid_padded_mac_without_normalizing_it() {
        let canonical = "00:1a:79:00:00:01";
        for padding in [1, 4000, 5000] {
            let raw = format!("{}{}{}", " ".repeat(padding), canonical, " ".repeat(padding));
            let source = json!({"portal":"https://fixture.invalid/c/", "mac":raw});
            let original = profile(ProviderKind::Stalker, source);
            original.validate().unwrap();
            if raw.len() <= 8192 {
                assert!(source_patch(ProviderKind::Stalker, None,
                    &json!({"server":"https://fixture.invalid/c/", "mac":raw})).is_ok());
            }
            let mut saved = Saved::default();
            saved.profiles.profiles.push(original);
            let mut params = json!({"provider":"stalker", "number":1, "settings":{
                "server":"https://fixture.invalid/c/", "mac":raw, "name":"Test"
            }});
            let response = compare(&saved, ProviderKind::Stalker, &params);
            assert_eq!(response["status"], "ok");
            assert_eq!(response["data"]["matched"], true);
            assert!(!response.to_string().contains(canonical));
            assert!(!response.to_string().contains("fixture.invalid"));
            params["settings"]["mac"] = json!(canonical);
            assert_eq!(compare(&saved, ProviderKind::Stalker, &params)["data"]["matched"], false,
                "comparison remains exact raw bytes, not codec-normalized equality");
        }
    }

    #[test]
    fn stalker_compare_keeps_controls_and_oversize_refused_and_empty_slot_supported() {
        let saved = Saved::default();
        let mut params = json!({"provider":"stalker", "number":1, "settings":{
            "server":"", "mac":"", "name":""
        }});
        assert_eq!(compare(&saved, ProviderKind::Stalker, &params)["data"]["matched"], true);
        for invalid in ["\n".to_owned(), "\u{7f}".to_owned(), "\u{85}".to_owned(), " ".repeat(16385)] {
            params["settings"]["mac"] = json!(invalid);
            let result = compare(&saved, ProviderKind::Stalker, &params);
            assert_eq!(result["status"], "rejected");
            assert!(result["data"].get("matched").is_none());
        }
    }

    #[test]
    fn compare_accepts_saved_name_byte_boundary_without_mutation_or_echo() {
        for (name, different) in [
            ("x".repeat(512), "y".repeat(512)),
            ("é".repeat(256), "è".repeat(256)),
        ] {
            assert_eq!(name.len(), 512);
            let mut original = profile(
                ProviderKind::Stalker,
                json!({"portal":"https://example.test/c/","mac":"00:1a:79:00:00:01"}),
            );
            original.name = name.clone();
            original.validate().unwrap();
            let mut saved = Saved::default();
            saved.profiles.active_profile = Some(original.id.clone());
            saved.profiles.profiles.push(original);
            saved.settings.insert("slot:test".into(), json!(9));
            let before = serde_json::to_value(&saved).unwrap();
            let mut params = json!({"provider":"stalker","number":9,"settings":{
                "server":"https://example.test/c/","mac":"00:1a:79:00:00:01","name":name
            }});
            assert_eq!(
                compare(&saved, ProviderKind::Stalker, &params),
                json!({"status":"ok","data":{"provider":"stalker","number":9,"matched":true}})
            );
            params["settings"]["name"] = json!(different);
            assert_eq!(
                compare(&saved, ProviderKind::Stalker, &params),
                json!({"status":"ok","data":{"provider":"stalker","number":9,"matched":false}})
            );
            assert_eq!(serde_json::to_value(&saved).unwrap(), before);
        }
    }

    #[test]
    fn compare_rejects_oversized_and_control_names_without_mutation() {
        let original = profile(
            ProviderKind::Stalker,
            json!({"portal":"https://example.test/c/","mac":"00:1a:79:00:00:01"}),
        );
        original.validate().unwrap();
        let mut saved = Saved::default();
        saved.profiles.active_profile = Some(original.id.clone());
        saved.profiles.profiles.push(original.clone());
        saved.settings.insert("slot:test".into(), json!(9));
        let before = serde_json::to_value(&saved).unwrap();
        for name in [
            "x".repeat(513),
            "é".repeat(257),
            "name\u{0000}suffix".to_owned(),
            "name\u{001f}suffix".to_owned(),
            "name\u{007f}suffix".to_owned(),
            "name\u{0085}suffix".to_owned(),
        ] {
            let mut invalid = original.clone();
            invalid.name = name.clone();
            assert!(invalid.validate().is_err());
            let params = json!({"provider":"stalker","number":9,"settings":{
                "server":"https://example.test/c/","mac":"00:1a:79:00:00:01","name":name
            }});
            let result = compare(&saved, ProviderKind::Stalker, &params);
            assert_eq!(result["status"], "rejected");
            assert!(result["data"].get("matched").is_none());
            assert_eq!(serde_json::to_value(&saved).unwrap(), before);
        }
    }
}

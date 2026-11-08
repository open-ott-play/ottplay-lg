//! Bounded quality metadata. URLs and session controls never become UI row identifiers.
use super::*;

pub(crate) const MAX_CHOICES: usize = 16;
pub(crate) const MAX_LABEL_BYTES: usize = 256;

#[derive(Clone)]
pub(crate) struct Variant {
    pub key: String,
    pub label: String,
    pub url: String,
    pub session: Option<SessionHints>,
}
impl std::fmt::Debug for Variant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Variant(<private>)") }
}
pub(crate) struct ParsedChoices {
    pub variants: Vec<Variant>,
    pub selected: Variant,
    pub sessions: Vec<SessionHints>,
    pub failure: Option<&'static str>,
}
pub(crate) fn valid_key(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= MAX_LABEL_BYTES
        && !key.chars().any(char::is_control) && !key.contains("://")
}

pub(super) fn resolve(
    profile: &ProviderProfile, entry: &CatalogEntry, cancel: &AtomicBool,
    preferred: Option<&str>, interactive: bool, fetch: &mut Fetch<'_>,
) -> Result<ParsedChoices> {
    active(cancel)?;
    let link = link(profile)?;
    let MediaLocator::VPortal { request: params } = &entry.media else {
        return Err("Invalid VPortal media locator");
    };
    let text = request(&link, params, cancel, fetch)?;
    let data: Value = serde_json::from_str(&text).map_err(|_| "Invalid VPortal playback response")?;
    Ok(parse(&link.endpoint, &data, preferred, interactive))
}

fn parse(endpoint: &str, data: &Value, preferred: Option<&str>, interactive: bool) -> ParsedChoices {
    let variants = data.get("variants").and_then(Value::as_object);
    // Preserve the existing automatic fallback even if the interactive list is too large.
    let selected_pair = preferred.and_then(|key| variants?.get(key).map(|value| (key, value)))
        .filter(|(_, value)| playable_url(endpoint, Some(value)).is_some());
    let default = data.get("url").filter(|value| playable_url(endpoint, Some(value)).is_some())
        .or_else(|| variants?.values().find(|value| playable_url(endpoint, Some(value)).is_some()));
    let selected_value = selected_pair.map(|(_, value)| value).or(default);
    let selected_url = playable_url(endpoint, selected_value).unwrap_or_default();
    let selected = Variant {
        key: selected_pair.map_or_else(String::new, |(key, _)| key.to_owned()),
        label: selected_pair.map_or_else(String::new, |(key, _)| key.to_owned()),
        session: selected_value.and_then(|value| session_hints(endpoint, data, value.as_str().unwrap_or(""), &selected_url)),
        url: selected_url,
    };
    let mut output = ParsedChoices { variants: Vec::new(), selected, sessions: Vec::new(), failure: None };
    if data.get("type").and_then(Value::as_str) == Some("error") {
        output.failure = Some("VPortal could not resolve this video");
    } else if output.selected.url.is_empty() {
        output.failure = Some("VPortal returned no playable source");
    }
    // Top-level allocation may exist even on failure. The same strict origin/path grammar applies.
    if let Some(stop) = data.get("stop").and_then(Value::as_str).and_then(|value| session_url(endpoint, value)) {
        add_session(&mut output, SessionHints { stop, heartbeat: None });
    }
    if let Some(session) = output.selected.session.clone() { add_session(&mut output, session); }
    if let Some(variants) = variants {
        let mut admitted = 0;
        for (key, value) in variants {
            let Some(url) = playable_url(endpoint, Some(value)) else { continue; };
            if admitted == MAX_CHOICES {
                if interactive { output.failure = Some("VPortal returned too many quality choices"); }
                break;
            }
            admitted += 1;
            let session = session_hints(endpoint, data, value.as_str().unwrap_or(""), &url);
            if let Some(session) = session.clone() { add_session(&mut output, session); }
            if !valid_key(key) {
                if interactive { output.failure = Some("VPortal returned an invalid quality label"); }
                continue;
            }
            output.variants.push(Variant { key: key.clone(), label: key.clone(), url, session });
        }
    }
    output
}

fn add_session(output: &mut ParsedChoices, session: SessionHints) {
    if let Some(existing) = output.sessions.iter_mut().find(|existing| existing.stop == session.stop) {
        // A top-level cleanup-only claim may be enriched by its selected playback heartbeat.
        if existing.heartbeat.is_none() { existing.heartbeat = session.heartbeat; }
        return;
    }
    output.sessions.push(session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ENDPOINT: &str = "https://fixture.invalid/nas/api";
    #[test]
    fn keeps_names_and_default_without_exporting_urls_as_keys() {
        let value = json!({"url":"/default.mp4","variants":{"High":"/high.mp4","Low":"/low.mp4"}});
        let choices = parse(ENDPOINT, &value, None, true);
        assert_eq!(choices.variants.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["High", "Low"]);
        assert!(choices.selected.url.ends_with("/default.mp4"));
        assert!(choices.failure.is_none());
        assert_eq!(format!("{:?}", choices.variants[0]), "Variant(<private>)");
    }
    #[test]
    fn preference_selects_exact_key_and_missing_preference_keeps_default() {
        let value = json!({"url":"/default.mp4","variants":{"High":"/high.mp4","Low":"/low.mp4"}});
        assert!(parse(ENDPOINT, &value, Some("High"), false).selected.url.ends_with("/high.mp4"));
        assert!(parse(ENDPOINT, &value, Some("high"), false).selected.url.ends_with("/default.mp4"));
    }
    #[test]
    fn oversized_interactive_list_fails_but_automatic_fallback_is_preserved() {
        let variants: serde_json::Map<String, Value> = (0..MAX_CHOICES+1)
            .map(|index| (format!("quality-{index:02}"), Value::String(format!("/{index}.mp4")))).collect();
        let value = json!({"url":"/default.mp4","variants":variants});
        let interactive = parse(ENDPOINT, &value, None, true);
        assert_eq!(interactive.variants.len(), MAX_CHOICES);
        assert_eq!(interactive.failure, Some("VPortal returned too many quality choices"));
        let automatic = parse(ENDPOINT, &value, Some("quality-16"), false);
        assert!(automatic.failure.is_none());
        assert!(automatic.selected.url.ends_with("/16.mp4"));
    }
    #[test]
    fn labels_are_bounded_without_truncating_the_preference_key() {
        assert!(valid_key(&"a".repeat(MAX_LABEL_BYTES)));
        for bad in ["a".repeat(MAX_LABEL_BYTES+1), "\nHigh".into(), "https://fixture.invalid/private".into(), " ".into()] {
            assert!(!valid_key(&bad));
            let value = json!({"variants":{(bad):"/video.mp4"}});
            assert_eq!(parse(ENDPOINT, &value, None, true).failure, Some("VPortal returned an invalid quality label"));
            assert!(parse(ENDPOINT, &value, None, false).failure.is_none());
        }
    }
    fn allocated(count: usize) -> Value {
        let mut variants = serde_json::Map::new();
        let mut sessions = serde_json::Map::new();
        for index in 0..count {
            let url = format!("/video-{index}.mp4");
            variants.insert(format!("Q{index:02}"), Value::String(url.clone()));
            sessions.insert(url, json!({"stop":format!("https://fixture.invalid/nas/stream/session{index}.secret/media.stop")}));
        }
        json!({"variants":variants,"sessions":sessions})
    }
    #[test]
    fn exact_admission_limit_keeps_all_named_choices_and_unique_cleanup_claims() {
        let parsed = parse(ENDPOINT, &allocated(MAX_CHOICES), None, true);
        assert_eq!(parsed.variants.len(), MAX_CHOICES);
        assert_eq!(parsed.sessions.len(), MAX_CHOICES);
        assert!(parsed.failure.is_none());
    }
    #[test]
    fn overflow_keeps_cleanup_for_admitted_choices_without_admitting_extra_controls() {
        let parsed = parse(ENDPOINT, &allocated(MAX_CHOICES+1), None, true);
        assert_eq!(parsed.failure, Some("VPortal returned too many quality choices"));
        assert_eq!(parsed.sessions.len(), MAX_CHOICES);
        assert!(parsed.sessions.iter().all(|session| !session.stop.contains("session16.")));
    }
    #[test]
    fn repeated_stop_identity_is_owned_once_and_unreferenced_or_foreign_controls_are_ignored() {
        let stop = "https://fixture.invalid/nas/stream/shared.secret/media.stop";
        let value = json!({"variants":{"A":"/a.mp4","B":"/b.mp4","C":"/c.mp4"},
            "sessions":{"/a.mp4":{"stop":stop},"/b.mp4":{"stop":stop},
                "/c.mp4":{"stop":"https://other.invalid/nas/stream/shared.secret/media.stop"},
                "/unreferenced.mp4":{"stop":"https://fixture.invalid/nas/stream/unreferenced.secret/media.stop"}}});
        let parsed = parse(ENDPOINT, &value, None, true);
        assert_eq!(parsed.sessions.len(), 1);
        assert_eq!(parsed.sessions[0].stop, "https://fixture.invalid:443/nas/stream/shared.secret/media.stop");
    }
    #[test]
    fn failed_response_retains_a_valid_top_level_cleanup_claim() {
        let value = json!({"type":"error","stop":"https://fixture.invalid/nas/stream/failed.secret/media.stop"});
        let parsed = parse(ENDPOINT, &value, None, true);
        assert_eq!(parsed.failure, Some("VPortal could not resolve this video"));
        assert_eq!(parsed.sessions.len(), 1);
        assert!(parsed.variants.is_empty());
    }

}

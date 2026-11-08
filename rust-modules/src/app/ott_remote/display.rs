//! Bounded display metadata shared by status and presence. Never a URL or secret.

pub(super) fn text(value: &str, limit: usize) -> Option<String> {
    // Check before trim: a control character must not disappear into an otherwise
    // acceptable label. Match the browser's conservative ASCII classification
    // fallback without altering the Unicode label returned to the operator.
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = value.trim();
    let classification: String = value.chars().filter(char::is_ascii).collect();
    if value.is_empty() || private_text(value) || private_text(classification.trim()) {
        return None;
    }
    let mut result = String::new();
    for character in value.chars() {
        if result.len() + character.len_utf8() > limit {
            break;
        }
        result.push(character);
    }
    Some(result)
}

fn private_text(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if value.contains("://")
        || value.starts_with(['/', '\\'])
        || value.starts_with("~/")
        || value.starts_with("~\\")
        || ["data:", "blob:", "file:"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
        || drive
    {
        return true;
    }
    for key in [
        "authorization",
        "bearer",
        "password",
        "token",
        "secret",
        "apikey",
        "api_key",
        "api-key",
        "api key",
    ] {
        for (offset, _) in value.match_indices(key) {
            // Credential syntax requires a word boundary and separator; a title
            // containing just "Token" or "Secret" is ordinary display metadata.
            if offset > 0
                && (bytes[offset - 1].is_ascii_alphanumeric() || bytes[offset - 1] == b'_')
            {
                continue;
            }
            let rest = &value[offset + key.len()..];
            let stripped = rest.trim_start();
            let private = match key {
                "bearer" => stripped.len() < rest.len(),
                "authorization" => stripped.starts_with(':'),
                _ => stripped.starts_with([':', '=']),
            };
            if private {
                return true;
            }
        }
    }
    false
}

/// The retained player-state v1 protocol accepts only its original simple IDs.
pub(super) fn retained_media_id(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)))
    .then_some(value)
}

/// Live status additionally preserves generated catalog IDs for correlation.
pub(super) fn media_id(value: &str) -> Option<&str> {
    if let Some(id) = retained_media_id(value) {
        return Some(id);
    }
    // Catalog identity is the validated profile ID plus ':' and the existing
    // 16-digit lowercase fingerprint. Keep the full ID for correlation, but
    // do not admit arbitrary colon-bearing text, URLs or credential syntax.
    if value.len() > 128 + 1 + 16 {
        return None;
    }
    let (profile, fingerprint) = value.rsplit_once(':')?;
    (!profile.is_empty()
        && profile.len() <= 128
        && profile.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
        && fingerprint.len() == 16
        && fingerprint.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && !private_text(value))
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_titles_and_wire_ids_keep_their_identity() {
        for value in [
            "MyZen TV HD",
            "Том и Джерри",
            "日本語の映画",
            "Token",
            "Secret Window",
            "Password",
            "API key documentary",
            "Part 1: The Beginning",
        ] {
            assert_eq!(text(value, 256).as_deref(), Some(value));
        }
        for value in ["42", "m3u.channel-1", "local_4d3738", "A_B-01"] {
            assert_eq!(media_id(value), Some(value));
        }
    }

    #[test]
    fn urls_paths_and_credentials_never_become_display_metadata() {
        for value in [
            "https://example.invalid/video?token=sentinel",
            "file:/private/movie",
            "DATA:sentinel",
            "blob:sentinel",
            "/private/movie",
            "~/Movies/movie",
            "C:\\private\\movie",
            "Authorization: Basic sentinel",
            "Bearer sentinel",
            "password = sentinel",
            "token:sentinel",
            "secret=sentinel",
            "api_key:sentinel",
            "api-key = sentinel",
            "API key:sentinel",
        ] {
            assert!(text(value, 256).is_none(), "unsafe display text: {value:?}");
        }
    }

    #[test]
    fn invisible_characters_cannot_hide_credentials_or_urls() {
        for value in [
            "\u{200b}file:sentinel",
            "ht\u{200d}tps://example.invalid",
            "to\u{2060}ken=sentinel",
            "\u{e000}/private/movie",
            "\u{feff}C:\\private\\movie",
            "Bearer\u{a0}sentinel",
            "Authoriz\u{202e}ation:sentinel",
            "token\u{200b} = sentinel",
            "\nordinary title",
            "ordinary title\t",
        ] {
            assert!(
                text(value, 256).is_none(),
                "hidden private display text: {value:?}"
            );
        }
    }

    #[test]
    fn bounded_text_is_classified_before_utf8_safe_truncation() {
        assert_eq!(text("  café  ", 4).as_deref(), Some("caf"));
        assert_eq!(text("日文", 3).as_deref(), Some("日"));
        assert!(text("ordinary title token=sentinel", 8).is_none());
        assert!(text("  ", 256).is_none());
        assert!(media_id("https://example.invalid").is_none());
        assert!(media_id("private:42").is_none());
        assert!(media_id(&"a".repeat(129)).is_none());
    }

    #[test]
    fn generated_catalog_media_ids_preserve_the_full_profile_and_fingerprint() {
        for profile in ["portal-home".to_string(), "native:vportal:Home_2.1".into(), "p".repeat(128)] {
            let id = format!("{profile}:0123456789abcdef");
            assert_eq!(media_id(&id), Some(id.as_str()), "{id}");
        }
        assert!(media_id(&format!("{}:0123456789abcdef", "p".repeat(129))).is_none());
        // A larger bound applies only to the generated form, never arbitrary wire IDs.
        assert!(media_id(&"p".repeat(145)).is_none());
    }

    #[test]
    fn generated_catalog_media_ids_reject_private_or_noncanonical_input() {
        for id in [
            "private:42", ":0123456789abcdef", "portal:",
            "portal:0123456789abcde", "portal:0123456789abcdef0",
            "portal:0123456789abcdeF", "portal:0123456789abcdeg",
            " portal:0123456789abcdef", "portal:0123456789abcdef ",
            "portal:0123456789abcdef\n", "portal:\u{200b}0123456789abcdef",
            "https://example.invalid:0123456789abcdef", "/private:0123456789abcdef",
            "~/private:0123456789abcdef", "C:\\private:0123456789abcdef",
            "portal:0123456789abcdef?token=sentinel", "user:password@host:0123456789abcdef",
            "token:0123456789abcdef", "safe:password:0123456789abcdef",
            "Authorization:0123456789abcdef", "api_key:0123456789abcdef",
            "secret:0123456789abcdef", "data:0123456789abcdef",
            "BLOB:0123456789abcdef", "file:0123456789abcdef",
        ] {
            assert!(media_id(id).is_none(), "private or malformed media ID: {id:?}");
        }
    }
}

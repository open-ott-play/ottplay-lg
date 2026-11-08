use super::caseless_key;

#[test]
fn canonical_caseless_keys_preserve_the_project_search_contract() {
    for (left, right) in [
        ("ASCII 123", "ascii 123"),
        ("Straße", "STRASSE"),
        ("ΟΣ", "οσ"),
        ("ς", "σ"),
        ("Café", "Cafe\u{301}"),
        ("한", "\u{1112}\u{1161}\u{11AB}"),
        ("\u{10400}", "\u{10428}"),
        ("A\u{1ACF}\u{323}", "A\u{323}\u{1ACF}"),
    ] {
        assert_eq!(
            caseless_key(left),
            caseless_key(right),
            "{left:?} vs {right:?}"
        );
    }
    assert_eq!(caseless_key(""), "");
    assert_eq!(caseless_key("Iİı"), "ii\u{307}ı");
    assert_ne!(caseless_key("café"), caseless_key("cafe"));
    assert_ne!(caseless_key("а"), caseless_key("a")); // Cyrillic vs Latin
    assert_ne!(caseless_key("Ａ"), caseless_key("a")); // No compatibility normalization
    assert_eq!(caseless_key("A\0B"), "a\0b");
}

#[test]
fn long_unsorted_combining_runs_are_stable() {
    let source = format!("X{}", "\u{315}\u{300}".repeat(16_384));
    let expected = format!("x{}{}", "\u{300}".repeat(16_384), "\u{315}".repeat(16_384));
    assert_eq!(caseless_key(&source), expected);
    // Same-class order must be preserved; marks are not sorted by codepoint.
    assert_ne!(
        caseless_key("x\u{300}\u{301}"),
        caseless_key("x\u{301}\u{300}")
    );
}

fn decode(points: &str) -> String {
    if points == "-" {
        return String::new();
    }
    points
        .split_ascii_whitespace()
        .map(|point| {
            char::from_u32(u32::from_str_radix(point, 16).expect("fixture codepoint"))
                .expect("fixture scalar")
        })
        .collect()
}

#[test]
fn matches_pinned_web_differential_vectors() {
    let mut count = 0;
    for line in include_str!("unicode_vectors.tsv").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let (input, expected) = line.split_once('\t').expect("fixture columns");
        let input = decode(input);
        let expected = decode(expected);
        assert_eq!(caseless_key(&input), expected, "input {input:?}");
        assert_eq!(
            caseless_key(&expected),
            expected,
            "idempotence for {input:?}"
        );
        count += 1;
    }
    assert_eq!(
        count, 48_301,
        "the complete pinned differential corpus must be present"
    );
}

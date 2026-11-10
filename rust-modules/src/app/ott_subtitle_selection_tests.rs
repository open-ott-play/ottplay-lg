use super::*;

#[test]
fn missing_native_policy_preserves_manual_behavior_independently_of_other_languages() {
    let mut saved = Saved::default();
    saved.settings.insert("torrentPreferredLanguage".into(), json!("Russian"));
    saved.settings.insert("language".into(), json!("ru"));
    let policy = read(&saved).unwrap();
    assert_eq!(policy.mode, HlsSubtitleMode::Off);
    assert!(policy.language.is_empty());
}

#[test]
fn native_mode_language_and_geometry_share_durable_revision_without_losing_fields() {
    let mut base = Saved::default();
    base.settings.insert("unrelated".into(), json!(true));
    let auto = Field::Mode.candidate(&base, &json!("auto")).unwrap();
    let language = Field::Language.candidate(&auto, &json!("es-419")).unwrap();
    let sized = super::super::Edit::Size(plx_plex::plex::session::SubtitleSize::Small)
        .candidate(&language).unwrap();
    assert_eq!(read(&sized).unwrap(), HlsSubtitlePolicy { mode: HlsSubtitleMode::Auto, language: "es-419".into() });
    assert_eq!(revision(&sized), Ok(3));
    assert_eq!(sized.settings["unrelated"], json!(true));
    assert!(!base.settings.contains_key(MODE));
    let same = Field::Language.candidate(&sized, &json!("es-419")).unwrap();
    assert_eq!(revision(&same), Ok(3));
}

#[test]
fn malformed_selection_policy_is_rejected_before_restore_or_write() {
    for (field, values) in [
        (Field::Mode, vec![json!("default"), json!(1), Value::Null, json!({"mode":"auto"})]),
        (Field::Language, vec![json!("r"), json!("ru--RU"), json!("../en"), json!("en\n"), json!("en-US-123456789"), json!(true)]),
    ] {
        for value in values {
            assert!(!field.valid(&value));
            assert!(field.candidate(&Saved::default(), &value).is_err());
            let mut saved = Saved::default();
            saved.settings.insert(field.id().into(), value);
            assert!(read(&saved).is_err());
            assert!(super::super::Stored::read(&saved).is_err());
        }
    }
    for language in ["", "ru", "eng", "en-US", "zh-Hant", "es-419"] {
        assert!(Field::Language.valid(&json!(language)));
    }
}

#[test]
fn policy_counter_rejects_exhaustion_and_does_not_revive_old_cas_revision() {
    let auto = Field::Mode.candidate(&Saved::default(), &json!("auto")).unwrap();
    let off = Field::Mode.candidate(&auto, &json!("off")).unwrap();
    let again = Field::Mode.candidate(&off, &json!("auto")).unwrap();
    assert_eq!(auto.settings[MODE], again.settings[MODE]);
    assert_ne!(auto.settings[REVISION], again.settings[REVISION]);
    let mut exhausted = again;
    exhausted.settings.insert(REVISION.into(), json!(u64::MAX));
    assert!(Field::Mode.candidate(&exhausted, &json!("forced")).is_err());
    assert!(Field::Mode.candidate(&exhausted, &json!("auto")).is_ok());
}

#[test]
fn local_choices_use_the_same_wire_values_and_preserve_custom_languages() {
    for field in Field::ALL {
        for (index, _) in field.choices() {
            let value = field.choice_value(index).unwrap();
            assert_eq!(field.index(&value), Some(index));
            assert!(field.candidate(&Saved::default(), &value).is_ok());
        }
    }
    assert_eq!(Field::Language.index(&json!("es-419")), Some(u64::MAX));
    assert!(Field::Language.choice_value(u64::MAX).is_err());
}

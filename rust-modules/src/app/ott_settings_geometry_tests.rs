//! Prepared source tests for the existing Saved/typed-enum contract.
use super::*;
use super::super::super::subtitle_settings::{Edit, Stored, REVISION};
use plx_media::player::text_style::{TextAppearance, TextColor};
use plx_media::route::NativeSubtitlePreferences;
use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};

#[test]
fn exact_geometry_aliases_are_appended_and_advertise_only_real_native_rungs() {
    let expected = [
        ("nativeSubtitleSize", json!(["small", "medium", "large", "extra_large", "huge"]), json!("medium")),
        ("nativeSubtitlePosition", json!(["low", "middle", "high"]), json!("low")),
        ("nativeSubtitleTone", json!(["white", "grey_85", "grey_70", "grey_55", "grey_40", "grey_28"]), json!("white")),
    ];
    assert_eq!(Setting::ALL[22].id(), "m3uMediaUrl");
    assert_eq!(Setting::ALL.len(), 41);
    assert_eq!(Setting::ALL[40].id(), "uiColorConfig");
    assert_eq!(Setting::ALL[26..40].iter().map(|setting| setting.id()).collect::<Vec<_>>(),
        vec!["volumeStep", "sleepTimeout", "infoTimeout", "osdOpacity", "permanentTime", "pageSize", "listPosition", "seek13Duration", "arrowFun", "showNumber", "showName", "showProgram", "showDescription", "showProgress"]);
    for (offset, (id, values, default)) in expected.into_iter().enumerate() {
        let setting = Setting::find(id).expect("exact CLI alias must be advertised");
        assert_eq!(Setting::ALL[23 + offset].id(), id);
        assert_eq!(setting.storage_key(), id);
        assert_eq!(revision_key(setting), REVISION);
        let descriptor = setting.descriptor();
        assert_eq!(descriptor["id"], id);
        assert_eq!(descriptor["type"], "string");
        assert_eq!(descriptor["scope"], "application");
        assert_eq!(descriptor["access"], json!({"read":true,"write":true}));
        assert_eq!(descriptor["constraints"]["values"], values);
        assert_eq!(descriptor["default_value"], default);
        for choice in descriptor["choices"].as_array().unwrap() {
            assert!(setting.valid(&choice["value"]));
            assert_eq!(choice["aliases"], json!([choice["value"]]));
        }
        for invalid in [json!(0), json!(true), Value::Null, json!({}), json!("Medium"), json!("unknown"), json!("medium\n")] {
            assert!(!setting.valid(&invalid), "{id}: {invalid}");
        }
        let default = stored(&Saved::default(), setting);
        assert_eq!(default["storage"], "default");
        assert!(default.get("persisted_value").is_none());
    }
}

#[test]
fn geometry_aliases_share_ui_edit_revision_without_new_storage_or_unrelated_changes() {
    let initial = Edit::Appearance(TextAppearance { color: TextColor::Cyan, ..Default::default() })
        .candidate(&Saved::default()).unwrap();
    let mut saved = initial.clone();
    for (field, value) in [
        (geometry::Field::Size, json!("large")),
        (geometry::Field::Position, json!("high")),
        (geometry::Field::Tone, json!("grey_85")),
    ] {
        saved = field.edit(&value).unwrap().candidate(&saved).unwrap();
        let snapshot = stored(&saved, Setting::Geometry(field));
        assert_eq!(snapshot["storage"], "stored");
        assert_eq!(snapshot["persisted_value"], value);
    }
    assert_eq!(saved.settings.len(), initial.settings.len() + 3);
    assert_eq!(saved.settings[REVISION], 4);
    assert_eq!(saved.settings[appearance::STORAGE_KEY], initial.settings[appearance::STORAGE_KEY]);
    let fallback = NativeSubtitlePreferences {
        size: SubtitleSize::Small, position: SubtitlePosition::Low, tone: SubtitleTone::White,
    };
    assert_eq!(Stored::read(&saved).unwrap().effective(fallback), NativeSubtitlePreferences {
        size: SubtitleSize::Large, position: SubtitlePosition::High, tone: SubtitleTone::Silver,
    });
    let changed = Edit::Size(SubtitleSize::Small).candidate(&saved).unwrap();
    let restored = Edit::Size(SubtitleSize::Large).candidate(&changed).unwrap();
    for field in [geometry::Field::Size, geometry::Field::Position, geometry::Field::Tone] {
        assert_ne!(signature(&saved, Setting::Geometry(field)), signature(&restored, Setting::Geometry(field)));
    }
    for field in appearance::Field::ALL {
        assert_ne!(signature(&saved, Setting::Appearance(field)), signature(&restored, Setting::Appearance(field)));
    }
}

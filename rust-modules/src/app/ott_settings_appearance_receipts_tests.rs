use super::super::super::subtitle_settings::Edit;
use super::*;
use plx_media::player::text_style::{TextAppearance, TextColor};

#[test]
fn all_aliases_fingerprint_the_whole_appearance_and_shared_ui_revision() {
    let first = Edit::Appearance(TextAppearance::default())
        .candidate(&Saved::default())
        .unwrap();
    let second = Edit::Appearance(TextAppearance {
        color: TextColor::Yellow,
        ..Default::default()
    })
    .candidate(&first)
    .unwrap();
    let third = Edit::Appearance(TextAppearance::default())
        .candidate(&second)
        .unwrap();
    for field in appearance::Field::ALL {
        let setting = Setting::Appearance(field);
        let sig = signature(&first, setting);
        let mut receipts = Receipts::default();
        let token = receipts.record(
            "read-before-ui-change",
            sig.clone(),
            "context".into(),
            0,
            10,
        );
        assert_ne!(sig, signature(&second, setting));
        assert_ne!(sig, signature(&third, setting));
        assert!(!receipts.consume(&token, &signature(&third, setting), "context", 0, 11));
    }
}

#[test]
fn scalar_aliases_use_existing_compound_storage_and_do_not_create_parallel_keys() {
    let field = appearance::Field::Background;
    let changed = field
        .merge(TextAppearance::default(), &json!("half"))
        .unwrap();
    let saved = Edit::Appearance(changed)
        .candidate(&Saved::default())
        .unwrap();
    let snapshot = stored(&saved, Setting::Appearance(field));
    assert_eq!(snapshot["storage"], "stored");
    assert_eq!(snapshot["persisted_value"], "half");
    assert!(saved.settings.get(field.id()).is_none());
    assert_eq!(
        saved.settings[appearance::STORAGE_KEY]["background"],
        "half"
    );
}

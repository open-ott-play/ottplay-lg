use super::*;
use plx_media::player::text_style::TextColor;

#[test]
fn legacy_revision_starts_at_zero_and_every_real_edit_uses_one_counter() {
    let before = Saved::default();
    assert_eq!(revision(&before), Ok(0));
    let first = Edit::Appearance(TextAppearance::default())
        .candidate(&before)
        .unwrap();
    assert_eq!(revision(&first), Ok(1));
    let second = Edit::Size(SubtitleSize::Small).candidate(&first).unwrap();
    assert_eq!(revision(&second), Ok(2));
    let third = Edit::Position(SubtitlePosition::High)
        .candidate(&second)
        .unwrap();
    assert_eq!(revision(&third), Ok(3));
    assert_eq!(
        revision(&before),
        Ok(0),
        "preparation must not publish a revision"
    );
}

#[test]
fn invalid_or_exhausted_revision_fails_before_candidate_publication() {
    for value in [json!(null), json!("1"), json!(-1), json!(1.5), json!(true)] {
        let mut before = Saved::default();
        before.settings.insert(REVISION.into(), value.clone());
        assert!(Edit::Appearance(TextAppearance::default())
            .candidate(&before)
            .is_err());
        assert!(Stored::read(&before).is_err());
        assert_eq!(before.settings[REVISION], value);
    }
    let mut exhausted = Edit::Appearance(TextAppearance::default())
        .candidate(&Saved::default())
        .unwrap();
    exhausted.settings.insert(REVISION.into(), json!(u64::MAX));
    assert!(Stored::read(&exhausted).is_ok());
    assert!(Edit::Appearance(TextAppearance {
        color: TextColor::Yellow,
        ..Default::default()
    })
    .candidate(&exhausted)
    .is_err());
    let same = Edit::Appearance(TextAppearance::default())
        .candidate(&exhausted)
        .unwrap();
    assert_eq!(
        same.settings, exhausted.settings,
        "equal no-op needs no revision increment"
    );
}

#[test]
fn appearance_a_to_b_to_a_cannot_restore_the_original_revision() {
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
    assert_eq!(first.settings[APPEARANCE], third.settings[APPEARANCE]);
    assert_eq!(revision(&first), Ok(1));
    assert_eq!(revision(&third), Ok(3));
    let no_op = Edit::Appearance(TextAppearance::default())
        .candidate(&third)
        .unwrap();
    assert_eq!(revision(&no_op), Ok(3));
}

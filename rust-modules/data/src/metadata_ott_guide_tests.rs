use super::{OttGuideProgramme, OttNowNext};

fn row(start_s: i64, end_s: i64, title: &str) -> OttGuideProgramme {
    OttGuideProgramme { title: title.into(),
        description: "First sentence. Second sentence.\n\nFINAL_PARAGRAPH".into(), start_s, end_s }
}
fn facts(current: Option<OttGuideProgramme>, next: Option<OttGuideProgramme>) -> OttNowNext {
    OttNowNext { now_s: 150, current, next }
}
fn titles(value: &OttNowNext, now_s: i64) -> (Option<&str>, Option<&str>) {
    let (current, next) = value.at(now_s);
    (current.map(|row| row.title.as_str()), next.map(|row| row.title.as_str()))
}

#[test]
fn guide_at_uses_half_open_boundaries_and_preserves_the_full_description() {
    let value = facts(Some(row(100, 200, "Current")), Some(row(200, 300, "Next")));
    assert_eq!(titles(&value, 100), (Some("Current"), Some("Next")));
    assert_eq!(titles(&value, 199), (Some("Current"), Some("Next")));
    assert_eq!(titles(&value, 200), (Some("Next"), None));
    assert!(std::ptr::eq(value.at(150).0.unwrap(), value.current.as_ref().unwrap()));
    assert_eq!(value.at(150).0.unwrap().description,
        "First sentence. Second sentence.\n\nFINAL_PARAGRAPH");
}

#[test]
fn guide_at_empty_or_negative_clocks_do_not_invent_rows() {
    let empty = facts(None, None);
    assert_eq!(empty.at(150), (None, None));
    let mut value = facts(Some(row(100, 200, "Current")), Some(row(200, 300, "Next")));
    assert_eq!(value.at(-1), (None, None));
    value.now_s = -1;
    assert_eq!(value.at(150), (None, None));
}

#[test]
fn guide_at_any_invalid_interval_invalidates_the_whole_snapshot() {
    for (start, end) in [(-1, 200), (100, 100), (200, 100)] {
        let invalid = row(start, end, "Invalid");
        let value = facts(Some(invalid.clone()), Some(row(300, 400, "Valid future")));
        assert_eq!(value.at(150), (None, None));
        let value = facts(Some(row(100, 200, "Valid current")), Some(invalid));
        assert_eq!(value.at(150), (None, None));
    }
}

#[test]
fn guide_at_crosses_midnight_using_absolute_broadcast_time() {
    let value = facts(Some(row(86_340, 86_460, "Across midnight")), Some(row(86_460, 86_520, "After midnight")));
    assert_eq!(titles(&value, 86_400), (Some("Across midnight"), Some("After midnight")));
    assert_eq!(titles(&value, 86_460), (Some("After midnight"), None));
}

#[test]
fn guide_at_promotes_former_next_then_expires_without_synthesizing_a_successor() {
    let value = facts(Some(row(100, 200, "Old current")), Some(row(250, 300, "Old next")));
    assert_eq!(titles(&value, 200), (None, Some("Old next")));
    assert_eq!(titles(&value, 249), (None, Some("Old next")));
    assert_eq!(titles(&value, 250), (Some("Old next"), None));
    assert_eq!(titles(&value, 300), (None, None));
    assert_eq!(titles(&value, i64::MAX), (None, None));
}

#[test]
fn guide_at_overlaps_use_latest_start_then_end_and_never_repeat_current_as_next() {
    let value = facts(Some(row(100, 300, "Earlier")), Some(row(150, 200, "Later")));
    assert_eq!(titles(&value, 175), (Some("Later"), None));
    assert_eq!(titles(&value, 120), (Some("Earlier"), None));
    let value = facts(Some(row(100, 200, "Short")), Some(row(100, 300, "Long")));
    assert_eq!(titles(&value, 150), (Some("Long"), None));
    let value = facts(Some(row(100, 200, "First equal")), Some(row(100, 200, "Last equal")));
    assert_eq!(titles(&value, 150), (Some("Last equal"), None));
}

#[test]
fn guide_at_rollback_uses_retained_facts_without_mutation_or_extra_history() {
    let value = facts(Some(row(100, 200, "First")), Some(row(250, 300, "Second")));
    let before = value.clone();
    assert_eq!(titles(&value, 260), (Some("Second"), None));
    assert_eq!(titles(&value, 150), (Some("First"), Some("Second")));
    assert_eq!(titles(&value, 99), (None, Some("First")));
    assert_eq!(value, before);
}

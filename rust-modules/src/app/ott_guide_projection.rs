//! Read-only display of an already admitted current/next guide reply.
//! The caller retains source/selection authority; no request or playback starts here.
use plx_data::ott::{
    epg::Programme,
    guide::{Query, ResultSet},
};
use plx_screens::ott::{OttGuideProgramme, OttNowNext};

fn programme(value: &Programme) -> OttGuideProgramme {
    OttGuideProgramme {
        title: value.title.clone(),
        description: value.description.clone(),
        start_s: value.start,
        end_s: value.end,
    }
}

fn select<'a, T: 'a>(
    rows: impl Iterator<Item = &'a T> + Clone,
    now_s: i64,
    interval: impl Fn(&T) -> (i64, i64),
) -> Option<(Option<&'a T>, Option<&'a T>)> {
    if now_s < 0 {
        return None;
    }
    let mut current: Option<&T> = None;
    for row in rows.clone() {
        let (start, end) = interval(row);
        if start < 0 || end <= start {
            return None;
        }
        if start <= now_s && now_s < end && current.is_none_or(|old| (start, end) >= interval(old))
        {
            current = Some(row);
        }
    }
    // The existing guide selector uses the end of the current programme, not
    // merely `now`, so an overlapping programme is not also advertised as next.
    let boundary = current.map_or(now_s, |row| interval(row).1);
    let next = rows
        .filter(|row| interval(row).0 >= boundary)
        .min_by_key(|row| interval(row));
    Some((current, next))
}

/// Select at the supplied UTC sample, including when the reply arrived after
/// its requested time. Only the chosen two descriptions are copied, never the
/// whole guide. Failed requests and source binding are handled by the caller.
pub(super) fn current_next(result: &ResultSet, now_s: i64) -> Option<OttNowNext> {
    if now_s < 0 || !matches!(result.query.validate().ok()?, Query::CurrentNext { .. }) {
        return None;
    }
    let mut channel = None;
    for row in &result.programmes {
        if channel.is_some_and(|channel| channel != row.channel.as_str()) {
            return None;
        }
        channel = Some(row.channel.as_str());
    }
    let (current, next) = select(result.programmes.iter(), now_s, |row| (row.start, row.end))?;
    Some(OttNowNext {
        now_s,
        current: current.map(programme),
        next: next.map(programme),
    })
}

/// Re-project only retained facts. Clock rollback may expose an earlier retained
/// programme again, but cannot recover a programme absent from this snapshot.
pub(super) fn advance(snapshot: &OttNowNext, now_s: i64) -> Option<OttNowNext> {
    // This API distinguishes malformed input (None) from an accepted empty
    // projection (Some with no rows); the shared display helper returns a pair.
    if snapshot.now_s < 0 || now_s < 0
        || [snapshot.current.as_ref(), snapshot.next.as_ref()]
            .into_iter().flatten()
            .any(|row| row.start_s < 0 || row.end_s <= row.start_s)
    {
        return None;
    }
    let (current, next) = snapshot.at(now_s);
    Some(OttNowNext {
        now_s,
        current: current.cloned(),
        next: next.cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::ott::guide::{Context, Coverage};

    fn row(start: i64, end: i64, title: &str) -> Programme {
        Programme {
            channel: "fixture-channel".into(),
            start,
            end,
            title: title.into(),
            description: format!("Full description of {title}.\nFinal paragraph."),
        }
    }

    fn result(programmes: Vec<Programme>) -> ResultSet {
        ResultSet {
            context: Context {
                generation: 9,
                profile_id: "fixture-profile".into(),
                entry_id: "fixture-entry".into(),
            },
            query: Query::CurrentNext { now: 150 },
            coverage: Coverage::RequestedWindow,
            programmes,
        }
    }

    #[test]
    fn full_current_next_facts_are_retained_without_mutating_the_source() {
        let source = result(vec![row(100, 200, "Current"), row(200, 300, "Next")]);
        let before = source.clone();
        let projected = current_next(&source, 175).unwrap();
        assert_eq!(projected.now_s, 175);
        assert_eq!(projected.current, Some(programme(&source.programmes[0])));
        assert_eq!(projected.next, Some(programme(&source.programmes[1])));
        assert_eq!(source, before);
    }

    #[test]
    fn intervals_are_half_open_and_delayed_reply_uses_supplied_now() {
        let source = result(vec![row(100, 200, "First"), row(200, 300, "Second")]);
        assert_eq!(
            current_next(&source, 100).unwrap().current.unwrap().title,
            "First"
        );
        let at_boundary = current_next(&source, 200).unwrap();
        assert_eq!(at_boundary.current.unwrap().title, "Second");
        assert!(at_boundary.next.is_none());
        let expired = current_next(&source, 300).unwrap();
        assert!(expired.current.is_none() && expired.next.is_none());
    }

    #[test]
    fn missing_current_keeps_a_future_next_and_never_promotes_expired_rows() {
        let source = result(vec![row(100, 200, "Expired"), row(250, 300, "Future")]);
        let projected = current_next(&source, 225).unwrap();
        assert!(projected.current.is_none());
        assert_eq!(projected.next.unwrap().title, "Future");
    }

    #[test]
    fn midnight_is_an_epoch_boundary_not_a_reset() {
        let source = result(vec![
            row(86_000, 86_500, "Across midnight"),
            row(86_500, 87_000, "Morning"),
        ]);
        let projected = current_next(&source, 86_400).unwrap();
        let current = projected.current.unwrap();
        assert_eq!((current.start_s, current.end_s), (86_000, 86_500));
        assert_eq!(projected.next.unwrap().start_s, 86_500);
    }

    #[test]
    fn unordered_overlaps_follow_the_existing_guide_selection_semantics() {
        let source = result(vec![
            row(300, 400, "Next"),
            row(100, 300, "Long"),
            row(140, 200, "Current"),
            row(180, 220, "Overlap"),
        ]);
        let projected = current_next(&source, 150).unwrap();
        assert_eq!(projected.current.unwrap().title, "Current");
        assert_eq!(projected.next.unwrap().title, "Next");
        let selected = plx_data::ott::guide::select(
            &source.programmes,
            "fixture-channel",
            Query::CurrentNext { now: 150 },
            0,
        )
        .unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Current", "Next"]
        );
    }

    #[test]
    fn empty_success_is_distinct_from_a_rejected_query() {
        let source = result(Vec::new());
        assert_eq!(
            current_next(&source, 150),
            Some(OttNowNext {
                now_s: 150,
                current: None,
                next: None
            })
        );
        let mut day = source.clone();
        day.query = Query::Day {
            start: 0,
            end: 86_400,
        };
        assert!(current_next(&day, 150).is_none());
        day.query = Query::CurrentNext { now: -1 };
        assert!(current_next(&day, 150).is_none());
        assert!(current_next(&source, -1).is_none());
    }

    #[test]
    fn malformed_intervals_fail_closed_including_unselected_rows() {
        for invalid in [
            row(-1, 10, "Negative"),
            row(300, 300, "Empty"),
            row(400, 300, "Reverse"),
        ] {
            let source = result(vec![row(100, 200, "Current"), invalid]);
            assert!(current_next(&source, 150).is_none());
        }
    }

    #[test]
    fn mixed_channel_reply_cannot_supply_another_channels_next() {
        let mut other = row(200, 300, "Other channel");
        other.channel = "other-channel".into();
        assert!(current_next(&result(vec![row(100, 200, "Current"), other]), 150).is_none());
    }

    #[test]
    fn a_large_reply_projects_only_two_rows_and_preserves_long_text() {
        let mut source = result(
            (0..20_000)
                .map(|i| row(i * 100, (i + 1) * 100, "Programme"))
                .collect(),
        );
        source.programmes[10_000].description = "Complete paragraph.\n".repeat(1000);
        let projected = current_next(&source, 1_000_025).unwrap();
        assert_eq!(
            projected.current.unwrap().description,
            source.programmes[10_000].description
        );
        assert_eq!(projected.next.unwrap().start_s, 1_000_100);
        assert_eq!(source.programmes.len(), 20_000);
    }

    #[test]
    fn extreme_valid_epoch_does_not_overflow() {
        let source = result(vec![row(i64::MAX - 2, i64::MAX, "Last")]);
        let projected = current_next(&source, i64::MAX - 1).unwrap();
        assert_eq!(projected.current.unwrap().end_s, i64::MAX);
        assert!(projected.next.is_none());
    }

    #[test]
    fn retained_next_becomes_current_then_expires_without_inventing_more() {
        let initial = current_next(
            &result(vec![row(100, 200, "First"), row(200, 300, "Second")]),
            150,
        )
        .unwrap();
        let before = initial.clone();
        let advanced = advance(&initial, 200).unwrap();
        assert_eq!(advanced.current.unwrap().title, "Second");
        assert!(advanced.next.is_none());
        let expired = advance(&initial, 300).unwrap();
        assert!(expired.current.is_none() && expired.next.is_none());
        assert_eq!(initial, before);
    }

    #[test]
    fn advancing_gap_and_clock_rollback_use_only_retained_facts() {
        let initial = current_next(
            &result(vec![row(100, 200, "First"), row(250, 300, "Second")]),
            150,
        )
        .unwrap();
        let gap = advance(&initial, 225).unwrap();
        assert!(gap.current.is_none());
        assert_eq!(gap.next.as_ref().unwrap().title, "Second");
        let rollback = advance(&initial, 50).unwrap();
        assert!(rollback.current.is_none());
        assert_eq!(rollback.next.unwrap().title, "First");
        // A caller that retained only the re-projected gap cannot recover First.
        let lost = advance(&gap, 150).unwrap();
        assert!(lost.current.is_none());
        assert_eq!(lost.next.unwrap().title, "Second");
    }

    #[test]
    fn advancing_invalid_facts_fails_closed_but_empty_success_is_preserved() {
        let empty = OttNowNext {
            now_s: 150,
            current: None,
            next: None,
        };
        assert_eq!(
            advance(&empty, 200),
            Some(OttNowNext {
                now_s: 200,
                current: None,
                next: None
            })
        );
        assert!(advance(&empty, -1).is_none());
        let mut invalid = empty;
        invalid.current = Some(programme(&row(200, 200, "Invalid")));
        assert!(advance(&invalid, 200).is_none());
        invalid.current = None;
        invalid.now_s = -1;
        assert!(advance(&invalid, 200).is_none());
    }
}

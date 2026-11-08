use super::*;
use crate::ott::epg;
struct Document {
    channels: Vec<Channel>,
    programmes: Vec<Programme>,
}
fn document() -> Document {
    let xml = r#"<tv><channel id="one"><display-name>World News</display-name></channel><channel id="two"><display-name>Other</display-name></channel><programme channel="one" start="20260101000000 +0100" stop="20260101010000 +0100"><title>Past</title></programme><programme channel="one" start="20260101010000 +0100" stop="20260101020000 +0100"><title>Now</title></programme><programme channel="one" start="20260101020000 +0100" stop="20260101030000 +0100"><title>Next</title></programme></tv>"#;
    Document {
        channels: epg::discover_channels(xml).unwrap(),
        programmes: epg::parse_xmltv(xml, "one").unwrap(),
    }
}
#[test]
fn id_wins_and_unambiguous_name_only_matching_works() {
    let mut d = document();
    assert_eq!(
        channel_id(&d.channels, Some("two"), "World News", ""),
        Some("two".into())
    );
    assert_eq!(
        channel_id(&d.channels, None, " world  NEWS ", ""),
        Some("one".into())
    );
    assert_eq!(
        channel_id(&d.channels, Some("missing"), "", "World News"),
        Some("one".into())
    );
    d.channels.push(Channel {
        id: "duplicate".into(),
        names: vec!["World News".into()],
    });
    assert_eq!(channel_id(&d.channels, None, "World News", ""), None);
}
#[test]
fn current_next_and_timezone_boundaries() {
    let d = document();
    let now = epg::timestamp("20260101003000 +0000").unwrap();
    let rows = select(&d.programmes, "one", Query::CurrentNext { now }, 0).unwrap();
    assert_eq!(
        rows.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["Now", "Next"]
    );
    let shifted = select(&d.programmes, "one", Query::CurrentNext { now }, 3600).unwrap();
    assert_eq!(shifted[0].title, "Past");
}
#[test]
fn profile_change_and_cancel_block_delayed_guide() {
    let context = Context {
        generation: 7,
        profile_id: "a".into(),
        entry_id: "one".into(),
    };
    let query = Query::CurrentNext { now: 1800 };
    let result = ResultSet {
        context: context.clone(),
        query,
        coverage: Coverage::RequestedWindow,
        programmes: document().programmes,
    };
    let mut current = Request {
        context: context.clone(),
        query,
    };
    current.context.profile_id = "b".into();
    assert!(!result.can_publish(&current, false));
    let other_query = Request {
        context: context.clone(),
        query: Query::CurrentNext { now: 3600 },
    };
    assert!(!result.can_publish(&other_query, false));
    assert!(!result.can_publish(
        &Request {
            context: context.clone(),
            query
        },
        true
    ));
    assert!(result.can_publish(
        &Request {
            context: context.clone(),
            query
        },
        false
    ));
}

#[test]
fn missing_current_never_promotes_expired_programmes() {
    let d = document();
    let now = d.programmes.last().unwrap().end;
    assert!(select(&d.programmes, "one", Query::CurrentNext { now }, 0)
        .unwrap()
        .is_empty());
    assert!(select(&d.programmes, "one", Query::CurrentNext { now: -1 }, 0).is_err());
}
#[test]
fn overlapping_entries_choose_latest_current_without_repeating_it_as_next() {
    let rows = vec![
        Programme {
            channel: "one".into(),
            start: 0,
            end: 200,
            title: "Old".into(),
            description: String::new(),
        },
        Programme {
            channel: "one".into(),
            start: 50,
            end: 150,
            title: "Current".into(),
            description: String::new(),
        },
        Programme {
            channel: "one".into(),
            start: 150,
            end: 250,
            title: "Next".into(),
            description: String::new(),
        },
    ];
    let selected = select(&rows, "one", Query::CurrentNext { now: 100 }, 0).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["Current", "Next"]
    );
}

#[test]
fn day_selects_overlaps_but_excludes_programmes_at_the_exclusive_end() {
    let d = document();
    let start = d.programmes[1].start + 1;
    let end = d.programmes[2].start;
    let selected = select(&d.programmes, "one", Query::Day { start, end }, 0).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["Now"]
    );
    let shifted = select(&d.programmes, "one", Query::Day { start, end }, 3600).unwrap();
    assert_eq!(
        shifted.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["Past"]
    );
}

#[test]
fn explicit_day_bounds_allow_dst_and_reject_invalid_or_unbounded_windows() {
    for hours in [23, 24, 25] {
        assert!(Query::Day {
            start: 100,
            end: 100 + hours * 3600
        }
        .validate()
        .is_ok());
    }
    for query in [
        Query::Day { start: -1, end: 1 },
        Query::Day { start: 1, end: 1 },
        Query::Day { start: 2, end: 1 },
        Query::Day {
            start: 0,
            end: 172801,
        },
    ] {
        assert!(query.validate().is_err());
    }
}

#[test]
fn delayed_day_cannot_publish_into_another_day_or_current_next() {
    let request = Request {
        context: Context {
            generation: 7,
            profile_id: "p".into(),
            entry_id: "e".into(),
        },
        query: Query::Day {
            start: 100,
            end: 200,
        },
    };
    let result = ResultSet {
        context: request.context.clone(),
        query: request.query,
        coverage: Coverage::RequestedWindow,
        programmes: Vec::new(),
    };
    assert!(result.can_publish(&request, false));
    for query in [
        Query::Day {
            start: 200,
            end: 300,
        },
        Query::CurrentNext { now: 150 },
    ] {
        assert!(!result.can_publish(
            &Request {
                context: request.context.clone(),
                query
            },
            false
        ));
    }
}

#[test]
fn old_channel_metadata_decodes_without_a_catchup_source() {
    let metadata: ChannelMetadata =
        serde_json::from_str(r#"{"tvg_name":"News","guide_shift_seconds":3600}"#).unwrap();
    assert!(metadata.catchup.is_none());
    assert_eq!(metadata.guide_shift_seconds, 3600);
}

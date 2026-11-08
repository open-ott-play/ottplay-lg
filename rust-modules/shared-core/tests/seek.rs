use plx_shared_core::{Error, Planner, SeekPlan, SeekRequest, SeekTarget};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    target: Option<SeekTarget>,
    request: SeekRequest,
    expected: Option<SeekPlan>,
    #[serde(default)]
    error: bool,
}

#[test]
fn canonical_seek_fixtures_match_in_quickjs() {
    let planner = Planner::new().expect("canonical core initializes in the isolated VM");
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/seek.json")).unwrap();
    assert!(
        cases.len() >= 25,
        "the public semantic fixture set must not disappear"
    );
    for case in cases {
        let actual = planner.seek_plan(case.target.as_ref(), &case.request);
        if case.error {
            assert_eq!(actual, Err(Error::InvalidInput), "{}", case.name);
        } else {
            assert_eq!(actual, Ok(case.expected.unwrap()), "{}", case.name);
        }
    }
}

#[test]
fn non_json_numbers_are_rejected_without_becoming_null() {
    let planner = Planner::new().unwrap();
    let mut request = SeekRequest::finite(plx_shared_core::SeekIntent::Offset, 10.0, 30.0, 60.0);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        request.value = Some(value);
        assert_eq!(planner.seek_plan(None, &request), Err(Error::InvalidInput));
    }
}

#[test]
fn invalid_target_does_not_poison_the_next_valid_request() {
    let planner = Planner::new().unwrap();
    let mut target = SeekTarget {
        source_id: "fixture".into(),
        channel_id: "".into(),
        kind: plx_shared_core::PlaybackKind::Vod,
        archive_start: None,
    };
    let request = SeekRequest::finite(plx_shared_core::SeekIntent::Offset, 10.0, 30.0, 60.0);
    assert_eq!(
        planner.seek_plan(Some(&target), &request),
        Err(Error::InvalidInput)
    );
    target.channel_id = "movie".into();
    assert_eq!(
        planner.seek_plan(Some(&target), &request),
        Ok(SeekPlan::Seek { position: 40.0 })
    );
    assert_eq!(
        plx_shared_core::seek_plan(Some(&target), &request),
        Ok(SeekPlan::Seek { position: 40.0 })
    );
}

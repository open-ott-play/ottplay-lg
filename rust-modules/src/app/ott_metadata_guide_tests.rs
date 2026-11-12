//! Pure admission regressions. No worker, provider, clock, storage or UI is run.
use super::*;
use plx_data::ott::{
    epg::Programme,
    guide::{ChannelMetadata, Context, Coverage},
    MediaLocator,
};
use serde_json::json;

const GENERATION: u64 = 7;
const NOW: i64 = 1_000;

fn target() -> (String, String) {
    ("profile-a".into(), "entry-a".into())
}
fn query() -> Query {
    Query::CurrentNext { now: NOW }
}
fn programme() -> Programme {
    Programme {
        channel: "xmltv-channel-a".into(),
        start: 900,
        end: 1_100,
        title: "Current programme".into(),
        description: "Complete programme description".into(),
    }
}
fn fixture(outer_selection: &str, resolved_id: &str) -> Guide {
    let context = Context {
        generation: GENERATION,
        profile_id: "profile-a".into(),
        entry_id: resolved_id.into(),
    };
    Guide { row_guide: None,
        source_profile: ProviderProfile {
            id: "profile-a".into(), name: "Guide fixture".into(),
            kind: plx_data::ott::ProviderKind::M3u,
            source: "https://media.example.invalid/list.m3u".into(),
            source_alias: String::new(), enabled: true,
        },
        generation: GENERATION,
        profile_id: "profile-a".into(),
        entry_id: outer_selection.into(),
        entry: CatalogEntry { protection: None,
            synopsis: None,
            id: resolved_id.into(),
            title: "Channel A".into(),
            category: "News".into(),
            description: String::new(),
            poster: String::new(),
            media: MediaLocator::Http {
                url: "https://media.example.invalid/a.m3u8".into(),
            },
            duration_ms: None,
            provider_id: Some("xmltv-channel-a".into()),
            provider_id_field: Some("tvg-id".into()),
            live: Some(ChannelMetadata { guide_id: None, guide_feed: String::new(),
                tvg_name: "Channel A".into(),
                guide_shift_seconds: 0,
        catchup: None,
            }),
        },
        request: Request {
            context: context.clone(),
            query: query(),
        },
        result: Ok(ResultSet {
            context,
            query: query(),
            coverage: Coverage::RequestedWindow,
            programmes: vec![programme()],
        }),
    }
}
fn ordinary() -> Guide {
    fixture("entry-a", "entry-a")
}

#[test]
fn exact_request_projects_programmes_and_preserves_accepted_identity() {
    let event = ordinary();
    let expected_entry = event.entry.clone();
    let preview = event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .expect("the exact current guide reply is admissible");
    assert_eq!(preview.generation, GENERATION);
    assert_eq!(preview.profile_id, "profile-a");
    assert_eq!(preview.entry_id, "entry-a");
    assert_eq!(preview.entry, Some(expected_entry));
    assert_eq!(
        preview.result.unwrap(),
        json!({"programmes": [programme()]})
    );
}

#[test]
fn favorite_selection_can_differ_from_the_resolved_channel_identity() {
    let event = fixture("favorite:stable-a", "provider-channel-a");
    let expected_entry = event.entry.clone();
    let target = ("profile-a".into(), "favorite:stable-a".into());
    let preview = event
        .into_preview(GENERATION, Some(&target), Some(&query()), false)
        .expect("outer selection and resolved guide identity are separate fences");
    assert_eq!(preview.entry_id, "favorite:stable-a");
    assert_eq!(preview.entry, Some(expected_entry));
    assert_eq!(
        preview.result.unwrap(),
        json!({"programmes": [programme()]})
    );
}

#[test]
fn a_different_now_query_rejects_an_otherwise_matching_reply() {
    assert!(ordinary()
        .into_preview(
            GENERATION,
            Some(&target()),
            Some(&Query::CurrentNext { now: NOW + 1 }),
            false,
        )
        .is_none());
}

#[test]
fn absent_current_target_or_query_cannot_publish() {
    assert!(ordinary()
        .into_preview(GENERATION, None, Some(&query()), false)
        .is_none());
    assert!(ordinary()
        .into_preview(GENERATION, Some(&target()), None, false)
        .is_none());
}

#[test]
fn cancelled_success_and_cancelled_error_cannot_publish() {
    assert!(ordinary()
        .into_preview(GENERATION, Some(&target()), Some(&query()), true)
        .is_none());
    let mut error = ordinary();
    error.result = Err("Guide unavailable");
    assert!(error
        .into_preview(GENERATION, Some(&target()), Some(&query()), true)
        .is_none());
}

#[test]
fn old_event_generation_is_rejected() {
    assert!(ordinary()
        .into_preview(GENERATION + 1, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn current_target_profile_and_outer_selection_both_must_match() {
    for target in [
        ("profile-b".into(), "entry-a".into()),
        ("profile-a".into(), "entry-b".into()),
    ] {
        assert!(ordinary()
            .into_preview(GENERATION, Some(&target), Some(&query()), false)
            .is_none());
    }
}

#[test]
fn matching_outer_profile_cannot_hide_the_wrong_request_profile() {
    let mut event = ordinary();
    event.profile_id = "profile-b".into();
    let target = ("profile-b".into(), "entry-a".into());
    assert!(event
        .into_preview(GENERATION, Some(&target), Some(&query()), false)
        .is_none());
}

#[test]
fn self_consistent_request_and_result_cannot_override_event_generation() {
    let mut event = ordinary();
    event.request.context.generation = GENERATION - 1;
    event.result.as_mut().unwrap().context = event.request.context.clone();
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn self_consistent_request_and_result_cannot_override_event_profile() {
    let mut event = ordinary();
    event.request.context.profile_id = "profile-b".into();
    event.result.as_mut().unwrap().context = event.request.context.clone();
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn self_consistent_request_and_result_must_still_match_resolved_entry() {
    let mut event = ordinary();
    event.request.context.entry_id = "entry-b".into();
    event.result.as_mut().unwrap().context = event.request.context.clone();
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn resolved_entry_cannot_be_replaced_after_request() {
    let mut event = ordinary();
    event.entry.id = "replacement-entry".into();
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn matching_favorite_outer_target_does_not_bypass_resolved_identity() {
    let mut event = fixture("favorite:stable-a", "provider-channel-a");
    event.entry.id = "provider-channel-b".into();
    let target = ("profile-a".into(), "favorite:stable-a".into());
    assert!(event
        .into_preview(GENERATION, Some(&target), Some(&query()), false)
        .is_none());
}

#[test]
fn result_context_generation_profile_and_entry_are_checked_independently() {
    for field in 0..3 {
        let mut event = ordinary();
        let result = event.result.as_mut().unwrap();
        match field {
            0 => result.context.generation += 1,
            1 => result.context.profile_id = "profile-b".into(),
            _ => result.context.entry_id = "entry-b".into(),
        }
        assert!(event
            .into_preview(GENERATION, Some(&target()), Some(&query()), false)
            .is_none());
    }
}

#[test]
fn result_query_must_match_the_original_request_not_just_channel() {
    let mut event = ordinary();
    event.result.as_mut().unwrap().query = Query::CurrentNext { now: NOW - 1 };
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn self_consistent_old_request_and_result_cannot_publish_to_new_query() {
    let mut event = ordinary();
    event.request.query = Query::CurrentNext { now: NOW - 1 };
    event.result.as_mut().unwrap().query = event.request.query;
    assert!(event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .is_none());
}

#[test]
fn an_admitted_error_stays_an_error_not_a_successful_empty_guide() {
    let mut event = ordinary();
    event.result = Err("Provider rejected guide request");
    let preview = event
        .into_preview(GENERATION, Some(&target()), Some(&query()), false)
        .expect("a current failure belongs to its current preview");
    assert_eq!(preview.result, Err("Provider rejected guide request"));
    assert_eq!(preview.entry_id, "entry-a");
    assert_eq!(preview.entry.unwrap().id, "entry-a");
}

#[test]
fn stale_error_cannot_replace_a_new_preview() {
    let mut event = ordinary();
    event.result = Err("Provider rejected guide request");
    assert!(event
        .into_preview(
            GENERATION,
            Some(&target()),
            Some(&Query::CurrentNext { now: NOW + 1 }),
            false,
        )
        .is_none());
}

#[test]
fn error_path_must_also_check_request_context() {
    for field in 0..3 {
        let mut event = ordinary();
        event.result = Err("Provider rejected guide request");
        match field {
            0 => event.request.context.generation += 1,
            1 => event.request.context.profile_id = "profile-b".into(),
            _ => event.request.context.entry_id = "entry-b".into(),
        }
        assert!(event
            .into_preview(GENERATION, Some(&target()), Some(&query()), false)
            .is_none());
    }
}

#[test]
fn successful_empty_and_provider_short_results_do_not_invent_programmes() {
    for coverage in [Coverage::RequestedWindow, Coverage::ProviderShortGuide] {
        let mut event = ordinary();
        let result = event.result.as_mut().unwrap();
        result.coverage = coverage;
        result.programmes.clear();
        let preview = event
            .into_preview(GENERATION, Some(&target()), Some(&query()), false)
            .expect("coverage is not an identity mismatch");
        assert_eq!(preview.result.unwrap(), json!({"programmes": []}));
    }
}

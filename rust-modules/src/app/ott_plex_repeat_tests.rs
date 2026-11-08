//! Proposed additive Repeat wire and existing queue-state controls. Unrun.
use super::*;

fn accepted() -> PlexQueue {
    let ids = ["41", "42", "41"].map(|id| DecimalId::parse(id).unwrap());
    let rows = ["41", "42"].map(|id| PlexRecord {
        id: id.into(), title: format!("Item {id}"), kind: RecordKind::Episode, playable: true,
    });
    let mut queue = PlexQueue::new(OwnerToken(8), prepare(&ids, &rows).unwrap());
    queue.begin(OwnerToken(8), PlaybackToken(12), 2).unwrap();
    assert!(queue.observe(OwnerToken(8), PlaybackToken(12), PlaybackPhase::Paused));
    queue
}

#[test]
fn ordinary_repeat_wire_is_additive_and_closed_for_both_state_versions() {
    let old_capability = queue_capability();
    assert_eq!(old_capability["operations"], json!(["play", "preview", "status", "next", "previous", "stop"]));
    for version in [1, 2] {
        for mode in ["none", "all"] {
            let mut params = json!({"runtime":"page-123","op":"repeat","repeat":mode});
            if version == 2 { params["state_version"] = json!(2); }
            assert!(parse("plex_queue", &params, "page-123").is_ok(), "{params}");
            assert!(parse("plex_library", &params, "page-123").is_err());
            assert!(parse("plex_queue", &params, "different-runtime").is_err());
            for value in [json!("one"), json!("ALL"), json!(true), json!(1), Value::Null] {
                let mut bad = params.clone();
                bad["repeat"] = value;
                assert!(parse("plex_queue", &bad, "page-123").is_err(), "{bad}");
            }
            let mut missing = params.clone();
            missing.as_object_mut().unwrap().remove("repeat");
            assert!(parse("plex_queue", &missing, "page-123").is_err());
            let mut extra = params;
            extra["context"] = json!("not-part-of-this-protocol");
            assert!(parse("plex_queue", &extra, "page-123").is_err());
        }
    }
    assert_eq!(queue_capability(), old_capability, "legacy operations stay frozen");
    assert_eq!(queue_state_v2_capability()["operations"], json!(["play", "status", "next", "previous", "stop"]));
}

#[test]
fn prospective_repeat_snapshot_checks_the_new_encoded_size_without_mutation() {
    for version in [StateVersion::V1, StateVersion::V2] {
        let mut queue = accepted();
        let owner = queue.owner();
        queue.set_repeat(owner, RepeatPolicy::All).unwrap();
        let index = queue.index();
        // Internal serializer boundary fixture, intentionally beyond the normal
        // 512-byte prepared title. This does not claim a valid public request can
        // reach the byte cap; it proves the preflight checks the proposed value.
        queue.prepared.records[index].title.clear();
        let length = serde_json::to_vec(&queue.snapshot_version("page-123", version).unwrap()).unwrap().len();
        queue.prepared.records[index].title = "x".repeat(version.byte_limit() - length);
        let before = queue.snapshot_version("page-123", version).unwrap();
        assert_eq!(serde_json::to_vec(&before).unwrap().len(), version.byte_limit());
        assert!(queue.snapshot_for_repeat("page-123", version, RepeatPolicy::All).is_ok());
        assert!(queue.snapshot_for_repeat("page-123", version, RepeatPolicy::None).is_err());
        assert_eq!(queue.snapshot_version("page-123", version).unwrap(), before);
    }
}

#[test]
fn ordinary_repeat_setter_preserves_paused_occurrence_and_rejects_foreign_owner() {
    let mut queue = accepted();
    let before = queue.snapshot("page-123").unwrap();
    let owner = queue.owner();
    let playback = queue.playback();
    for mode in [RepeatPolicy::All, RepeatPolicy::None, RepeatPolicy::None] {
        queue.set_repeat(owner, mode).unwrap();
        let mut expected = before.clone();
        expected["repeat"] = json!(if mode == RepeatPolicy::All { "all" } else { "none" });
        assert_eq!(queue.snapshot("page-123").unwrap(), expected);
        assert!(queue.owner() == owner);
        assert!(queue.playback() == playback);
        let preserved = queue.snapshot("page-123").unwrap();
        assert_eq!(queue.set_repeat(OwnerToken(9), RepeatPolicy::All), Err(QueueError::ContextChanged));
        assert_eq!(queue.snapshot("page-123").unwrap(), preserved);
    }
}

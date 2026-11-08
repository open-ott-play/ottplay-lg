use super::*;
use crate::ff::inspect::{InspectedTrack, TrackInventory, TrackKind};

fn row(index: i32, language: &str) -> InspectedTrack {
    InspectedTrack {
        kind: TrackKind::Audio, stream_index: index, ordinal: index - 1,
        codec: "aac".into(), language: language.into(), title: String::new(),
        channels: 2, sample_rate: 48_000, bit_depth: 0, decoder_available: true,
        disposition_known: false, default: false, forced: false,
    }
}
fn table(rows: Vec<InspectedTrack>) -> TrackInventory {
    TrackInventory::complete_for_test(rows).unwrap()
}
#[test]
fn audio_descriptor_survives_segment_stream_reordering_not_language_replacement() {
    let first = table(vec![row(1, "eng"), row(2, "spa")]);
    let pick = Selection::new(&first, 1).unwrap();
    assert_eq!(pick.select(&first), Ok(2));
    let reordered = table(vec![row(1, "spa"), row(2, "eng")]);
    assert_eq!(pick.select(&reordered), Ok(1));
    assert!(pick.select(&table(vec![row(1, "eng"), row(2, "fra")])).is_err());
    assert!(pick.select(&table(vec![row(1, "eng")])).is_err());
}
#[test]
fn audio_pick_rejects_ambiguous_missing_or_non_aac_identity() {
    assert!(Selection::new(&table(vec![row(1, "eng"), row(2, "eng")]), 1).is_err());
    assert!(Selection::new(&table(vec![row(1, "eng")]), 1).is_err());
    let mut ac3 = row(1, "eng"); ac3.codec = "ac3".into();
    assert!(Selection::new(&table(vec![ac3]), 0).is_err());
}
#[test]
fn audio_pick_is_bound_to_snapshot_and_refuses_shape_and_topology_changes() {
    let first = table(vec![row(1, "eng"), row(2, "spa")]);
    let pick = Selection::new(&first, 1).unwrap();
    let mut changed = first.clone(); changed.audio[1].sample_rate = 44_100;
    assert!(pick.select(&changed).is_err());
    let mut changed = first.clone(); changed.audio[1].channels = 6;
    assert!(pick.select(&changed).is_err());
    let mut changed = first.clone(); changed.audio[0].language = "deu".into();
    assert!(pick.select(&changed).is_err());
    assert!(pick.select(&table(vec![row(1, "eng"), row(2, "spa"), row(3, "fra")])).is_err());
    let mut partial = first.clone(); partial.mark_incomplete();
    assert!(pick.select(&partial).is_err(), "per-segment collection must be complete");
    // One segment's snapshot need not claim the entire live presentation is complete.
    let partial_pick = Selection::new(&partial, 1).unwrap();
    assert_eq!(partial_pick.select(&first), Ok(2));
    assert!(!partial.is_complete());
}

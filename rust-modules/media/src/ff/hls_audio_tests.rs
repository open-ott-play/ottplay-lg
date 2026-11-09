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
fn audio_pick_rejects_ambiguous_missing_or_unsupported_codec_identity() {
    assert!(Selection::new(&table(vec![row(1, "eng"), row(2, "eng")]), 1).is_err());
    assert!(Selection::new(&table(vec![row(1, "eng")]), 1).is_err());
    let mut other = row(1, "eng"); other.codec = "eac3".into();
    assert!(Selection::new(&table(vec![other]), 0).is_err());
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

fn pid_bound(inventory: TrackInventory, bindings: &[(i32,u16)]) -> TrackInventory {
    let bindings:Vec<_>=bindings.iter().map(|&(index,pid)| (index,crate::ff::hls_observed::TransportIdentity {
        transport_stream_id:1,program_id:10,pid,stream_type:0x0f,
    })).collect();
    inventory.with_transport_for_test(&bindings).unwrap()
}
#[test]
fn legacy_inspection_choice_and_observed_pid_choice_require_distinct_inventory_proofs() {
    let complete=table(vec![row(1,"eng"),row(2,"spa")]);
    let legacy=Selection::new(&complete,1).unwrap();
    let bound=pid_bound(complete.clone(),&[(1,0x101),(2,0x102)]);
    assert!(!bound.is_complete(),"transport observation must not claim presentation completeness");
    assert!(!legacy.requires_transport_identity());
    assert_eq!(legacy.select(&complete),Ok(2),"initial inspection still uses complete per-container evidence");
    assert_eq!(legacy.select(&bound),Err("HLS segment audio inventory is incomplete"),"feeding bound partial data to the legacy branch is the canary regression");
    let observed=Selection::new(&bound,1).unwrap();
    assert!(observed.requires_transport_identity());
    assert_eq!(observed.select(&bound),Ok(2));
    assert!(observed.select(&complete).is_err(),"observed PID choices never fall back to matching labels");
}
#[test]
fn observed_same_label_audio_remains_pid_bound_after_stream_and_ordinal_reorder() {
    let first=pid_bound(table(vec![row(1,"eng"),row(2,"eng")]),&[(1,0x101),(2,0x102)]);
    let selection=Selection::new(&first,1).unwrap();
    let reordered=pid_bound(table(vec![row(1,"eng"),row(2,"eng")]),&[(1,0x102),(2,0x101)]);
    assert_eq!(selection.select(&reordered),Ok(1));
    let replacement=pid_bound(table(vec![row(1,"eng"),row(2,"eng")]),&[(1,0x103),(2,0x101)]);
    assert!(selection.select(&replacement).is_err(),"identical labels and codec never replace the selected PID");
}

#[test]
fn native_ac3_selection_requires_known_shape_and_freezes_segment_identity() {
    let mut ac3 = row(1,"eng"); ac3.codec="ac3".into(); ac3.channels=6;
    ac3.decoder_available=false; // Native compressed packets need no software decoder.
    let original=table(vec![ac3.clone()]);
    let choice=Selection::new(&original,0).unwrap();
    assert_eq!(choice.select(&original),Ok(1));
    for (channels,rate) in [(2,48_000),(6,44_100)] {
        let mut changed=ac3.clone(); changed.channels=channels; changed.sample_rate=rate;
        assert!(choice.select(&table(vec![changed])).is_err());
    }
    for (channels,rate) in [(0,48_000),(7,48_000),(2,0),(2,24_000)] {
        let mut invalid=ac3.clone(); invalid.channels=channels; invalid.sample_rate=rate;
        assert!(Selection::new(&table(vec![invalid]),0).is_err());
    }
    for rate in [32_000,44_100,48_000] {
        let mut supported=ac3.clone(); supported.sample_rate=rate;
        assert!(Selection::new(&table(vec![supported]),0).is_ok());
    }
}

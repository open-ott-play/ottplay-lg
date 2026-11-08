//! Pure projection into a freshly validated catalog for the same logical item.
//! No old menu token, Local asset authority or native action is reused here.
use crate::ff::inspect::{InspectedTrack, TrackKind};
use crate::route::ExternalTracks;

pub(super) enum SubtitleContinuation {
    Off,
    Retained,
    Missing,
}

fn same_track(a: &InspectedTrack, b: &InspectedTrack) -> bool {
    a.kind == b.kind
        && a.codec == b.codec
        && a.language == b.language
        && a.title == b.title
        && a.channels == b.channels
        && a.sample_rate == b.sample_rate
        && a.bit_depth == b.bit_depth
        && a.disposition_known == b.disposition_known
        && (!a.disposition_known || (a.default == b.default && a.forced == b.forced))
}

fn remap(previous: &[InspectedTrack], current: &[InspectedTrack], selected: i32) -> Option<i32> {
    let old = previous.get(usize::try_from(selected).ok()?)?;
    // Equal validated layouts retain even indistinguishable duplicate tracks.
    // Once the layout changes, an ordinal or container stream index alone does
    // not identify the same audio/subtitle in the newly resolved resource.
    if previous == current {
        return Some(old.ordinal);
    }
    // A unique survivor does not prove which of two old identical rows survived.
    if previous
        .iter()
        .filter(|track| same_track(old, track))
        .count()
        != 1
    {
        return None;
    }
    let mut matches = current.iter().filter(|track| same_track(old, track));
    let found = matches.next()?;
    matches.next().is_none().then_some(found.ordinal)
}

pub(super) fn restore(
    previous: &ExternalTracks,
    current: &mut ExternalTracks,
) -> SubtitleContinuation {
    if let Some(ordinal) = previous.selected_audio().and_then(|selected| {
        remap(
            &previous.inventory().audio,
            &current.inventory().audio,
            selected,
        )
    }) {
        // New availability is authoritative. Unsupported or missing audio keeps
        // the new source's ordinary default; it cannot borrow old decoder support.
        if let Ok(choice) = current.choice(TrackKind::Audio, Some(ordinal)) {
            let _ = current.commit(&choice);
        }
    }
    // Sidecars need the app's current root/volume proof and independent rearming;
    // an external continuation cannot manufacture that authority from a choice.
    if previous.selected_local_subtitle().is_some() {
        return SubtitleContinuation::Missing;
    }
    let Some(selected) = previous.selected_subtitle() else {
        return SubtitleContinuation::Off;
    };
    let Some(ordinal) = remap(
        &previous.inventory().subtitles,
        &current.inventory().subtitles,
        selected,
    ) else {
        return SubtitleContinuation::Missing;
    };
    let Ok(choice) = current.choice(TrackKind::Subtitle, Some(ordinal)) else {
        return SubtitleContinuation::Missing;
    };
    if current.commit(&choice).is_ok() {
        SubtitleContinuation::Retained
    } else {
        SubtitleContinuation::Missing
    }
}

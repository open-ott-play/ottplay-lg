//! A frozen audio choice for one independent MPEG-TS HLS playback.
//! The segment worker receives this value with its source/queue, never a global
//! ordinal. Ordinals and AVStream indexes may change between segments; metadata
//! and codec shape must still identify exactly one track. No IO lives here.
use super::inspect::{InspectedTrack, TrackInventory, TrackKind};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Descriptor {
    codec: String,
    language: String,
    title: String,
    channels: i64,
    sample_rate: i64,
    bit_depth: i64,
}
impl Descriptor {
    fn of(track: &InspectedTrack) -> Self {
        Self {
            codec: track.codec.clone(), language: track.language.clone(),
            title: track.title.clone(), channels: track.channels,
            sample_rate: track.sample_rate, bit_depth: track.bit_depth,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Selection {
    expected: Vec<Descriptor>,
    selected: Descriptor,
    observed: Option<super::hls_observed::Selection>,
}
impl Selection {
    /// Freeze a known segment snapshot, without declaring the full presentation
    /// complete. Ambiguous same-label/same-format tracks cannot be safely picked.
    pub fn new(inventory: &TrackInventory, ordinal: i32) -> Result<Self, &'static str> {
        let mut indexes = std::collections::HashSet::new();
        if inventory.audio.iter().enumerate().any(|(i, t)| {
            t.kind != TrackKind::Audio || t.ordinal != i as i32
                || t.stream_index < 0 || !indexes.insert(t.stream_index)
        }) {
            return Err("Invalid HLS audio snapshot");
        }
        let track = usize::try_from(ordinal).ok().and_then(|i| inventory.audio.get(i))
            .ok_or("HLS audio track is missing")?;
        let aac = track.codec == "aac" && track.channels > 0 && track.sample_rate > 0;
        let mp2 = super::hls_mp2::format(&track.codec, track.channels, track.sample_rate)
            && track.decoder_available;
        let ac3 = super::hls_ac3::format(&track.codec, track.channels, track.sample_rate);
        if !aac && !mp2 && !ac3 {
            return Err("HLS audio selection requires known AAC/AC3 or decodable MP2 format");
        }
        let observed = inventory.transport_identity(track.stream_index).map(|_|
            super::hls_observed::Selection::new(inventory,TrackKind::Audio,ordinal)).transpose()?;
        let selected = Descriptor::of(track);
        let mut expected: Vec<_> = inventory.audio.iter().map(Descriptor::of).collect();
        if observed.is_none() && expected.iter().filter(|d| **d == selected).count() != 1 {
            return Err("HLS audio identity is ambiguous");
        }
        expected.sort();
        Ok(Self { expected, selected, observed })
    }

    /// Initial inspection descriptors require a complete container collection.
    /// Observed choices require the separately proved PID/program inventory;
    /// a missing program table must never downgrade them to descriptor matching.
    pub(super) fn requires_transport_identity(&self) -> bool { self.observed.is_some() }

    /// Verify every segment before emitting any of its buffered AUs. A new or
    /// missing track, changed metadata/format, or ambiguous identity fails closed;
    /// it must never pick another language through best-stream/codec fallback.
    pub(super) fn select(&self, inventory: &TrackInventory) -> Result<i32, &'static str> {
        if let Some(observed) = &self.observed { return observed.resolve_stream_index(inventory); }
        if !inventory.is_complete() {
            return Err("HLS segment audio inventory is incomplete");
        }
        let mut current: Vec<_> = inventory.audio.iter().map(Descriptor::of).collect();
        current.sort();
        if current != self.expected {
            return Err("HLS audio inventory changed; reopen the source");
        }
        let mut matches = inventory.audio.iter().filter(|t| Descriptor::of(t) == self.selected);
        let selected = matches.next().ok_or("Selected HLS audio is missing")?;
        if matches.next().is_some() || selected.stream_index < 0 {
            return Err("HLS audio identity is ambiguous");
        }
        Ok(selected.stream_index)
    }
}

#[cfg(test)]
#[path = "hls_audio_tests.rs"]
mod tests;

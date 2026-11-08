//! Embedded MPEG-TS MP2 only. Decode to the existing PCM sink; no native MPEG
//! claim, server profile change, or separate-rendition admission is made here.
use super::*;

pub(crate) fn format(codec: &str, channels: i64, rate: i64) -> bool {
    codec == "mp2" && (1..=2).contains(&channels) && matches!(rate, 32_000 | 44_100 | 48_000)
}

pub(crate) fn supported(
    track: &inspect::InspectedTrack,
    caps: &plx_platform::devcaps::Caps,
) -> bool {
    format(&track.codec, track.channels, track.sample_rate)
        && track.decoder_available
        && caps.native_audio_track("pcm", 2, 48_000, false)
}

/// One worker owns the decoder across contiguous, completely fed segments.
/// Dropping it on a seek/discontinuity also drops synthesis/filter history.
#[derive(Default)]
pub(super) struct Decoder {
    inner: Option<crate::ff_pcm::PcmDecoder>,
    shape: Option<(i32, i32)>,
    timeline: Option<(i64, i64)>,
    last_stamp: Option<i64>,
}

impl Decoder {
    /// Keep the decoded sample clock across contiguous segments. Their EXTINF
    /// boundaries need not coincide with a 1152-sample MPEG audio frame.
    pub(super) fn map_stamp(&mut self, raw: i64, segment_start: i64) -> Result<i64, &'static str> {
        let (origin, base) = *self.timeline.get_or_insert((raw, segment_start));
        let mapped = raw
            .checked_sub(origin)
            .and_then(|delta| base.checked_add(delta))
            .ok_or("MP2 HLS audio timestamp overflow")?;
        if self.last_stamp.is_some_and(|previous| mapped < previous) {
            return Err("MP2 HLS audio timestamp regressed");
        }
        self.last_stamp = Some(mapped);
        Ok(mapped)
    }

    pub(super) unsafe fn prepare(
        &mut self,
        par: *const AVCodecParameters,
    ) -> Result<(), &'static str> {
        if pcm_out() != Some(crate::ff_pcm::STEREO_48K) || !pcm_lossy() {
            return Err("MP2 HLS requires its declared PCM output");
        }
        let shape = ((*par).ch_layout.nb_channels, (*par).sample_rate);
        if self.shape.is_some_and(|previous| previous != shape) {
            return Err("MP2 HLS audio format changed");
        }
        if self.inner.is_none() {
            self.inner = Some(
                crate::ff_pcm::PcmDecoder::open_hls_mp2(par, crate::ff_pcm::STEREO_48K)
                    .map_err(|_| "Could not open MP2 HLS audio decoder")?,
            );
            self.shape = Some(shape);
        }
        Ok(())
    }

    pub(super) unsafe fn decode(
        &mut self,
        packet: *const AVPacket,
        pts: i64,
    ) -> Result<(i64, Vec<u8>), &'static str> {
        let decoder = self
            .inner
            .as_mut()
            .ok_or("MP2 HLS decoder is not prepared")?;
        decoder
            .decode(packet, pts)
            .map(|(stamp, bytes)| (stamp, bytes.to_vec()))
            .map_err(|_| "MP2 HLS audio decode failed")
    }
}

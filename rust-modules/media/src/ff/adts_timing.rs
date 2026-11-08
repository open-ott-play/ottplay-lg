//! Packet-local ADTS duration facts shared by ordinary AAC and the strict HLS lane.
//! FFmpeg 9's AAC parser deliberately omits duration: HE-AAC changes samples and
//! rate together. Its ADTS core samples/rate ratio is nevertheless unchanged
//! (aac_ac3_parser.c). ADTS cannot signal the 960/120 window (adtsenc.c).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Config {
    mpeg_id: u8,
    profile: u8,
    rate_index: u8,
    channels: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Duration {
    pub(super) samples: i64,
    pub(super) rate: i64,
    pub(super) config: Config,
    single_block_frames: bool,
}
impl Duration {
    /// The new rendition recovery is narrower than ordinary ADTS duration
    /// accounting. It cannot widen the decoder's supported profile/block shape.
    pub(super) fn for_rendition(self) -> Option<Self> {
        (self.config.profile == 1 && self.single_block_frames).then_some(self)
    }
}

struct Header {
    config: Config,
    rate: i64,
    blocks: i64,
    header_size: usize,
    frame_size: usize,
}

/// Decode header facts once. This deliberately does not validate that the whole
/// declared frame is present: the legacy duration caller only needed seven bytes.
fn header(data: &[u8]) -> Option<Header> {
    const RATES: [i64; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000,
        12_000, 11_025, 8_000, 7_350,
    ];
    if data.len() < 7 || data[0] != 0xff || data[1] & 0xf6 != 0xf0 { return None; }
    let rate_index = (data[2] >> 2) & 15;
    let rate = *RATES.get(usize::from(rate_index))?;
    let config = Config { mpeg_id: (data[1] >> 3) & 1, profile: data[2] >> 6, rate_index,
        channels: ((data[2] & 1) << 2) | (data[3] >> 6) };
    let blocks = i64::from((data[6] & 3) + 1);
    // Protected multi-block ADTS also has a position for each later block.
    let header_size = if data[1] & 1 == 0 { 9 + 2 * (blocks as usize - 1) } else { 7 };
    let frame_size = (usize::from(data[3] & 3) << 11)
        | (usize::from(data[4]) << 3) | usize::from(data[5] >> 5);
    Some(Header { config, rate, blocks, header_size, frame_size })
}

/// Preserve the existing ordinary HLS contract exactly: first header only,
/// all header profiles and 1-4 raw blocks, regardless of trailing packet bytes.
/// This estimate never qualifies raw-clock recovery or a codec configuration.
pub(super) fn transport_duration_ns(data: &[u8]) -> Option<i64> {
    let value = header(data)?;
    Some(1_024 * value.blocks * 1_000_000_000 / value.rate)
}

/// Validate every complete frame, with no resynchronization or trailing bytes.
/// The core samples/rate ratio remains correct for backwards-compatible SBR.
/// CRC presence affects header size; this is framing proof, not CRC verification
/// or validation of the encoded raw-data blocks. This walk allocates nothing.
pub(super) fn duration(mut data: &[u8]) -> Option<Duration> {
    let mut result: Option<Duration> = None;
    while !data.is_empty() {
        let value = header(data)?;
        if value.frame_size <= value.header_size || value.frame_size > data.len() { return None; }
        match &mut result {
            Some(total) if total.config == value.config => {
                total.samples = total.samples.checked_add(value.blocks * 1_024)?;
                total.single_block_frames &= value.blocks == 1;
            }
            Some(_) => return None,
            None => result = Some(Duration { samples: value.blocks * 1_024, rate: value.rate,
                config: value.config, single_block_frames: value.blocks == 1 }),
        }
        data = &data[value.frame_size..];
    }
    result
}

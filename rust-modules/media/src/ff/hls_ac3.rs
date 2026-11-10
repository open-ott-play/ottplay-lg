//! Closed native AC-3 syncframe admission; no decoding, reframing, or I/O.
//! The caller retains transport timestamps and passes these exact frame bytes.
//! This validates framing/header shape, not payload CRC or decodability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub sample_rate: i64,
    pub channels: i64,
    pub duration_ns: i64,
}

pub fn format(codec: &str, channels: i64, sample_rate: i64) -> bool {
    codec == "ac3" && (1..=6).contains(&channels)
        && matches!(sample_rate, 32_000 | 44_100 | 48_000)
}

/// Exactly one complete, ordinary-rate AC-3 syncframe. No search/resync or
/// concatenated frames: silently skipping bytes would change packet timing.
pub fn frame(packet: &[u8], channels: i64, sample_rate: i64) -> Result<Frame, &'static str> {
    if !format("ac3", channels, sample_rate) { return Err("Unknown AC3 HLS audio format"); }
    if packet.len() < 7 || packet[..2] != [0x0b, 0x77] { return Err("Invalid AC3 syncframe header"); }
    let fscod = packet[4] >> 6;
    let size_code = packet[4] & 63;
    // bsid 9/10 are half/quarter-rate AC-3, 11..16 E-AC-3. Neither is
    // admitted under this initial full-rate AC-3 native sink contract.
    if fscod == 3 || size_code > 37 || packet[5] >> 3 > 8 {
        return Err("Unsupported AC3 syncframe format");
    }
    let rate = [48_000, 44_100, 32_000][usize::from(fscod)];
    const KBPS: [usize; 19] = [32,40,48,56,64,80,96,112,128,160,192,224,256,320,384,448,512,576,640];
    let bitrate = KBPS[usize::from(size_code >> 1)];
    let words = match fscod {
        0 => 2 * bitrate,
        1 => 320 * bitrate / 147 + usize::from(size_code & 1),
        _ => 3 * bitrate,
    };
    if packet.len() != 2 * words { return Err("AC3 packet is not one complete syncframe"); }
    let mut bits = Bits { data: packet, bit: 48 };
    let mode = bits.read(3)?;
    if mode == 2 { bits.read(2)?; }
    else {
        if mode & 1 != 0 && mode != 1 { bits.read(2)?; }
        if mode & 4 != 0 { bits.read(2)?; }
    }
    let lfe = bits.read(1)?;
    let actual_channels = [2,1,2,3,3,4,4,5][usize::from(mode)] + i64::from(lfe);
    if rate != sample_rate || actual_channels != channels { return Err("AC3 syncframe format changed"); }
    Ok(Frame { sample_rate: rate, channels: actual_channels, duration_ns: 1536 * 1_000_000_000 / rate })
}
struct Bits<'a> { data: &'a [u8], bit: usize }
impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<u8, &'static str> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self.data.get(self.bit / 8).ok_or("Truncated AC3 bitstream information")?;
            value = (value << 1) | ((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(value)
    }
}
#[cfg(test)]
#[path = "hls_ac3_tests.rs"]
mod tests;

//! Selected MPEG-TS subtitle decoding. Decode is private until the caller commits
//! the complete segment; a failed/interruptible feed must call `abort`.
use super::*;
use super::inspect::InspectedTrack;
use super::subs::open_sub_decoder;
use crate::player::{ass_source, text_ass::TextDialect, text_style::TextStyle};

const PACKET_BYTES: usize = 256 * 1024;
const SEGMENT_BYTES: usize = 4 * 1024 * 1024;
const CUE_LIMIT: usize = 1024;
const RECT_LIMIT: usize = 32;
const HEADER_BYTES: usize = 64 * 1024;

/// Capability alone is not provenance: callers must additionally resolve a
/// committed observed MPEG-TS Selection for the exact source/epoch/program/PID.
pub fn supported(track: &InspectedTrack) -> bool {
    track.kind == inspect::TrackKind::Subtitle &&
        (matches!(track.codec.as_str(), "ass" | "ssa") || TextDialect::from_codec_name(&track.codec).is_some()
        || track.decoder_available && matches!(track.codec.as_str(), "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle"))
}
#[derive(Clone, PartialEq, Eq)]
pub struct Scope {
    pub source: String,
    pub worker: u64,
    pub discontinuity: u64,
    pub transport: super::hls_observed::TransportIdentity,
}
#[derive(Clone)]
enum Header { Text(TextDialect), Ass(Vec<u8>), Bitmap }
struct Decoder {
    codec: String,
    codec_id: i32,
    extra: Vec<u8>,
    header: Header,
    context: *mut AVCodecContext,
    charset: crate::player::sub_charset::TrackCharset,
    track: i32,
}
impl Drop for Decoder {
    fn drop(&mut self) { if !self.context.is_null() { unsafe { avcodec_free_context(&mut self.context); } } }
}
impl Decoder {
    unsafe fn open(track: &InspectedTrack, cp: *const AVCodecParameters, ordinal: i32) -> Result<Self, &'static str> {
        if cp.is_null() || (*cp).codec_type != AVMEDIA_TYPE_SUBTITLE || ordinal < 0 || !supported(track) {
            return Err("Unsupported observed HLS subtitle");
        }
        let codec = std::ffi::CStr::from_ptr(avcodec_get_name((*cp).codec_id)).to_string_lossy();
        if codec != track.codec { return Err("HLS subtitle codec changed"); }
        let extra = Self::extradata(cp)?;
        let header = if matches!(track.codec.as_str(), "ass" | "ssa") { Header::Ass(extra.clone()) }
            else if let Some(dialect) = TextDialect::from_codec_name(&track.codec) { Header::Text(dialect) }
            else { Header::Bitmap };
        let context = if matches!(header, Header::Bitmap) { open_sub_decoder(cp) } else { std::ptr::null_mut() };
        if matches!(header, Header::Bitmap) && context.is_null() { return Err("HLS subtitle decoder unavailable"); }
        Ok(Self { codec: track.codec.clone(), codec_id: (*cp).codec_id, extra, header, context, track: ordinal,
            charset: crate::player::sub_charset::TrackCharset::new(Some(&track.language)) })
    }
    unsafe fn extradata(cp: *const AVCodecParameters) -> Result<Vec<u8>, &'static str> {
        let size = usize::try_from((*cp).extradata_size).map_err(|_| "Invalid HLS subtitle header")?;
        if size > HEADER_BYTES || size != 0 && (*cp).extradata.is_null() { return Err("Invalid HLS subtitle header"); }
        Ok(if size == 0 { Vec::new() } else { std::slice::from_raw_parts((*cp).extradata, size).to_vec() })
    }
}

enum Cue {
    Text { track: i32, start: i64, end: i64, payload: Vec<u8>, identity: u64 },
    Ass { track: i32, start: i64, end: i64, payload: Vec<u8> },
    Bitmap(subs::SubCue<'static>),
}
#[derive(Default)]
struct Pending { cues: Vec<Cue>, bytes: usize, packets: usize }
impl Pending {
    fn admit(&mut self, bytes: usize) -> Result<(), &'static str> {
        if self.cues.len() >= CUE_LIMIT { return Err("HLS subtitle cue limit exceeded"); }
        self.bytes = self.bytes.checked_add(bytes).filter(|size| *size <= SEGMENT_BYTES)
            .ok_or("HLS subtitle segment exceeds byte limit")?;
        Ok(())
    }
}
/// Owned batch; no pointers into the closed segment demuxer escape.
pub struct Batch {
    cues: Vec<Cue>,
    header: Header,
    track: i32,
    reset: bool,
}
impl Batch {
    pub fn cue_count(&self) -> usize { self.cues.len() }
    /// Call only after exact source/epoch/selection admission and complete A/V
    /// feed. `shift_ns` maps source packet timestamps into the committed clock.
    pub fn publish(self, media_key: &str, style: TextStyle, generation: &mut Option<u64>, shift_ns: i64) {
        if self.reset || generation.is_none() {
            let headers = match self.header {
                Header::Text(dialect) => vec![(self.track, ass_source::TrackHeader::Text(dialect))],
                Header::Ass(bytes) => vec![(self.track, ass_source::TrackHeader::Authored(bytes))],
                Header::Bitmap => Vec::new(),
            };
            *generation = Some(ass_source::begin(media_key, headers, Vec::new(), style));
            SHARED.sub_bitmaps.lock().unwrap_or_else(|error| error.into_inner()).clear();
        }
        let generation = generation.unwrap_or(0);
        for cue in self.cues {
            match cue {
                Cue::Text { track, start, end, payload, identity } => subs::SubCue::Text {
                    track, start, end, payload: std::borrow::Cow::Owned(payload), identity,
                }.shifted(shift_ns).push(generation),
                Cue::Ass { track, start, end, payload } => subs::SubCue::Ass {
                    track, start, end, payload: &payload,
                }.shifted(shift_ns).push(generation),
                Cue::Bitmap(cue) => cue.shifted(shift_ns).push(generation),
            }
        }
    }
}

#[derive(Default)]
pub struct State {
    scope: Option<Scope>,
    sequence: Option<u64>,
    committed: Option<u64>,
    decoder: Option<Decoder>,
    pending: Option<Pending>,
    reset: bool,
}
impl State {
    pub fn abort(&mut self) { *self = Self::default(); }
    /// Call with the selected stream from a currently validated PID snapshot.
    /// Existing decoder state is reused only after a committed contiguous segment.
    pub unsafe fn begin(&mut self, scope: Scope, sequence: u64, track: &InspectedTrack,
        cp: *const AVCodecParameters, renderer_ordinal: i32) -> Result<(), &'static str>
    {
        let contiguous = self.scope.as_ref() == Some(&scope) && self.pending.is_none()
            && self.committed.and_then(|last| last.checked_add(1)) == Some(sequence);
        if !contiguous { self.abort(); }
        if cp.is_null() || (*cp).codec_type != AVMEDIA_TYPE_SUBTITLE { self.abort(); return Err("Missing HLS subtitle codec"); }
        if let Some(decoder) = &self.decoder {
            if decoder.codec_id != (*cp).codec_id || decoder.codec != track.codec || decoder.extra != Decoder::extradata(cp)? || decoder.track != renderer_ordinal {
                self.abort(); return Err("HLS subtitle format changed");
            }
        } else {
            self.decoder = Some(Decoder::open(track, cp, renderer_ordinal)?);
            self.reset = true;
        }
        self.scope = Some(scope);
        self.sequence = Some(sequence);
        self.pending = Some(Pending::default());
        Ok(())
    }
    pub unsafe fn packet(&mut self, pkt: *mut AVPacket, stream: *mut AVStream) -> Result<(), &'static str> {
        let result = self.packet_inner(pkt, stream);
        if result.is_err() { self.abort(); }
        result
    }
    unsafe fn packet_inner(&mut self, pkt: *mut AVPacket, stream: *mut AVStream) -> Result<(), &'static str> {
        let pending = self.pending.as_mut().ok_or("HLS subtitle segment was not begun")?;
        let decoder = self.decoder.as_ref().ok_or("Missing HLS subtitle decoder")?;
        pending.packets += 1;
        if pending.packets > 4096 { return Err("HLS subtitle packet count exceeds limit"); }
        if pkt.is_null() || stream.is_null() || (*pkt).size < 0 || (*pkt).size as usize > PACKET_BYTES {
            return Err("HLS subtitle packet exceeds limit");
        }
        if (*pkt).size == 0 { return Ok(()); }
        if (*pkt).data.is_null() { return Err("Invalid HLS subtitle packet"); }
        let start = pts_ns_opt(pkt, stream).ok_or("HLS subtitle packet has no timestamp")?;
        if matches!(decoder.header, Header::Bitmap) {
            return decode_bitmap(decoder, pending, pkt, start);
        }
        let duration = if (*pkt).duration > 0 { av_rescale_q((*pkt).duration, stream_time_base(stream), NS_TB) }
            else { 4_000_000_000 };
        if !(1..=60_000_000_000).contains(&duration) { return Err("Invalid HLS subtitle duration"); }
        let end = start.checked_add(duration).ok_or("HLS subtitle timestamp overflow")?;
        let mut raw = std::slice::from_raw_parts((*pkt).data, (*pkt).size as usize);
        if matches!(decoder.header, Header::Text(TextDialect::MovText)) {
            if raw.len() < 2 { return Err("Invalid HLS mov_text packet"); }
            let size = u16::from_be_bytes([raw[0], raw[1]]) as usize;
            if size > raw.len() - 2 { return Err("Truncated HLS mov_text packet"); }
            raw = &raw[2..2 + size];
        }
        let cue = if matches!(decoder.header, Header::Ass(_)) {
            pending.admit(raw.len())?;
            Cue::Ass { track: decoder.track, start, end, payload: raw.to_vec() }
        } else {
            let payload = crate::player::sub_charset::subtitle_text(raw, &decoder.charset).into_owned().into_bytes();
            pending.admit(payload.len())?;
            Cue::Text { track: decoder.track, start, end, identity: ass_source::packet_identity(raw), payload }
        };
        pending.cues.push(cue);
        Ok(())
    }
    pub fn commit(&mut self) -> Result<Batch, &'static str> {
        let pending = self.pending.take().ok_or("HLS subtitle segment was not begun")?;
        let decoder = self.decoder.as_ref().ok_or("Missing HLS subtitle decoder")?;
        self.committed = self.sequence;
        let reset = std::mem::take(&mut self.reset);
        Ok(Batch { cues: pending.cues, header: decoder.header.clone(), track: decoder.track, reset })
    }
}

unsafe fn decode_bitmap(decoder: &Decoder, pending: &mut Pending, pkt: *mut AVPacket, pts: i64) -> Result<(), &'static str> {
    struct Display(AVSubtitle);
    impl Drop for Display { fn drop(&mut self) { unsafe { avsubtitle_free(&mut self.0); } } }
    let mut display = Display(std::mem::zeroed());
    let sub = &mut display.0;
    let mut got = 0;
    if avcodec_decode_subtitle2(decoder.context, sub, &mut got, pkt) < 0 { return Err("HLS subtitle decode failed"); }
    if got == 0 { return Ok(()); }
    if sub.num_rects == 0 {
        pending.admit(0)?;
        pending.cues.push(Cue::Bitmap(subs::SubCue::BitmapClear { track: decoder.track, pts }));
        return Ok(());
    }
    if sub.num_rects as usize > RECT_LIMIT || sub.rects.is_null() { return Err("HLS subtitle rectangle limit exceeded"); }
    let mut rects = Vec::new();
    for index in 0..sub.num_rects as usize {
        let rect = *sub.rects.add(index);
        if rect.is_null() { return Err("Invalid HLS subtitle rectangle"); }
        let r = &*rect;
        if r.type_ != 1 || r.w <= 0 || r.h <= 0 || r.w > 4096 || r.h > 4096 || r.linesize[0] < r.w || r.linesize[0] > 8192
            || r.data[0].is_null() || r.data[1].is_null() { return Err("Invalid HLS subtitle bitmap"); }
        let pixels = (r.w as usize).checked_mul(r.h as usize).ok_or("HLS subtitle bitmap overflow")?;
        pending.admit(pixels.checked_add(1024).ok_or("HLS subtitle bitmap overflow")?)?;
        let mut indices = Vec::with_capacity(pixels);
        for row in 0..r.h as usize {
            indices.extend_from_slice(std::slice::from_raw_parts(r.data[0].add(row * r.linesize[0] as usize), r.w as usize));
        }
        let mut palette = Box::new([[0; 4]; 256]);
        for (index, color) in palette.iter_mut().enumerate() {
            let argb = *(r.data[1] as *const u32).add(index);
            *color = [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8];
        }
        rects.push(crate::player::SubRect { x: r.x, y: r.y, w: r.w, h: r.h, index: indices, palette });
    }
    // Ask the library for its authored canvas using the existing checked codecpar ABI.
    let cp = avcodec_parameters_alloc();
    if cp.is_null() { return Err("Could not allocate HLS subtitle canvas"); }
    let canvas = if avcodec_parameters_from_context(cp, decoder.context) >= 0 { ((*cp).width, (*cp).height) } else { (0, 0) };
    let mut cp = cp;
    avcodec_parameters_free(&mut cp);
    pending.cues.push(Cue::Bitmap(subs::SubCue::BitmapSet { track: decoder.track, pts,
        cw: canvas.0, ch: canvas.1, total_rects: rects.len(), rects }));
    Ok(())
}

#[cfg(test)]
#[path = "hls_embedded_subtitles_tests.rs"]
mod tests;

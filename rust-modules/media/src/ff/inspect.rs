//! Bounded, independent media inspection for sources which have no Plex metadata.
//! Uses the same bundled demuxer and verified HTTP transport as playback. It never
//! publishes the active player's abort handle or mutates its queues/track state.
use super::*;
#[path = "inspect_tracks.rs"]
mod tracks;
use plx_base::checkpoint::{Checkpoint, Flow};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};
pub use tracks::{InspectedTrack, TrackInventory, TrackKind};

/// The segment reader owns a live, probed context under the same checked ABI.
pub(super) unsafe fn collect_tracks(context: *mut AVFormatContext) -> Result<TrackInventory, &'static str> {
    tracks::collect(context)
}

struct InspectCheckpoint<'a>(&'a AtomicBool);
impl Checkpoint for InspectCheckpoint<'_> {
    fn check(&mut self) -> Flow {
        if self.0.load(Ordering::Relaxed) {
            Flow::Stop
        } else {
            Flow::Continue {
                next_check: Some(Instant::now() + Duration::from_millis(100)),
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub resolved_source: Option<ExternalSource>,
    pub hls_playback: Option<super::hls_presentation::HlsPlayback>,
    pub tracks: TrackInventory,
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u16,
    pub height: u16,
    pub fps: f64,
    pub duration_ns: i64,
    pub audio_channels: i64,
    pub audio_sample_rate: i64,
    pub audio_bit_depth: i64,
    pub dovi: plx_data::metadata::Dovi,
}

struct Input {
    source: InputSource,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    bytes: usize,
    offset: i64,
    parked_at_end: bool,
    prefix: Vec<u8>,
    prefix_at: usize,
}
enum InputSource {
    Http(Box<crate::curlio::CurlSource>),
    Local(file_source::FileSource),
}
impl InputSource {
    fn abort(&self) {
        if let Self::Http(source) = self {
            source.abort();
        }
    }
    fn size(&self) -> i64 {
        match self {
            Self::Http(source) => source.size(),
            Self::Local(source) => source.size(),
        }
    }
    fn read(&mut self, bytes: &mut [u8], deadline: Instant, cancelled: &AtomicBool) -> i32 {
        match self {
            Self::Http(source) => {
                source.read_until(bytes, Some(deadline), &mut InspectCheckpoint(cancelled))
            }
            Self::Local(file) => std::io::Read::read(file, bytes)
                .map(|n| n as i32)
                .unwrap_or(-1),
        }
    }
    fn seek(&mut self, position: i64, deadline: Instant, cancelled: &AtomicBool) -> bool {
        match self {
            Self::Http(source) => {
                source.seek_until(position, Some(deadline), &mut InspectCheckpoint(cancelled))
            }
            Self::Local(file) => {
                std::io::Seek::seek(file, std::io::SeekFrom::Start(position as u64)).is_ok()
            }
        }
    }
}
impl Input {
    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
            || Instant::now() >= self.deadline
            || self.bytes >= 8 * 1024 * 1024
    }
}
extern "C" fn read(opaque: *mut c_void, buffer: *mut u8, size: c_int) -> c_int {
    if opaque.is_null() || buffer.is_null() || size <= 0 {
        return AVERROR_IO;
    }
    // The owned Input and AVIO buffer outlive every synchronous libavformat call.
    let input = unsafe { &mut *opaque.cast::<Input>() };
    if input.stopped() {
        input.source.abort();
        return AVERROR_IO;
    }
    if input.parked_at_end {
        return AVERROR_EOF;
    }
    let count = (size as usize).min(8 * 1024 * 1024 - input.bytes);
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, count) };
    if input.prefix_at < input.prefix.len() {
        let n = buffer.len().min(input.prefix.len() - input.prefix_at);
        buffer[..n].copy_from_slice(&input.prefix[input.prefix_at..input.prefix_at + n]);
        input.prefix_at += n;
        input.bytes += n;
        input.offset += n as i64;
        return n as i32;
    }
    let n = input.source.read(buffer, input.deadline, &input.cancelled);
    if n > 0 {
        input.bytes += n as usize;
        input.offset += i64::from(n);
        n
    } else if n == 0 {
        AVERROR_EOF
    } else {
        AVERROR_IO
    }
}
extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    if opaque.is_null() {
        return -1;
    }
    let input = unsafe { &mut *opaque.cast::<Input>() };
    if input.stopped() {
        input.source.abort();
        return -1;
    }
    if whence == AVSEEK_SIZE {
        return input.source.size();
    }
    let base = match whence & !0x20000 {
        SEEK_SET => 0,
        SEEK_CUR => input.offset,
        SEEK_END if input.source.size() >= 0 => input.source.size(),
        _ => return -1,
    };
    let Some(position) = base.checked_add(offset).filter(|p| *p >= 0) else {
        return -1;
    };
    if input.source.size() >= 0 && position >= input.source.size() {
        input.offset = position;
        input.parked_at_end = true;
        return position;
    }
    if input
        .source
        .seek(position, input.deadline, &input.cancelled)
    {
        input.prefix.clear();
        input.prefix_at = 0;
        input.offset = position;
        input.parked_at_end = false;
        position
    } else {
        -1
    }
}

/// Called by an application worker, before any native Load. Cancelled/stale results
/// are discarded by that worker's generation owner. No URL enters diagnostics.
pub fn http(url: &str, cancelled: Arc<AtomicBool>) -> Result<MediaInfo, &'static str> {
    source(&ExternalSource::Http(url.into()), cancelled)
}

pub fn source(
    source: &ExternalSource,
    cancelled: Arc<AtomicBool>,
) -> Result<MediaInfo, &'static str> {
    match source {
        ExternalSource::Hls(url) => super::external_hls::inspect(url, cancelled),
        ExternalSource::Http(url)
            if url
                .split('?')
                .next()
                .unwrap_or(url)
                .to_ascii_lowercase()
                .ends_with(".m3u8") =>
        {
            super::external_hls::inspect(url, cancelled)
        }
        _ => raw_inner(source, cancelled, true, None),
    }
}

pub(super) fn raw(
    source: &ExternalSource,
    cancelled: Arc<AtomicBool>,
) -> Result<MediaInfo, &'static str> {
    raw_inner(source, cancelled, false, None)
}
pub(super) fn raw_lane(
    source: &ExternalSource, cancelled: Arc<AtomicBool>, lane: super::hls_rendition::Lane,
) -> Result<MediaInfo, &'static str> {
    raw_inner(source, cancelled, false, Some(lane))
}
type VideoTiming = Result<super::hls_rendition::TimedLaneSegment, &'static str>;

/// The same first-segment inspection, optionally retaining its real video clock.
/// A timing-only refusal does not make otherwise valid A/V unavailable.
pub(super) fn raw_video_timed(source: &ExternalSource, cancelled: Arc<AtomicBool>)
    -> Result<(MediaInfo, VideoTiming), &'static str>
{
    let mut timing = None;
    let info = raw_inner_capture(source, cancelled, false,
        Some(super::hls_rendition::Lane::Video), Some(&mut timing))?;
    Ok((info, timing.unwrap_or(Err("HLS subtitle video clock is unavailable"))))
}
fn raw_inner(
    source: &ExternalSource,
    cancelled: Arc<AtomicBool>,
    recognize_hls: bool,
    lane: Option<super::hls_rendition::Lane>,
) -> Result<MediaInfo, &'static str> {
    raw_inner_capture(source, cancelled, recognize_hls, lane, None)
}
fn raw_inner_capture(
    source: &ExternalSource,
    cancelled: Arc<AtomicBool>,
    recognize_hls: bool,
    lane: Option<super::hls_rendition::Lane>,
    mut timing: Option<&mut Option<VideoTiming>>,
) -> Result<MediaInfo, &'static str> {
    if !ABI_OK.load(Ordering::Relaxed) {
        return Err("Bundled media demuxer is unavailable");
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Media inspection cancelled");
    }
    ensure_registered();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut prefix = Vec::new();
    let source = match source {
        ExternalSource::Http(url) => {
            let url = plx_data::ott::http_url(url).ok_or("Invalid HTTP media address")?;
            let (mut source, _abort) = crate::curlio::CurlSource::open_private_checked(
                &url,
                0,
                Some(deadline),
                &mut InspectCheckpoint(&cancelled),
            )
            .map_err(|_| "Could not open media source")?;
            if recognize_hls {
                let mut head = [0u8; 512];
                while prefix.len() < 16 {
                    let n = source.read_until(
                        &mut head,
                        Some(deadline),
                        &mut InspectCheckpoint(&cancelled),
                    );
                    if n < 0 {
                        return Err("Could not read media source");
                    }
                    if n == 0 {
                        break;
                    }
                    prefix.extend_from_slice(&head[..n as usize]);
                }
                if String::from_utf8_lossy(&prefix)
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .starts_with("#EXTM3U")
                {
                    let landed = source.effective_url().to_owned();
                    drop(source);
                    return super::external_hls::inspect(&landed, cancelled);
                }
            }
            InputSource::Http(source)
        }
        ExternalSource::OwnedHttp(url) => {
            let (mut source, _abort) = crate::curlio::CurlSource::open_owned_private_checked(
                url, 0, Some(deadline), &mut InspectCheckpoint(&cancelled),
            ).map_err(|_| "Could not open local companion source")?;
            let mut head = [0u8; 512];
            while prefix.len() < 16 {
                let n = source.read_until(&mut head, Some(deadline), &mut InspectCheckpoint(&cancelled));
                if n < 0 { return Err("Could not read local companion source"); }
                if n == 0 { break; }
                prefix.extend_from_slice(&head[..n as usize]);
            }
            if String::from_utf8_lossy(&prefix).trim_start_matches('\u{feff}').trim_start().starts_with("#EXTM3U") {
                return Err("Companion source must be a media file, not a playlist");
            }
            InputSource::Http(source)
        }
        ExternalSource::Local(path) => InputSource::Local(
            file_source::FileSource::open(path).map_err(|_| "Could not open local media")?),
        ExternalSource::HeldLocal(file) => InputSource::Local(
            file_source::FileSource::held(file).map_err(|_| "Local media source was withdrawn")?),
        ExternalSource::Hls(_) => return Err("Segmented media needs playlist inspection"),
    };
    let mut input = Input {
        source,
        deadline,
        cancelled,
        bytes: 0,
        offset: 0,
        parked_at_end: false,
        prefix,
        prefix_at: 0,
    };
    unsafe {
        let format = if lane.is_some() {
            let format = av_find_input_format(c"mpegts".as_ptr());
            if format.is_null() { return Err("MPEG-TS demuxer is unavailable"); }
            format.cast_mut()
        } else { std::ptr::null_mut() };
        let buffer = av_malloc(32768).cast::<u8>();
        if buffer.is_null() {
            return Err("Media inspection allocation failed");
        }
        // One pointer to the stack-owned Input is shared with synchronous AVIO
        // callbacks. Input stays in place until the context and AVIO are freed.
        let input_ptr = std::ptr::addr_of_mut!(input);
        let io = avio_alloc_context(
            buffer,
            32768,
            0,
            input_ptr.cast(),
            Some(read),
            None,
            Some(seek),
        );
        if io.is_null() {
            free_ptr(buffer.cast());
            return Err("Media inspection allocation failed");
        }
        let mut context = avformat_alloc_context();
        if context.is_null() {
            free_avio(io);
            return Err("Media inspection allocation failed");
        }
        (*context).pb = io;
        av_opt_set(
            context.cast(),
            c"probesize".as_ptr(),
            c"2097152".as_ptr(),
            0,
        );
        av_opt_set(
            context.cast(),
            c"analyzeduration".as_ptr(),
            c"2000000".as_ptr(),
            0,
        );
        let opened = avformat_open_input(
            &mut context,
            std::ptr::null(),
            format,
            std::ptr::null_mut(),
        );
        let result = if opened < 0 || context.is_null() {
            Err("Unsupported or unavailable media container")
        } else if avformat_find_stream_info(context, std::ptr::null_mut()) < 0 {
            Err("Could not identify media streams")
        } else if let Some(lane) = lane {
            describe_lane(context, lane)
        } else {
            describe(context)
        };
        let inspection_stopped = input.stopped();
        if result.is_ok() && !inspection_stopped {
            if let Some(out) = timing.as_mut() {
                **out = Some(capture_video_timing(context, input_ptr));
            }
        }
        if !context.is_null() {
            avformat_close_input(&mut context);
        }
        free_avio(io);
        if inspection_stopped || input.cancelled.load(Ordering::Relaxed)
            || (timing.is_none() && input.stopped()) {
            Err("Media inspection cancelled or exceeded its limit")
        } else {
            result
        }
    }
}

/// Read only this initial segment under Input's existing byte/deadline/abort bounds.
/// There is no playback queue, shared telemetry, native session or decoder output.
/// `input` is the original opaque AVIO pointer. Never retain a Rust reference
/// across an FFmpeg call: read/seek callbacks exclusively borrow that same Input.
unsafe fn capture_video_timing(context: *mut AVFormatContext, input: *mut Input) -> VideoTiming {
    use super::hls_rendition::{Binding, Lane, SourceTiming, TimedLaneSegment, TimedSample};
    let binding = Binding::inspect(Lane::Video, context)?;
    let stream = *(*context).streams;
    let tb = stream_time_base(stream);
    if tb.num != 1 || tb.den != 90_000 { return Err("HLS subtitles require a video PES clock"); }
    let mut packet = av_packet_alloc();
    if packet.is_null() { return Err("Media inspection allocation failed"); }
    let result = (|| {
        let mut samples = Vec::new();
        loop {
            let stopped = { (&*input).stopped() };
            if stopped { return Err("HLS subtitle clock capture exceeded its limit"); }
            let ret = av_read_frame(context, packet);
            if ret == AVERROR_EOF { break; }
            if ret < 0 { return Err("HLS subtitle clock capture failed"); }
            if samples.len() >= 16_384 || (*packet).pts == AV_NOPTS_VALUE
                || (*packet).stream_index != stream_index(stream) || (*packet).duration <= 0 {
                return Err("HLS subtitle clock capture has invalid timing");
            }
            samples.push(TimedSample { data: Vec::new(), key: false, timing: SourceTiming {
                pts: (*packet).pts,
                dts: ((*packet).dts != AV_NOPTS_VALUE).then_some((*packet).dts),
                duration: Some((*packet).duration), time_base: (tb.num, tb.den),
            } });
            av_packet_unref(packet);
        }
        let incomplete = {
            let input = &*input;
            input.stopped() || (input.source.size() >= 0 && input.offset != input.source.size())
        };
        if incomplete { return Err("HLS subtitle clock capture is incomplete"); }
        if Binding::inspect(Lane::Video, context)? != binding { return Err("HLS video binding changed"); }
        Ok(TimedLaneSegment { binding, samples, transfer: SegmentTransfer {
            bytes: 0, active_us: 0, total_us: 0, audio_expected: false,
        } })
    })();
    av_packet_free(&mut packet);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_probe_seek_to_end_does_not_invalidate_the_next_read() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "ott-probe-end-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"ABCDEFGH")
            .unwrap();
        let file = file_source::FileSource::open(&path).unwrap();
        let mut input = Input {
            source: InputSource::Local(file),
            deadline: Instant::now() + Duration::from_secs(2),
            cancelled: Arc::new(AtomicBool::new(false)),
            bytes: 0,
            offset: 0,
            parked_at_end: false,
            prefix: Vec::new(),
            prefix_at: 0,
        };
        let ptr = (&mut input as *mut Input).cast();
        let mut bytes = [0; 4];
        assert_eq!(seek(ptr, 8, SEEK_SET), 8);
        assert_eq!(read(ptr, bytes.as_mut_ptr(), 4), AVERROR_EOF);
        assert_eq!(seek(ptr, 4, SEEK_SET), 4);
        assert_eq!(read(ptr, bytes.as_mut_ptr(), 4), 4);
        assert_eq!(&bytes, b"EFGH");
        drop(input);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    #[ignore = "requires the bundled host FFmpeg dylibs (PLX_FFMPEG_DIR)"]
    fn bundled_probe_reads_local_mp4_without_importing_it() {
        let _serial = plx_base::testlock::serial();
        let _ffmpeg = super::super::image_subtitle_tests::bind_host_ffmpeg();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../site/media/feel-720p60.h264.mp4")
            .canonicalize()
            .unwrap();
        let before = std::fs::metadata(&path).unwrap();
        let info = source(
            &ExternalSource::Local(path.clone()),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(info.video_codec, "h264");
        assert_eq!((info.width, info.height), (1280, 720));
        assert!(info.duration_ns > 26_000_000_000);
        assert!(info.audio_codec.is_empty());
        assert!(info.tracks.is_complete());
        assert!(info.tracks.audio.is_empty() && info.tracks.subtitles.is_empty());
        let after = std::fs::metadata(path).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }
}

unsafe fn describe(context: *mut AVFormatContext) -> Result<MediaInfo, &'static str> {
    let tracks = tracks::collect(context)?;
    let mut video = None;
    let mut audio = None;
    let best_video =
        av_find_best_stream(context, AVMEDIA_TYPE_VIDEO, -1, -1, std::ptr::null_mut(), 0);
    for i in 0..(*context).nb_streams as usize {
        let stream = *(*context).streams.add(i);
        if stream.is_null() {
            continue;
        }
        let codec = stream_codecpar(stream);
        if codec.is_null() {
            continue;
        }
        if i as i32 == best_video && (*codec).codec_type == AVMEDIA_TYPE_VIDEO {
            video = Some((stream, codec));
        }
        if (*codec).codec_type == AVMEDIA_TYPE_AUDIO && audio.is_none() {
            audio = Some(codec);
        }
    }
    let Some((_stream, video)) = video else {
        return Err("No supported video stream found");
    };
    let name = |id| {
        let ptr = avcodec_get_name(id);
        if ptr.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    };
    Ok(MediaInfo {
        resolved_source: None,
        hls_playback: None,
        tracks,
        video_codec: name((*video).codec_id),
        audio_codec: audio.map_or_else(String::new, |a| name((*a).codec_id)),
        width: u16::try_from((*video).width).unwrap_or(0),
        height: u16::try_from((*video).height).unwrap_or(0),
        fps: if (*video).framerate.num > 0 && (*video).framerate.den > 0 {
            f64::from((*video).framerate.num) / f64::from((*video).framerate.den)
        } else {
            0.0
        },
        duration_ns: fmt_duration(context).max(0).saturating_mul(1000),
        audio_channels: audio.map_or(0, |a| i64::from((*a).ch_layout.nb_channels)),
        audio_sample_rate: audio.map_or(0, |a| i64::from((*a).sample_rate)),
        audio_bit_depth: audio.map_or(0, |a| {
            i64::from((*a).bits_per_raw_sample.max((*a).bits_per_coded_sample))
        }),
        dovi: dovi_conf(video).map_or(plx_data::metadata::Dovi::NONE, |d| {
            plx_data::metadata::Dovi {
                present: true,
                profile: i64::from(d.dv_profile),
                level: i64::from(d.dv_level),
                bl_compat: i64::from(d.dv_bl_signal_compatibility_id),
                el_present: d.el_present_flag != 0,
                version: (i64::from(d.dv_version_major), i64::from(d.dv_version_minor)),
                bl_present: d.bl_present_flag != 0,
                rpu_present: d.rpu_present_flag != 0,
            }
        }),
    })
}

unsafe fn describe_lane(context: *mut AVFormatContext, lane: super::hls_rendition::Lane)
    -> Result<MediaInfo, &'static str>
{
    use super::hls_rendition::{Binding, Lane};
    let binding = Binding::inspect(lane, context)?;
    if lane == Lane::Video { return describe(context); }
    let Binding::Audio(track) = binding else { return Err("Invalid HLS audio probe"); };
    Ok(MediaInfo {
        resolved_source: None,
        hls_playback: None, tracks: tracks::collect(context)?,
        video_codec: String::new(), audio_codec: track.codec, width: 0, height: 0,
        fps: 0.0, duration_ns: fmt_duration(context).max(0).saturating_mul(1000),
        audio_channels: track.channels, audio_sample_rate: track.sample_rate,
        audio_bit_depth: track.bit_depth, dovi: plx_data::metadata::Dovi::NONE,
    })
}

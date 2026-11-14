//! Logical/native ownership captured before the media thread is spawned.
use super::*;
use std::cell::RefCell;
use std::rc::Rc;
#[derive(Clone)]
struct Owner { playback: crate::route::PlaybackIdentity, epoch: u32, subtitles: Rc<RefCell<Subtitles>> }
thread_local! { static OWNER: RefCell<Option<Owner>> = const { RefCell::new(None) }; }
/// Bind this demux invocation to its actual logical item and native Load.
pub fn with_observed_hls_worker<T>(epoch: u32, playback: crate::route::PlaybackIdentity, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Owner>);
    impl Drop for Restore { fn drop(&mut self) {
        OWNER.with(|slot| *slot.borrow_mut() = self.0.take());
    } }
    let previous = OWNER.with(|slot| slot.replace(Some(Owner { playback, epoch,
        subtitles: Rc::new(RefCell::new(Subtitles::default())) })));
    let _restore = Restore(previous);
    run()
}

#[derive(Default)]
struct Subtitles {
    decoder: hls_embedded_subtitles::State,
    request: Option<hls_observed::SubtitleRequest>,
    scope: Option<hls_observed::Scope>,
    source: String,
    sequence: u64,
    selected: Option<inspect::InspectedTrack>,
    batch: Option<hls_embedded_subtitles::Batch>,
    renderer: Option<u64>,
}
impl Subtitles {
    fn abort(&mut self) {
        self.decoder.abort(); self.selected = None; self.batch = None; self.renderer = None;
    }
}

/// One owned transaction on this demux thread. No caption state is shared with
/// another Load, and no borrowed AVStream/packet escapes its segment demuxer.
pub(super) struct SubtitleSegment { committed: bool }
impl Drop for SubtitleSegment {
    fn drop(&mut self) { if !self.committed { abort_subtitle_segment(); } }
}
impl SubtitleSegment {
    pub(super) fn publish(mut self, source: &str, variant: &str, discontinuity: u64,
        sequence: u64, shift: Option<i64>, recovery_generation: u64)
    {
        self.committed = publish_subtitle_segment(source, variant, discontinuity, sequence, shift, recovery_generation);
    }
}
pub(super) fn begin_subtitle_segment(source: &str, variant: &str, discontinuity: u64, sequence: u64) -> SubtitleSegment {
    let guard = SubtitleSegment { committed: false };
    let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) else { return guard; };
    let mut subtitles = owner.subtitles.borrow_mut();
    // A previous un-published batch means its feed failed or was interrupted.
    if subtitles.batch.is_some() { subtitles.abort(); }
    let scope = scope(source, variant, discontinuity);
    let request = hls_observed::subtitle_request(&owner.playback, source)
        .filter(|request| scope.as_ref().is_some_and(|scope| request.matches_scope(scope)));
    if subtitles.request.as_ref().map(|request| request.generation())
        != request.as_ref().map(|request| request.generation())
        || subtitles.scope != scope || request.is_none()
    { subtitles.abort(); }
    subtitles.request = request;
    subtitles.scope = scope;
    subtitles.source = format!("{source}\n{variant}");
    subtitles.sequence = sequence;
    subtitles.selected = None;
    guard
}

unsafe fn selected_stream(fmt: *mut AVFormatContext, index: i32) -> Option<*mut AVStream> {
    if fmt.is_null() || (*fmt).nb_streams > 256 { return None; }
    let mut found = None;
    for position in 0..(*fmt).nb_streams {
        let stream = *(*fmt).streams.add(position as usize);
        if !stream.is_null() && stream_index(stream) == index {
            if found.is_some() { return None; }
            found = Some(stream);
        }
    }
    found
}

pub(super) unsafe fn prepare_subtitle_segment(fmt: *mut AVFormatContext, inventory: &inspect::TrackInventory) {
    let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) else { return; };
    let mut subtitles = owner.subtitles.borrow_mut();
    let prepared = (|| {
        let request = subtitles.request.as_ref()?;
        if !hls_observed::subtitle_request_is_current(request) { return None; }
        let index = request.selection().resolve_stream_index(inventory).ok()?;
        let track = inventory.subtitles.iter().find(|track| track.stream_index == index)?.clone();
        if !hls_embedded_subtitles::supported(&track) { return None; }
        let transport = inventory.transport_identity(index)?;
        let stream = selected_stream(fmt, index)?;
        let scope = subtitles.scope.as_ref()?;
        let decoder_scope = hls_embedded_subtitles::Scope {
            source: subtitles.source.clone(), worker: u64::from(owner.epoch),
            discontinuity: scope.discontinuity(), transport,
        };
        Some((track, stream, decoder_scope, subtitles.sequence))
    })();
    let Some((track, stream, scope, sequence)) = prepared else { subtitles.abort(); return; };
    if subtitles.decoder.begin(scope, sequence, &track, stream_codecpar(stream), track.ordinal).is_ok() {
        subtitles.selected = Some(track);
    } else { subtitles.abort(); }
}

pub(super) unsafe fn subtitle_packet(fmt: *mut AVFormatContext, packet: *mut AVPacket) {
    let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) else { return; };
    let mut subtitles = owner.subtitles.borrow_mut();
    let Some(track) = subtitles.selected.as_ref() else { return; };
    if packet.is_null() || (*packet).stream_index != track.stream_index { return; }
    let Some(stream) = selected_stream(fmt, track.stream_index) else { subtitles.abort(); return; };
    if subtitles.decoder.packet(packet, stream).is_err() { subtitles.abort(); }
}

pub(super) unsafe fn finish_subtitle_segment(fmt: *mut AVFormatContext, inventory: &inspect::TrackInventory) {
    let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) else { return; };
    let mut subtitles = owner.subtitles.borrow_mut();
    let valid = (|| {
        let request = subtitles.request.as_ref()?;
        let selected = subtitles.selected.as_ref()?;
        let index = request.selection().resolve_stream_index(inventory).ok()?;
        let track = inventory.subtitles.iter().find(|track| track.stream_index == index)?;
        if selected != track || !hls_observed::subtitle_request_is_current(request) { return None; }
        let stream = selected_stream(fmt, index)?;
        subtitles.decoder.format_is_current(stream_codecpar(stream), track.ordinal).then_some(())
    })().is_some();
    if !valid { subtitles.abort(); return; }
    match subtitles.decoder.commit() {
        Ok(batch) => subtitles.batch = Some(batch),
        Err(_) => subtitles.abort(),
    }
}

pub(super) fn abort_subtitle_segment() {
    if let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) { owner.subtitles.borrow_mut().abort(); }
}

/// Called only after complete A/V feeding and its final source-clock mapping.
/// Native epoch -> selection -> renderer is the publication lock order. No
/// inventory lock, IO, decoder call or worker launch occurs under these guards.
fn publish_subtitle_segment(source: &str, variant: &str, discontinuity: u64,
    sequence: u64, shift: Option<i64>, recovery_generation: u64) -> bool
{
    let Some(owner) = OWNER.with(|slot| slot.borrow().clone()) else { return false; };
    let mut subtitles = owner.subtitles.borrow_mut();
    let batch = subtitles.batch.take().and_then(|batch| shift.and_then(|shift| batch.shifted(shift)));
    if batch.is_none() { subtitles.abort(); }
    let current_scope = scope(source, variant, discontinuity);
    if subtitles.sequence != sequence || subtitles.scope != current_scope {
        subtitles.abort(); return false;
    }
    let Some(request) = subtitles.request.clone() else { subtitles.abort(); return false; };
    let style = crate::player::text_style::TextStyle::current(Default::default());
    let published = SHARED.with_active_native_load(owner.epoch, || {
        hls_observed::with_current_subtitle_request(&request, || {
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0
                || SHARED.hls_candidate_generation.load(Ordering::Acquire) != recovery_generation
            { return false; }
            if let Some(batch) = batch {
                batch.publish(source, style, &mut subtitles.renderer, 0);
            } else {
                // A failed caption transaction cannot leave an open bitmap or
                // authored cue on screen indefinitely. Retire only while this
                // exact selection and native Load still own the renderer.
                crate::player::ass_source::retire_selection();
                SHARED.sub_bitmaps.lock().unwrap_or_else(|error| error.into_inner()).clear();
                subtitles.renderer = None;
            }
            true
        }).unwrap_or(false)
    }).unwrap_or(false);
    if !published { subtitles.abort(); }
    published
}
pub(super) fn scope(source: &str, variant: &str, discontinuity: u64) -> Option<hls_observed::Scope> {
    let owner = OWNER.with(|slot| slot.borrow().clone())?;
    hls_observed::Scope::new(source, variant, u64::from(owner.epoch), discontinuity).ok()
        .map(|scope| scope.with_playback(owner.playback))
}

//! Non-Plex artwork source for the shared texture cache. HTTP and authorized
//! local reads share these slots, one worker and the existing thumbnail decoder.
use super::{tex, Decoded, PosterError, PosterKey, PosterReady};
use crate::app::ott_local_root::RootHandle;
use crate::app::ott_local_session::{BoundSource, SourceKind};
use plx_base::task::OffFrame;
use std::sync::{Arc, Mutex};

// Classic supports 30 visible logos plus separately sized detail artwork.
// Keep source metadata bounded while leaving room for the departing page.
const CAP: usize = 64;
const FIRST_KEY: u32 = 0x8000_0000;
const MAX_BYTES: usize = 8 * 1024 * 1024;
struct Slot {
    input: Input,
    w: u32,
    h: u32,
    key: PosterKey,
    touched: u64,
    frame: Option<u64>,
    state: State,
}
// Private inputs never enter a UI/wire value. The descriptor is an identity
// comparison, not a new grant; only the caller-authorized RootHandle grants IO.
#[derive(Clone)]
enum Input {
    Http(String),
    Local {
        source: BoundSource,
        root: Arc<RootHandle>,
        relative_path: String,
    },
}
impl Input {
    fn live(&self) -> bool {
        match self {
            Self::Http(_) => true,
            Self::Local { root, .. } => root.is_live(),
        }
    }
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Http(a), Self::Http(b)) => a == b,
            (
                Self::Local {
                    source: a,
                    root: ar,
                    relative_path: ap,
                },
                Self::Local {
                    source: b,
                    root: br,
                    relative_path: bp,
                },
            ) => {
                a.id == b.id
                    && a.authority_ref == b.authority_ref
                    && a.generation == b.generation
                    && Arc::ptr_eq(ar, br)
                    && ap == bp
            }
            _ => false,
        }
    }
}
enum State {
    Loading,
    Decoded(Decoded),
    Ready,
    Failed(Option<u32>),
    Evicted,
}
struct Store {
    slots: Vec<Slot>,
    clock: u64,
    frame: u64,
    next: u32,
    generation: u64,
    busy: bool,
}
static STORE: Mutex<Store> = Mutex::new(Store {
    slots: Vec::new(),
    clock: 0,
    frame: 0,
    next: FIRST_KEY,
    generation: 0,
    busy: false,
});

// Main/GL thread, once before probing the new frame. Retain previous-frame
// demand too: an early row must not evict a later row before it is touched again.
pub(super) fn begin_frame() {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.frame = store.frame.wrapping_add(1);
}

pub(super) fn owns(key: PosterKey) -> bool {
    key.0 >= FIRST_KEY
}
pub(super) fn idle() -> bool {
    let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    !store.busy
        && !store
            .slots
            .iter()
            .any(|s| matches!(s.state, State::Decoded(_)))
}
pub(super) fn reset() {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.generation = store.generation.wrapping_add(1);
    let keys: Vec<_> = store.slots.drain(..).map(|slot| slot.key).collect();
    // Reset invalidates publication, not the worker still holding the only slot.
    // Its terminal completion releases occupancy even when this generation is gone.
    drop(store);
    free_keys(keys);
}
// Main/GL-thread entry points retire both decoded and resident stale artwork.
fn free_keys(keys: impl IntoIterator<Item = PosterKey>) {
    for key in keys {
        tex::free(key, &mut super::GfxUploader);
    }
}
fn retire_revoked() {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = Vec::new();
    store.slots.retain(|slot| {
        if slot.input.live() {
            true
        } else {
            keys.push(slot.key);
            false
        }
    });
    drop(store);
    free_keys(keys);
}
fn request(url: &str, w: i32, h: i32) -> Option<(String, u32, u32)> {
    if !(1..=1024).contains(&w) || !(1..=1024).contains(&h) {
        return None;
    }
    Some((plx_data::ott::http_url(url)?, w as u32, h as u32))
}
pub(super) fn probe(url: &str, w: i32, h: i32) -> Option<PosterKey> {
    probe_with_start(url, w, h, start)
}

/// Main/GL thread, after the runtime checks the full current LocalSelection.
/// Registers an opaque key only: no file IO, network request or decode starts.
pub(super) fn register_local(
    source: &BoundSource,
    root: Arc<RootHandle>,
    relative_path: &str,
    w: i32,
    h: i32,
) -> Option<PosterKey> {
    if source.kind != SourceKind::Folder
        || !source.available
        || source.generation == 0
        || !root.is_live()
        || !(1..=1024).contains(&w)
        || !(1..=1024).contains(&h)
        || !plx_data::ott::valid_local_relative_path(relative_path)
        || !relative_path
            .rsplit('/')
            .next()?
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && ["jpg", "jpeg", "png"]
                        .iter()
                        .any(|kind| ext.eq_ignore_ascii_case(kind))
            })
    {
        return None;
    }
    register_input(
        Input::Local {
            source: source.clone(),
            root,
            relative_path: relative_path.into(),
        },
        w as u32,
        h as u32,
    )
}

/// The key must already name a local slot. A foreign/forgotten key never creates
/// an HTTP request or reconstructs a path; the UI keeps its normal placeholder.
pub(super) fn probe_local(key: PosterKey) -> Option<PosterKey> {
    probe_registered_with_start(key, true, start)
}

fn start(input: Input, w: u32, h: u32, generation: u64, key: PosterKey) -> bool {
    plx_base::task::spawn_off_frame("ott-art", move |off| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fetch_input(off, &input, w, h, generation, key)
        }))
        .ok()
        .flatten();
        complete(generation, key, result);
    })
}

// The start seam keeps admission and terminal publication identical for a real worker
// and a held completion. It does not own another queue or release the occupied slot.
fn probe_with_start(
    url: &str,
    w: i32,
    h: i32,
    start: impl FnOnce(Input, u32, u32, u64, PosterKey) -> bool,
) -> Option<PosterKey> {
    let decline = plx_ui::card_motion::declines_request();
    let (url, w, h) = request(url, w, h)?;
    {
        let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let known = store.slots.iter().find(|slot| {
            slot.w == w && slot.h == h && matches!(&slot.input, Input::Http(known) if known == &url)
        });
        // Motion admission limits new work, not resident artwork. A refreshed
        // snapshot or a sparse frame can make placement Unknown even when the
        // exact image is ready; keep that hit visible and touch its usual LRU.
        if decline && !known.is_some_and(|slot| matches!(slot.state, State::Ready)) {
            return None;
        }
        if store.busy && known.is_none() {
            return None;
        }
    }
    let key = register_input(Input::Http(url), w, h)?;
    probe_registered_with_start(key, false, start)
}

fn register_input(input: Input, w: u32, h: u32) -> Option<PosterKey> {
    retire_revoked();
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.clock = store.clock.wrapping_add(1);
    let clock = store.clock;
    let frame = store.frame;
    let visible = matches!(&input, Input::Http(_));
    if let Some(slot) = store
        .slots
        .iter_mut()
        .find(|s| s.input.same(&input) && s.w == w && s.h == h)
    {
        slot.touched = clock;
        if visible { slot.frame = Some(frame); }
        return Some(slot.key);
    }
    if !input.live() {
        return None;
    }
    let victim = if store.slots.len() < CAP {
        None
    } else {
        // HTTP misses defer rather than tear down visible textures. Local
        // registration is one-shot metadata admission: preserve its existing
        // LRU fallback, but never evict the sole in-flight worker's identity.
        Some(store.slots.iter().enumerate()
            .filter(|(_, slot)| !matches!(slot.state, State::Loading)
                && (!visible || slot.frame.is_none_or(|used| frame.wrapping_sub(used) > 1)))
            .min_by_key(|(_, slot)| slot.touched)?.0)
    };
    let next = store.next.checked_add(1)?;
    let key = PosterKey(store.next);
    store.next = next;
    let new = Slot {
        input,
        w,
        h,
        key,
        touched: clock,
        // Give one-shot local registration a bounded grace until its first draw.
        // Re-registering metadata does not refresh it; an actual probe does.
        frame: Some(frame),
        state: State::Evicted,
    };
    let mut evicted = None;
    if let Some(i) = victim {
        evicted = Some(store.slots[i].key);
        store.slots[i] = new;
    } else {
        store.slots.push(new);
    }
    drop(store);
    free_keys(evicted);
    Some(key)
}

fn probe_registered_with_start(
    key: PosterKey,
    local_only: bool,
    start: impl FnOnce(Input, u32, u32, u64, PosterKey) -> bool,
) -> Option<PosterKey> {
    // Check revocation even if motion would defer new work or a texture is Ready.
    retire_revoked();
    let now = crate::app::clock::now();
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let index = store
        .slots
        .iter()
        .position(|s| s.key == key && (!local_only || matches!(s.input, Input::Local { .. })))?;
    if !store.slots[index].input.live() {
        return None;
    }
    store.clock = store.clock.wrapping_add(1);
    let clock = store.clock;
    store.slots[index].touched = clock;
    let frame = store.frame;
    store.slots[index].frame = Some(frame);
    match &mut store.slots[index].state {
        State::Ready => return Some(key),
        State::Loading | State::Decoded(_) => return None,
        State::Failed(at) => {
            if !retry_ready(at, now) {
                return None;
            }
        }
        _ => {}
    }
    let waiting: usize = store
        .slots
        .iter()
        .map(|s| match &s.state {
            State::Decoded(d) => d.rgba.len(),
            _ => 0,
        })
        .sum();
    if plx_ui::card_motion::declines_request()
        || store.busy
        || waiting.saturating_add(tex::pending_bytes()) >= MAX_BYTES
    {
        return None;
    }
    let key = store.slots[index].key;
    let input = store.slots[index].input.clone();
    let (w, h) = (store.slots[index].w, store.slots[index].h);
    store.slots[index].state = State::Loading;
    store.busy = true;
    let generation = store.generation;
    drop(store);
    if !start(input, w, h, generation, key) {
        complete(generation, key, None);
    }
    None
}

fn job_current(input: &Input, generation: u64, key: PosterKey) -> bool {
    if !input.live() {
        return false;
    }
    let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.generation == generation
        && store.slots.iter().any(|slot| {
            slot.key == key
                && slot.input.same(input)
                && slot.input.live()
                && matches!(slot.state, State::Loading)
        })
}

fn fetch_input(
    off: &OffFrame,
    input: &Input,
    w: u32,
    h: u32,
    generation: u64,
    key: PosterKey,
) -> Option<Decoded> {
    let mut current = || job_current(input, generation, key);
    if !current() {
        return None;
    }
    let decoded = match input {
        Input::Http(url) => fetch(url, w, h),
        Input::Local {
            root,
            relative_path,
            ..
        } => {
            let file = root.file_if_live(off)?;
            let bytes = plx_platform::storage::readonly::read_bounded(
                file,
                relative_path,
                MAX_BYTES,
                &mut current,
            )
            .ok()?;
            if !current() {
                return None;
            }
            thumbnail(&bytes, w, h)
        }
    };
    if current() {
        decoded
    } else {
        None
    }
}

fn complete(generation: u64, key: PosterKey, result: Option<Decoded>) {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    // There is only one occupied worker slot. Reset cannot admit its successor,
    // so even an obsolete completion must release it before a new probe can start.
    store.busy = false;
    if store.generation == generation {
        if let Some(slot) = store.slots.iter_mut().find(|s| s.key == key) {
            slot.state = match result.filter(|_| slot.input.live()) {
                Some(image) => State::Decoded(image),
                None => State::Failed(None),
            };
        }
    }
    drop(store);
    plx_machine::idle::invalidate();
}
// A worker only publishes failure. The frame that observes it starts the
// cooldown on the application's replayable clock; workers never read SDL time.
fn retry_ready(at: &mut Option<u32>, now: u32) -> bool {
    match *at {
        Some(start) => now.wrapping_sub(start) >= 30_000,
        None => {
            *at = Some(now);
            false
        }
    }
}
fn fetch(url: &str, w: u32, h: u32) -> Option<Decoded> {
    let response = plx_net::net::request(
        url,
        &[],
        "GET",
        None,
        plx_net::net::Timeouts {
            connect_s: 5,
            total_s: 15,
            total_ms: 0,
            low_speed_bps: 1,
            low_speed_s: 5,
        },
        true,
        Some(MAX_BYTES),
        None,
    )?;
    if !response.ok() {
        return None;
    }
    thumbnail(&response.body, w, h)
}
fn thumbnail(bytes: &[u8], w: u32, h: u32) -> Option<Decoded> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    // The existing decoder enforces dimension/allocation limits before decoding.
    let (width, height, rgba) = plx_gfx::img::img_decode_owned(bytes)?;
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    let scale = (w as f64 / width as f64)
        .min(h as f64 / height as f64)
        .min(1.0);
    let width = (width as f64 * scale).round().max(1.0) as u32;
    let height = (height as f64 * scale).round().max(1.0) as u32;
    let thumb =
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
    Some(Decoded {
        w: width as u16,
        h: height as u16,
        rgba: thumb.into_raw().into_boxed_slice(),
    })
}
pub(super) fn drain() {
    retire_revoked();
    loop {
        let result = {
            let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
            let Some(slot) = store
                .slots
                .iter_mut()
                .find(|s| matches!(s.state, State::Decoded(_)))
            else {
                break;
            };
            let State::Decoded(image) = std::mem::replace(&mut slot.state, State::Ready) else {
                unreachable!()
            };
            (
                slot.input.clone(),
                PosterReady {
                    key: slot.key,
                    result: Ok::<_, PosterError>(image),
                },
            )
        };
        let (input, result) = result;
        if input.live() {
            let key = result.key;
            tex::accept(result);
            if !input.live() {
                free_keys([key]);
            }
        }
    }
}
pub(super) fn unresident(key: PosterKey) {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = store.slots.iter_mut().find(|s| s.key == key) {
        slot.state = State::Failed(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ott_local_root::LocalSource;
    use crate::app::ott_local_session::AuthorityRef;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    struct LocalFixture {
        path: PathBuf,
        source: LocalSource,
    }
    impl LocalFixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "poster-local-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let path = fs::canonicalize(path).unwrap();
            let source = LocalSource::acquire_authorized(
                &OffFrame::for_test(),
                BoundSource {
                    id: "local-one".into(),
                    name: "Videos".into(),
                    kind: SourceKind::Folder,
                    available: true,
                    authority_ref: AuthorityRef(1),
                    generation: 1,
                    canonical_path: Some(path.to_str().unwrap().into()),
                },
                &mut || true,
            )
            .unwrap();
            Self { path, source }
        }
        fn register(&self, path: &str) -> PosterKey {
            register_local(
                self.source.descriptor(),
                self.source.root_handle().unwrap().clone(),
                path,
                12,
                12,
            )
            .expect("local key")
        }
        fn png(&self, path: &str) {
            let image = image::RgbaImage::from_pixel(32, 64, image::Rgba([50, 60, 70, 255]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            fs::write(self.path.join(path), bytes.into_inner()).unwrap();
        }
    }
    impl Drop for LocalFixture {
        fn drop(&mut self) {
            self.source.withdraw();
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn hold_local(key: PosterKey) -> Option<(Input, HeldCompletion)> {
        let mut held = None;
        let found = probe_registered_with_start(key, true, |input, _, _, generation, key| {
            assert!(matches!(input, Input::Local { .. }));
            held = Some((input, HeldCompletion(Some((generation, key)))));
            true
        });
        assert!(found.is_none());
        held
    }
    // A started worker whose terminal callback is held by the test. Dropping it
    // also settles occupancy if an assertion fails, without starting a thread.
    struct HeldCompletion(Option<(u64, PosterKey)>);
    impl HeldCompletion {
        fn settle(mut self, result: Option<Decoded>) {
            let (generation, key) = self.0.take().expect("unsettled worker");
            complete(generation, key, result);
        }
    }
    impl Drop for HeldCompletion {
        fn drop(&mut self) {
            if let Some((generation, key)) = self.0.take() {
                complete(generation, key, None);
            }
        }
    }
    fn hold_probe(url: &str) -> Option<HeldCompletion> {
        let mut held = None;
        let found = probe_with_start(url, 12, 12, |_, _, _, generation, key| {
            held = Some(HeldCompletion(Some((generation, key))));
            true
        });
        assert!(found.is_none());
        held
    }
    fn one_pixel(value: u8) -> Decoded {
        Decoded {
            w: 1,
            h: 1,
            rgba: vec![value; 4].into_boxed_slice(),
        }
    }

    #[test]
    fn thirty_logos_and_separate_detail_art_load_once_across_repeated_frames() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let mut starts = 0;
        let mut keys = vec![None; 31];
        // One held worker at a time, exactly like production. No network or GL.
        for _ in 0..36 {
            begin_frame();
            let mut completion = None;
            for (row, expected) in keys.iter_mut().enumerate() {
                let url = format!("https://example.invalid/art/{}.png", if row == 30 { 0 } else { row });
                let (w, h) = if row == 30 { (250, 375) } else { (96, 64) };
                let found = probe_with_start(&url, w, h, |_, _, _, generation, key| {
                    starts += 1;
                    assert!(completion.is_none());
                    completion = Some(HeldCompletion(Some((generation, key))));
                    true
                });
                if let Some(key) = found {
                    if let Some(previous) = *expected { assert_eq!(key, previous); }
                    *expected = Some(key);
                }
            }
            if let Some(held) = completion { held.settle(Some(one_pixel(1))); }
            drain();
        }
        assert_eq!(starts, 31, "visible artwork must not repeatedly restart");
        assert!(keys.iter().all(Option::is_some));
        assert_eq!(STORE.lock().unwrap().slots.len(), 31);
        reset();
    }

    #[test]
    fn saturated_current_and_previous_frame_slots_decline_without_freeing() {
        let _serial = plx_base::testlock::serial();
        tex::reset_for_test(MAX_BYTES);
        reset();
        begin_frame();
        let mut keys = Vec::new();
        for n in 0..CAP {
            keys.push(register_input(Input::Http(format!("https://example.invalid/{n}")), 12, 12).unwrap());
        }
        for advance in 0..2 {
            if advance > 0 { begin_frame(); }
            assert!(register_input(Input::Http("https://example.invalid/overflow".into()), 12, 12).is_none());
            assert_eq!(STORE.lock().unwrap().slots.iter().map(|s| s.key).collect::<Vec<_>>(), keys);
        }
        begin_frame();
        // First row is visible again; a new identity may take only a stale row.
        assert_eq!(register_input(Input::Http("https://example.invalid/0".into()), 12, 12), Some(keys[0]));
        assert!(register_input(Input::Http("https://example.invalid/overflow".into()), 12, 12).is_some());
        let store = STORE.lock().unwrap();
        assert_eq!(store.slots.len(), CAP);
        assert!(store.slots.iter().any(|s| s.key == keys[0]));
        assert!(!store.slots.iter().any(|s| s.key == keys[1]));
        drop(store);
        reset();
    }

    #[test]
    fn local_one_shot_registration_survives_http_reprobe_until_first_draw() {
        let _serial = plx_base::testlock::serial();
        tex::reset_for_test(MAX_BYTES);
        reset();
        begin_frame();
        for n in 0..CAP {
            register_input(Input::Http(format!("https://example.invalid/{n}")), 12, 12).unwrap();
        }
        let fixture = LocalFixture::new();
        let local = fixture.register("one-shot.png");
        assert_eq!(STORE.lock().unwrap().slots.len(), CAP);
        let admitted = STORE.lock().unwrap().slots.iter().find(|s| s.key == local).unwrap().frame;
        // The HTTP identity displaced by local registration is immediately
        // requested again by the next draw. It cannot forget the local key.
        assert!(register_input(Input::Http("https://example.invalid/0".into()), 12, 12).is_none());
        assert!(STORE.lock().unwrap().slots.iter().any(|s| s.key == local));
        begin_frame();
        assert_eq!(fixture.register("one-shot.png"), local);
        assert_eq!(STORE.lock().unwrap().slots.iter().find(|s| s.key == local).unwrap().frame, admitted);
        // Another metadata registration can recycle an unprobed local slot.
        for n in 0..CAP { fixture.register(&format!("replacement-{n}.png")); }
        assert!(!STORE.lock().unwrap().slots.iter().any(|s| s.key == local));
        assert!(probe_registered_with_start(local, true, |_, _, _, _, _| panic!("forgotten local key restarted")).is_none());
        assert_eq!(STORE.lock().unwrap().slots.len(), CAP);
        reset();
    }

    #[test]
    fn local_registration_has_no_io_and_reuses_only_the_exact_source_grant() {
        let _serial = plx_base::testlock::serial();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let first = LocalFixture::new();
        let second = LocalFixture::new();
        let key = first.register("missing.png");
        assert_eq!(key, first.register("missing.png"));
        assert!(
            idle(),
            "registration cannot start IO even for a missing file"
        );
        let mut changed = first.source.descriptor().clone();
        changed.generation += 1;
        let generation_key = register_local(
            &changed,
            first.source.root_handle().unwrap().clone(),
            "missing.png",
            12,
            12,
        )
        .unwrap();
        assert_ne!(key, generation_key);
        assert_ne!(
            key,
            second.register("missing.png"),
            "same labels do not equate separately acquired roots"
        );
        assert_eq!(STORE.lock().unwrap().slots.len(), 3);
        reset();
    }

    #[test]
    fn local_registration_rejects_paths_formats_bounds_and_withdrawn_grants() {
        let _serial = plx_base::testlock::serial();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let mut fixture = LocalFixture::new();
        let root = fixture.source.root_handle().unwrap().clone();
        let descriptor = fixture.source.descriptor().clone();
        for path in [
            "../poster.png",
            "/poster.png",
            "a//poster.png",
            "file:///poster.png",
            "https://invalid/poster.png",
            "poster.svg",
            "poster.webp",
            ".png",
        ] {
            assert!(register_local(&descriptor, root.clone(), path, 12, 12).is_none());
        }
        for (w, h) in [(0, 12), (12, -1), (1025, 12)] {
            assert!(register_local(&descriptor, root.clone(), "poster.png", w, h).is_none());
        }
        fixture.source.withdraw();
        assert!(register_local(&descriptor, root, "poster.png", 12, 12).is_none());
        assert!(STORE.lock().unwrap().slots.is_empty());
        reset();
    }

    #[test]
    fn local_probe_cannot_turn_foreign_or_unknown_keys_into_io() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let http = register_input(
            Input::Http("https://example.invalid/poster.png".into()),
            12,
            12,
        )
        .unwrap();
        for key in [PosterKey(1), PosterKey(u32::MAX), http] {
            assert!(
                probe_registered_with_start(key, true, |_, _, _, _, _| panic!(
                    "foreign key started IO"
                ))
                .is_none()
            );
        }
        assert!(idle());
        reset();
    }

    #[test]
    fn local_and_http_demand_share_one_worker_until_real_terminal_completion() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let mut fixture = LocalFixture::new();
        let key = fixture.register("poster.png");
        let (_, old) = hold_local(key).unwrap();
        fixture.source.withdraw();
        assert!(probe_local(key).is_none());
        assert!(
            !idle(),
            "revocation drops publication, not the running worker"
        );
        assert!(hold_probe("https://example.invalid/next.png").is_none());
        old.settle(Some(one_pixel(10)));
        assert!(idle());
        assert!(STORE.lock().unwrap().slots.is_empty());
        let next = hold_probe("https://example.invalid/next.png").unwrap();
        next.settle(None);
        reset();
    }

    #[test]
    fn live_local_bytes_use_the_existing_decoder_and_revocation_drops_completion() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let mut fixture = LocalFixture::new();
        fixture.png("poster.png");
        let key = fixture.register("poster.png");
        let (input, held) = hold_local(key).unwrap();
        let (generation, _) = held.0.unwrap();
        let decoded = fetch_input(&OffFrame::for_test(), &input, 12, 12, generation, key).unwrap();
        assert_eq!(
            (decoded.w, decoded.h, decoded.rgba.len()),
            (6, 12, 6 * 12 * 4)
        );
        fixture.source.withdraw();
        held.settle(Some(decoded));
        drain();
        assert!(STORE.lock().unwrap().slots.is_empty());
        assert_eq!(tex::pending_bytes(), 0);
        assert!(idle());
        reset();
    }

    #[test]
    fn withdrawn_ready_local_art_is_removed_before_a_texture_hit() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let mut fixture = LocalFixture::new();
        let key = fixture.register("poster.png");
        let (_, held) = hold_local(key).unwrap();
        held.settle(Some(one_pixel(1)));
        drain();
        assert_eq!(probe_local(key), Some(key));
        assert!(tex::pending_bytes() > 0);
        fixture.source.withdraw();
        assert!(probe_local(key).is_none());
        assert_eq!(
            tex::pending_bytes(),
            0,
            "revocation frees already accepted pending pixels"
        );
        assert!(STORE.lock().unwrap().slots.is_empty());
        reset();
    }

    #[test]
    fn slot_eviction_never_reconstructs_a_local_key_or_releases_running_work() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let fixture = LocalFixture::new();
        let key = fixture.register("first.png");
        let (input, held) = hold_local(key).unwrap();
        let (generation, _) = held.0.unwrap();
        begin_frame();
        begin_frame();
        for n in 0..CAP {
            fixture.register(&format!("other-{n}.png"));
        }
        assert_eq!(STORE.lock().unwrap().slots.len(), CAP);
        assert!(job_current(&input, generation, key), "in-flight work is not a capacity victim");
        assert!(!idle());
        held.settle(Some(one_pixel(8)));
        begin_frame();
        begin_frame();
        fixture.register("final.png");
        assert!(!job_current(&input, generation, key));
        assert!(probe_registered_with_start(key, true, |_, _, _, _, _| panic!("evicted key restarted")).is_none());
        assert!(idle());
        reset();
    }

    #[test]
    fn a_live_gpu_eviction_uses_the_existing_failure_cooldown() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let fixture = LocalFixture::new();
        let key = fixture.register("poster.png");
        let (_, held) = hold_local(key).unwrap();
        held.settle(Some(one_pixel(3)));
        drain();
        unresident(key);
        assert!(hold_local(key).is_none());
        {
            let mut store = STORE.lock().unwrap();
            let slot = store.slots.iter_mut().find(|s| s.key == key).unwrap();
            let State::Failed(Some(started)) = &mut slot.state else {
                panic!("cooldown not initialized")
            };
            *started = crate::app::clock::now().wrapping_sub(30_000);
        }
        let (_, retry) = hold_local(key).expect("same live source can retry after cooldown");
        retry.settle(None);
        reset();
    }

    #[test]
    fn local_read_refuses_an_outside_symlink_and_an_oversize_body() {
        use std::os::unix::fs::symlink;
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(MAX_BYTES);
        reset();
        let fixture = LocalFixture::new();
        let outside = LocalFixture::new();
        outside.png("outside.png");
        symlink(
            outside.path.join("outside.png"),
            fixture.path.join("link.png"),
        )
        .unwrap();
        fs::File::create(fixture.path.join("large.png"))
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        for path in ["link.png", "large.png"] {
            let key = fixture.register(path);
            let (input, held) = hold_local(key).unwrap();
            let (generation, _) = held.0.unwrap();
            assert!(fetch_input(&OffFrame::for_test(), &input, 12, 12, generation, key).is_none());
            held.settle(None);
        }
        reset();
    }

    #[test]
    fn ready_http_art_survives_unknown_and_deferred_motion_without_starting_work() {
        let _serial = plx_base::testlock::serial();
        let _enforce = plx_ui::card_motion::enforce_for_test();
        let url = "https://example.invalid/ready.jpg";
        for moving in [false, true] {
            tex::reset_for_test(MAX_BYTES);
            reset();
            hold_probe(url)
                .expect("initial request")
                .settle(Some(one_pixel(42)));
            drain();
            let key = {
                let store = STORE.lock().unwrap();
                assert!(matches!(store.slots[0].state, State::Ready));
                store.slots[0].key
            };
            let _scope = if moving {
                plx_ui::card_motion::Scope::moving_for_test()
            } else {
                plx_ui::card_motion::Scope::unknown_for_test()
            };
            assert!(plx_ui::card_motion::declines_request());
            // A new snapshot owns another String for the same image. The
            // source identity is the URL and size, not its borrowed address.
            let refreshed_url = url.to_owned();
            assert_eq!(
                probe_with_start(&refreshed_url, 12, 12, |_, _, _, _, _| {
                    panic!("ready artwork cannot start another worker")
                }),
                Some(key)
            );
            assert!(probe_with_start(
                "https://example.invalid/new.jpg",
                12,
                12,
                |_, _, _, _, _| panic!("motion must still defer new artwork")
            )
            .is_none());
            assert!(probe_with_start(url, 13, 12, |_, _, _, _, _| panic!(
                "another size is a new request"
            ))
            .is_none());
            assert_eq!(
                STORE.lock().unwrap().slots.len(),
                1,
                "declined misses claim no slots"
            );
            unresident(key);
            assert!(probe_with_start(url, 12, 12, |_, _, _, _, _| panic!(
                "motion must not reload evicted artwork"
            ))
            .is_none());
            assert!(idle());
            reset();
        }
    }

    #[test]
    fn reset_keeps_worker_occupied_until_stale_success_then_next_probe_can_start() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(8 * 1024 * 1024);
        reset();
        assert!(idle());
        let old = hold_probe("https://example.invalid/old.jpg").expect("first worker");
        assert!(!idle());
        reset();
        reset();
        assert!(!idle(), "reset cannot make a running worker idle");
        assert!(
            hold_probe("https://example.invalid/new.jpg").is_none(),
            "a next probe must not start a second worker"
        );
        old.settle(Some(one_pixel(10)));
        assert!(idle(), "stale completion releases the single occupied slot");
        assert!(
            STORE.lock().unwrap().slots.is_empty(),
            "stale pixels are never published"
        );

        let next = hold_probe("https://example.invalid/new.jpg").expect("next worker");
        assert!(!idle());
        next.settle(Some(one_pixel(20)));
        assert!(
            !idle(),
            "current pixels still await the ordinary texture drain"
        );
        {
            let store = STORE.lock().unwrap();
            assert_eq!(store.slots.len(), 1);
            let State::Decoded(image) = &store.slots[0].state else {
                panic!("current completion must publish decoded artwork")
            };
            assert_eq!(&*image.rgba, &[20; 4]);
        }
        reset();
        assert!(idle());
    }

    #[test]
    fn stale_failure_releases_occupancy_without_poisoning_the_next_request() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(8 * 1024 * 1024);
        reset();
        let old = hold_probe("https://example.invalid/old.jpg").expect("first worker");
        reset();
        assert!(hold_probe("https://example.invalid/new.jpg").is_none());
        old.settle(None);
        assert!(idle());
        assert!(STORE.lock().unwrap().slots.is_empty());
        let next =
            hold_probe("https://example.invalid/new.jpg").expect("failure did not strand slot");
        next.settle(None);
        assert!(idle());
        {
            let store = STORE.lock().unwrap();
            assert_eq!(store.slots.len(), 1);
            assert!(matches!(store.slots[0].state, State::Failed(None)));
        }
        reset();
    }

    #[test]
    fn refused_start_releases_occupancy_and_only_marks_its_current_generation() {
        let _serial = plx_base::testlock::serial();
        let _motion = plx_ui::card_motion::default_for_test();
        tex::reset_for_test(8 * 1024 * 1024);
        reset();
        let mut called = false;
        assert!(probe_with_start(
            "https://example.invalid/refused.jpg",
            12,
            12,
            |_, _, _, _, _| {
                called = true;
                false
            }
        )
        .is_none());
        assert!(called);
        assert!(idle());
        {
            let store = STORE.lock().unwrap();
            assert_eq!(store.slots.len(), 1);
            assert!(matches!(store.slots[0].state, State::Failed(None)));
        }
        reset();
        assert!(probe_with_start(
            "https://example.invalid/reset-at-start.jpg",
            12,
            12,
            |_, _, _, _, _| {
                reset();
                false
            }
        )
        .is_none());
        assert!(idle());
        assert!(STORE.lock().unwrap().slots.is_empty());
        let next = hold_probe("https://example.invalid/next.jpg").expect("slot can be reused");
        next.settle(None);
        reset();
    }

    #[test]
    fn failed_artwork_retries_on_frame_time_even_across_wrap() {
        let mut at = None;
        let now = u32::MAX - 1000;
        assert!(!retry_ready(&mut at, now));
        assert!(!retry_ready(&mut at, now.wrapping_add(29999)));
        assert!(retry_ready(&mut at, now.wrapping_add(30000)));
    }
    #[test]
    fn external_art_never_accepts_local_files_credentials_or_unbounded_sizes() {
        for url in [
            "file:///etc/passwd",
            "https://user:pass@host.invalid/a",
            "https://host.invalid/a\n",
            "javascript:alert(1)",
        ] {
            assert!(request(url, 250, 375).is_none());
        }
        assert!(request("https://host.invalid/poster.jpg", 250, 375).is_some());
        assert!(request("https://host.invalid/poster.jpg", 4096, 375).is_none());
    }
    #[test]
    fn loopback_http_jpeg_fetch_preserves_scheme_and_isolates_credentials() {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        let _serial = plx_base::testlock::serial();
        assert!(
            plx_net::net::global_init() && plx_net::net::available(),
            "poster transport regression requires host libcurl"
        );
        let color = [32u8, 96, 160];
        let image = image::RgbImage::from_pixel(32, 16, image::Rgb(color));
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = jpeg.into_inner();
        assert!(jpeg.starts_with(&[0xff, 0xd8]) && jpeg.len() < MAX_BYTES);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let caller_headers = [
            "Authorization: Bearer caller-auth-fixture".to_owned(),
            "Cookie: session=caller-cookie-fixture".to_owned(),
            "X-Plex-Token: plex-token-fixture".to_owned(),
            "X-Provider-Key: provider-key-fixture".to_owned(),
            "Referer: https://provider.invalid/private-fixture".to_owned(),
        ];
        std::thread::scope(|scope| {
            let peer = scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut requests = Vec::new();
                // One authenticated control request, then exactly one poster request.
                for _ in 0..2 {
                    let mut socket = loop {
                        match listener.accept() {
                            Ok((socket, _)) => break socket,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "HTTP fixture was not reached");
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            Err(error) => panic!("loopback accept failed: {error}"),
                        }
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut head = Vec::new();
                    let mut byte = [0u8];
                    while !head.ends_with(b"\r\n\r\n") {
                        assert!(head.len() < 8192, "unbounded fixture request");
                        socket.read_exact(&mut byte).unwrap();
                        head.push(byte[0]);
                    }
                    requests.push(String::from_utf8(head).unwrap());
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        jpeg.len()
                    )
                    .unwrap();
                    socket.write_all(&jpeg).unwrap();
                }
                requests
            });
            // A preceding caller's headers must never become ambient poster credentials.
            let control = plx_net::net::request(
                &format!("http://{address}/authenticated-fixture"),
                &caller_headers,
                "GET",
                None,
                plx_net::net::Timeouts {
                    connect_s: 2,
                    total_s: 3,
                    total_ms: 0,
                    low_speed_bps: 1,
                    low_speed_s: 2,
                },
                false,
                Some(MAX_BYTES),
                None,
            )
            .expect("control request");
            assert!(control.ok());
            let url = format!("http://{address}/posters/h/fixture.jpg?shape=wide");
            let (accepted_url, w, h) = request(&url, 12, 12).expect("HTTP artwork accepted");
            assert_eq!(accepted_url, url);
            let decoded = fetch(&accepted_url, w, h).expect("plaintext JPEG fetched and decoded");
            assert_eq!((decoded.w, decoded.h), (12, 6));
            assert_eq!(decoded.rgba.len(), 12 * 6 * 4);
            for pixel in decoded.rgba.chunks_exact(4) {
                assert_eq!(pixel[3], 255);
                for channel in 0..3 {
                    assert!(
                        pixel[channel].abs_diff(color[channel]) <= 3,
                        "JPEG pixel mismatch"
                    );
                }
            }
            let requests = peer.join().unwrap();
            for header in &caller_headers {
                assert!(requests[0].contains(header), "control header was not sent");
            }
            let mut lines = requests[1].split("\r\n");
            assert_eq!(
                lines.next(),
                Some("GET /posters/h/fixture.jpg?shape=wide HTTP/1.1")
            );
            for line in lines.filter(|line| !line.is_empty()) {
                let (name, _) = line.split_once(':').expect("request header");
                assert!(
                    ["host", "accept", "user-agent"].contains(&name.to_ascii_lowercase().as_str()),
                    "poster carried a caller header: {name}"
                );
            }
        });
    }
    #[test]
    fn decoded_thumbnail_retains_aspect_and_has_bounded_storage() {
        let image = image::RgbaImage::from_pixel(32, 64, image::Rgba([50, 60, 70, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let decoded = thumbnail(bytes.get_ref(), 12, 12).unwrap();
        assert_eq!((decoded.w, decoded.h), (6, 12));
        assert_eq!(decoded.rgba.len(), 6 * 12 * 4);
        assert!(owns(PosterKey(FIRST_KEY)));
        assert!(!owns(PosterKey(63)));
    }
}

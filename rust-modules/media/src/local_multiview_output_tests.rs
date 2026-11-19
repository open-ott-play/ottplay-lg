//! Explicit hosted output qualification, not App navigation or LG hardware acceptance.
//! The nine rectangles below are a GPU fixture, not a second production scene layout.
use super::*;
use plx_base::task::MainThread;
use plx_gfx::gfx::{
    self,
    video::{DrawBatch, Matrix, Range, Texture},
    Rect,
};
use plx_platform::local_audio::{OwnerToken, Sink};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

const WIDTH: usize = 384;
const HEIGHT: usize = 216;
extern "C" {
    fn SDL_InitSubSystem(flags: u32) -> c_int;
    fn SDL_QuitSubSystem(flags: u32);
    fn SDL_GL_SetAttribute(attribute: c_int, value: c_int) -> c_int;
    fn SDL_CreateWindow(
        title: *const c_char,
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        flags: u32,
    ) -> *mut c_void;
    fn SDL_DestroyWindow(window: *mut c_void);
    fn SDL_GL_CreateContext(window: *mut c_void) -> *mut c_void;
    fn SDL_GL_DeleteContext(context: *mut c_void);
    fn SDL_GL_MakeCurrent(window: *mut c_void, context: *mut c_void) -> c_int;
    fn SDL_GL_GetCurrentContext() -> *mut c_void;
    fn SDL_GL_GetDrawableSize(window: *mut c_void, width: *mut c_int, height: *mut c_int);
    fn SDL_GetCurrentAudioDriver() -> *const c_char;
    fn glViewport(x: c_int, y: c_int, width: c_int, height: c_int);
    fn glFinish();
    fn glGetIntegerv(name: c_uint, value: *mut c_int);
    fn glActiveTexture(unit: c_uint);
    fn glBindTexture(target: c_uint, id: c_uint);
    fn glGenTextures(count: c_int, ids: *mut c_uint);
    fn glDeleteTextures(count: c_int, ids: *const c_uint);
    fn glGetString(name: c_uint) -> *const u8;
    fn glGetError() -> c_uint;
    fn glReadPixels(
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        format: c_uint,
        ty: c_uint,
        pixels: *mut c_void,
    );
}
struct Context {
    window: *mut c_void,
    context: *mut c_void,
}
impl Context {
    fn open(profile: &str) -> Self {
        unsafe {
            assert_eq!(SDL_InitSubSystem(0x20), 0, "SDL video init failed");
            let attributes = match profile {
                "core" => [(21, 1), (17, 4), (18, 1), (5, 0)],
                "es2" => [(21, 4), (17, 2), (18, 0), (5, 0)],
                _ => panic!("explicit GL profile required"),
            };
            for (attribute, value) in attributes {
                assert_eq!(
                    SDL_GL_SetAttribute(attribute, value),
                    0,
                    "SDL GL attributes rejected"
                );
            }
            let window = SDL_CreateWindow(
                c"OTT hosted virtual output".as_ptr(),
                0,
                0,
                WIDTH as i32,
                HEIGHT as i32,
                0x2 | 0x8,
            );
            if window.is_null() {
                SDL_QuitSubSystem(0x20);
                panic!("SDL hidden window failed");
            }
            let context = SDL_GL_CreateContext(window);
            if context.is_null() {
                SDL_DestroyWindow(window);
                SDL_QuitSubSystem(0x20);
                panic!("actual OpenGL4.1 context failed");
            }
            let this = Self { window, context };
            assert_eq!(SDL_GL_GetCurrentContext(), context);
            let (mut w, mut h) = (0, 0);
            SDL_GL_GetDrawableSize(window, &mut w, &mut h);
            assert_eq!((w, h), (WIDTH as i32, HEIGHT as i32));
            glViewport(0, 0, w, h);
            this
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = SDL_GL_MakeCurrent(self.window, self.context);
            SDL_GL_DeleteContext(self.context);
            SDL_DestroyWindow(self.window);
            SDL_QuitSubSystem(0x20);
        }
    }
}
struct ForeignContext<'a> {
    owner: &'a Context,
    foreign: *mut c_void,
}
impl Drop for ForeignContext<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = SDL_GL_MakeCurrent(self.owner.window, self.owner.context);
            SDL_GL_DeleteContext(self.foreign);
        }
    }
}
fn gl_text(name: u32) -> String {
    unsafe {
        let p = glGetString(name);
        assert!(!p.is_null(), "GL identity missing");
        CStr::from_ptr(p.cast()).to_string_lossy().into_owned()
    }
}
fn clean_gl() {
    assert_eq!(unsafe { glGetError() }, 0, "actual GL error");
}
fn readback() -> Vec<u8> {
    let mut bottom = vec![0u8; WIDTH * HEIGHT * 4];
    unsafe {
        glFinish();
        glReadPixels(
            0,
            0,
            WIDTH as i32,
            HEIGHT as i32,
            0x1908,
            0x1401,
            bottom.as_mut_ptr().cast(),
        );
    }
    clean_gl();
    let mut top = vec![0u8; bottom.len()];
    for y in 0..HEIGHT {
        top[y * WIDTH * 4..(y + 1) * WIDTH * 4]
            .copy_from_slice(&bottom[(HEIGHT - y - 1) * WIDTH * 4..(HEIGHT - y) * WIDTH * 4]);
    }
    top
}
fn rect(index: usize) -> Rect {
    Rect::new(
        (index % 3) as f32 * 640.0 + 50.0,
        (index / 3) as f32 * 360.0,
        540.0,
        360.0,
    )
}
fn upload(texture: &mut Texture, main: &MainThread, frame: &FrameYuv) {
    texture
        .upload(
            main,
            &frame.y,
            &frame.u,
            &frame.v,
            frame.width,
            frame.height,
            frame.chroma_width,
            frame.chroma_height,
            match frame.range {
                ColorRange::Limited => Range::Limited,
                ColorRange::Full => Range::Full,
            },
            match frame.matrix {
                ColorMatrix::Bt601 => Matrix::Bt601,
                ColorMatrix::Bt709 => Matrix::Bt709,
            },
        )
        .expect("actual video upload failed");
}
fn render(textures: &[Texture], main: &MainThread) -> Vec<u8> {
    gfx::frame_clear(0.0, 0.0, 0.0);
    {
        let mut batch = DrawBatch::new(main);
        for (index, texture) in textures.iter().enumerate() {
            assert!(!texture.draw_uv_batched(&mut batch, rect(index), gfx::UV_FULL, 0.0).unwrap());
            assert!(texture.draw_uv_batched(&mut batch, rect(index), gfx::UV_FULL, 1.0).unwrap());
        }
    }
    clean_gl();
    readback()
}
const UNIT0: u32 = 0x84C0;
const TEX2D: u32 = 0x0DE1;

#[derive(Debug, PartialEq)]
struct Bindings {
    program: i32,
    active: i32,
    units: [i32; 4],
}
impl Bindings {
    fn current() -> Self {
        let mut s = Self { program: 0, active: 0, units: [0; 4] };
        unsafe {
            glGetIntegerv(0x8B8D, &mut s.program);
            glGetIntegerv(0x84E0, &mut s.active);
            for (i, id) in s.units.iter_mut().enumerate() {
                glActiveTexture(UNIT0 + i as u32);
                glGetIntegerv(0x8069, id);
            }
            glActiveTexture(s.active as u32);
        }
        s
    }
    fn restore(&self) {
        unsafe {
            for (i, &id) in self.units.iter().enumerate() {
                glActiveTexture(UNIT0 + i as u32);
                glBindTexture(TEX2D, id as u32);
            }
            glActiveTexture(self.active as u32);
        }
        gfx::use_prog(self.program as u32);
    }
}

fn check_scoped_bindings(textures: &[Texture], main: &MainThread, gl: &Context) {
    let original = Bindings::current();
    let mut markers = [0; 4];
    unsafe {
        glGenTextures(4, markers.as_mut_ptr());
        for (i, &id) in markers.iter().enumerate() {
            glActiveTexture(UNIT0 + i as u32);
            glBindTexture(TEX2D, id);
        }
    }
    let marked = Bindings::current();
    assert_eq!(marked.active as u32, UNIT0 + 3);
    assert_eq!(marked.units.map(|id| id as u32), markers);
    assert_ne!(marked.program, 0, "fixture must start with a real UI shader");
    {
        let mut batch = DrawBatch::new(main);
        for (index, texture) in textures.iter().enumerate() {
            assert!(texture.draw_uv_batched(&mut batch, rect(index), gfx::UV_FULL, 1.0).unwrap());
        }
    }
    assert_eq!(Bindings::current(), marked);
    // Early exit while the video shader and all three of its samplers are bound.
    let result = (|| {
        let mut batch = DrawBatch::new(main);
        textures[0].draw_uv_batched(&mut batch, rect(0), gfx::UV_FULL, 1.0)?;
        textures[1].draw_uv_batched(&mut batch, rect(1), gfx::UV_FULL, f32::NAN)
    })();
    assert!(result.is_err());
    assert_eq!(Bindings::current(), marked);
    // A temporarily switched or lost context must never receive the owner's bindings.
    let mut batch = DrawBatch::new(main);
    assert!(textures[0].draw_uv_batched(&mut batch, rect(0), gfx::UV_FULL, 1.0).unwrap());
    unsafe {
        let foreign = SDL_GL_CreateContext(gl.window);
        assert!(!foreign.is_null());
        let custody = ForeignContext { owner: gl, foreign };
        let other = Bindings::current();
        assert!(textures[0].draw_uv_batched(&mut batch, rect(0), gfx::UV_FULL, 1.0).is_err());
        drop(batch);
        assert_eq!(Bindings::current(), other);
        clean_gl();
        drop(custody);
        assert_eq!(SDL_GL_GetCurrentContext(), gl.context);
        original.restore();
        glDeleteTextures(4, markers.as_ptr());
    }
    assert_eq!(Bindings::current(), original);
    clean_gl();
}
fn hash(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(1469598103934665603u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}
fn picture_hash(frame: &FrameYuv) -> u64 {
    hash(frame.y.iter().chain(&frame.u).chain(&frame.v).copied())
}
fn tile_hash(bytes: &[u8], index: usize) -> u64 {
    let (x, y) = ((index % 3) * 128, (index / 3) * 72);
    hash((y..y + 72).flat_map(|y| {
        bytes[(y * WIDTH + x) * 4..(y * WIDTH + x + 128) * 4]
            .iter()
            .copied()
    }))
}
fn bilinear(plane: &[u8], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    ((plane[y0 * w + x0] as f32 * (1.0 - fx) + plane[y0 * w + x1] as f32 * fx) * (1.0 - fy)
        + (plane[y1 * w + x0] as f32 * (1.0 - fx) + plane[y1 * w + x1] as f32 * fx) * fy)
        / 255.0
}
fn reference(frame: &FrameYuv, u: f32, v: f32) -> [u8; 3] {
    let mut y = bilinear(&frame.y, frame.width as usize, frame.height as usize, u, v);
    let mut blue = bilinear(
        &frame.u,
        frame.chroma_width as usize,
        frame.chroma_height as usize,
        u,
        v,
    ) - 128.0 / 255.0;
    let mut red = bilinear(
        &frame.v,
        frame.chroma_width as usize,
        frame.chroma_height as usize,
        u,
        v,
    ) - 128.0 / 255.0;
    if frame.range == ColorRange::Limited {
        y = (y - 16.0 / 255.0) * 255.0 / 219.0;
        blue *= 255.0 / 224.0;
        red *= 255.0 / 224.0;
    }
    let (r, gu, gv, b) = match frame.matrix {
        ColorMatrix::Bt601 => (1.402, -0.344136, -0.714136, 1.772),
        ColorMatrix::Bt709 => (1.5748, -0.187324, -0.468124, 1.8556),
    };
    [y + r * red, y + gu * blue + gv * red, y + b * blue]
        .map(|n| (n.clamp(0.0, 1.0) * 255.0).round() as u8)
}
fn check_reference(frame: &FrameYuv, bytes: &[u8], index: usize) {
    let r = rect(index);
    for local_y in [12usize, 30, 54] {
        for local_x in [24usize, 54, 84] {
            let x = (index % 3) * 128 + 10 + local_x;
            let y = (index / 3) * 72 + local_y;
            let u = ((x as f32 + 0.5) * 5.0 - r.x) / r.w;
            let v = ((y as f32 + 0.5) * 5.0 - r.y) / r.h;
            let expected = reference(frame, u, v);
            let actual = &bytes[(y * WIDTH + x) * 4..(y * WIDTH + x) * 4 + 3];
            for channel in 0..3 {
                assert!((i16::from(actual[channel])-i16::from(expected[channel])).abs()<=8,"slot{index} CPU YUV reference mismatch at{x},{y}: actual{actual:?}, expected{expected:?}");
            }
        }
    }
}
fn next_frames(
    readers: &mut Readers,
    frames: &mut [Option<FrameYuv>],
    audio: &mut Vec<PcmChunk>,
    deadline: Instant,
) {
    while frames.iter().any(Option::is_none) && Instant::now() < deadline {
        for (index, session) in readers.0.iter_mut().enumerate() {
            for _ in 0..16 {
                match session.poll() {
                    Some(Event::Video(frame)) => {
                        if frames[index].is_none() {
                            frames[index] = Some(frame);
                        }
                    }
                    Some(Event::Audio(chunk)) if index == 0 => {
                        if audio.len() < 12 {
                            audio.push(chunk);
                        }
                    }
                    Some(Event::Failure(e)) => panic!("real source{index} failed:{e}"),
                    Some(Event::Ended) => panic!("real source{index} ended early"),
                    Some(_) => {}
                    None => break,
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        frames.iter().all(Option::is_some),
        "nine actual decoded pictures missing"
    );
}
fn queue_real_audio(
    readers: &mut Readers,
    slot: usize,
    sink: &mut Sink,
    main: &MainThread,
    token: OwnerToken,
    seed: &mut Vec<PcmChunk>,
    minimum: usize,
) -> (usize, Vec<i16>) {
    let end = Instant::now() + Duration::from_secs(5);
    let mut samples = 0;
    let mut last = Vec::new();
    let mut pending = None;
    while samples < minimum && Instant::now() < end {
        if pending.is_none() {
            if let Some(chunk) = if seed.is_empty() {
                None
            } else {
                Some(seed.remove(0))
            } {
                pending = Some(chunk);
            } else {
                for _ in 0..24 {
                    match readers.0[slot].poll() {
                        Some(Event::Audio(chunk)) => {
                            pending = Some(chunk);
                            break;
                        }
                        Some(Event::Failure(e)) => panic!("audio source failed:{e}"),
                        Some(Event::Ended) => panic!("audio source ended"),
                        Some(_) => {}
                        None => break,
                    }
                }
            }
        }
        if let Some(chunk) = pending.take() {
            let bytes = (chunk.samples.len() * 2) as u32;
            if sink.queued_bytes(main) + bytes <= plx_platform::local_audio::MAX_QUEUED_BYTES {
                sink.queue(main, token, &chunk.samples)
                    .expect("production SDL queue failed");
                samples += chunk.samples.len();
                last = chunk.samples;
            } else {
                pending = Some(chunk);
            }
        }
        assert!(sink.queued_bytes(main) <= plx_platform::local_audio::MAX_QUEUED_BYTES);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(samples >= minimum, "actual PCM did not reach SDL sink");
    (samples, last)
}
fn drain(sink: &Sink, main: &MainThread) {
    let end = Instant::now() + Duration::from_secs(3);
    while sink.queued_bytes(main) > 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        sink.queued_bytes(main),
        0,
        "virtual SDL output did not drain"
    );
}

fn phase_file(proof: &std::path::Path, name: &str, phase: &str) {
    let target = proof.join(name);
    let temporary = proof.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(&serde_json::json!({"phase":phase})).unwrap(),
    )
    .unwrap();
    std::fs::rename(temporary, target).unwrap();
}
fn wait_phase(
    proof: &std::path::Path,
    name: &str,
    phase: &str,
    measured: bool,
) -> serde_json::Value {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        if let Ok(bytes) = std::fs::read(proof.join(name)) {
            assert!(bytes.len() <= 8192, "phase receipt exceeds bound");
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if value.get("phase").and_then(|v| v.as_str()) == Some(phase) {
                    if measured {
                        assert_eq!(
                            value.get("measured").and_then(|v| v.as_bool()),
                            Some(true),
                            "external monitor did not measure the requested phase"
                        );
                    }
                    return value;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("bounded external phase handshake timed out: {name}/{phase}")
}
fn captured_pcm(readers: &mut Readers, seed: &mut Vec<PcmChunk>, samples: usize) -> Vec<i16> {
    let mut captured = Vec::with_capacity(samples);
    let end = Instant::now() + Duration::from_secs(4);
    while captured.len() < samples && Instant::now() < end {
        let chunk = if !seed.is_empty() {
            Some(seed.remove(0))
        } else {
            match readers.0[0].poll() {
                Some(Event::Audio(chunk)) => Some(chunk),
                Some(Event::Failure(error)) => panic!("real PCM capture failed:{error}"),
                Some(Event::Ended) => panic!("real PCM source ended before capture"),
                Some(_) | None => None,
            }
        };
        if let Some(chunk) = chunk {
            let n = (samples - captured.len()).min(chunk.samples.len());
            captured.extend_from_slice(&chunk.samples[..n]);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(
        captured.len(),
        samples,
        "real decoded PCM capture incomplete"
    );
    assert!(captured.iter().any(|n| *n != 0));
    captured
}
fn queue_captured(sink: &mut Sink, main: &MainThread, token: OwnerToken, pcm: &[i16]) {
    let end = Instant::now() + Duration::from_secs(4);
    for chunk in pcm.chunks(2048) {
        let bytes = (chunk.len() * 2) as u32;
        while sink.queued_bytes(main) + bytes > plx_platform::local_audio::MAX_QUEUED_BYTES {
            assert!(
                Instant::now() < end,
                "bounded gain phase SDL backpressure timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        sink.queue(main, token, chunk)
            .expect("actual gain phase SDL queue failed");
    }
    drain(sink, main);
}
fn measured_gain_phases(
    proof: &std::path::Path,
    sink: &mut Sink,
    main: &MainThread,
    token: OwnerToken,
    pcm: &[i16],
) -> Vec<serde_json::Value> {
    assert_eq!(
        pcm.len(),
        144_000,
        "each measured phase requires exactly1.5seconds stereo48k"
    );
    let fingerprint = hash(pcm.iter().flat_map(|n| n.to_le_bytes()));
    let mut measurements = Vec::new();
    for (phase, gain) in [("full", 100), ("half", 50), ("mute", 0), ("unmute", 100)] {
        sink.set_gain(main, token, gain)
            .expect("owner-bound gain dispatch failed");
        assert_eq!(
            sink.queued_bytes(main),
            0,
            "old-gain queued PCM not cleared"
        );
        phase_file(proof, "phase-request.json", phase);
        wait_phase(proof, "phase-go.json", phase, false);
        queue_captured(sink, main, token, pcm);
        // SDL cannot retract bytes already submitted to the audio server. The
        // external monitor retains raw PCM and measures its declared steady
        // window (first0.5s and final0.1s excluded), not instantaneous silence.
        std::thread::sleep(Duration::from_millis(150));
        phase_file(proof, "phase-done.json", phase);
        let receipt = wait_phase(proof, "phase-ack.json", phase, true);
        assert_eq!(
            hash(pcm.iter().flat_map(|n| n.to_le_bytes())),
            fingerprint,
            "production gain mutated source PCM"
        );
        measurements.push(receipt);
    }
    measurements
}

#[test]
#[ignore = "Explicit Mesa/Xvfb + virtual PulseAudio output qualification; no TV/hardware claim"]
fn nine_real_yuv_textures_and_virtual_sdl_audio() {
    assert!(
        cfg!(target_os = "linux"),
        "hosted Linux-only virtual output qualification"
    );
    let _serial = plx_base::testlock::serial();
    let proof = PathBuf::from(
        std::env::var_os("PLXNATIVE_LOCAL_VIDEO_PROOF_DIR")
            .expect("proof output directory required"),
    );
    std::fs::create_dir_all(&proof).unwrap();
    let root = PathBuf::from(
        std::env::var_os("PLXNATIVE_LOCAL_VIDEO_FIXTURES")
            .expect("real fixture directory required"),
    );
    assert!(std::env::var_os("PLXNATIVE_APP_DIR").is_some());
    let profile = std::env::var("PLXNATIVE_LOCAL_VIDEO_GL_PROFILE")
        .expect("explicit core/es2 profile required");
    let gl = Context::open(&profile);
    // Only the thread which initialized SDL and owns this actual current context mints it.
    assert_eq!(unsafe { SDL_GL_GetCurrentContext() }, gl.context);
    let main = unsafe { MainThread::assume() };
    gfx::init_gl();
    clean_gl();
    let identity = serde_json::json!({"vendor":gl_text(0x1F00),"renderer":gl_text(0x1F01),"version":gl_text(0x1F02),"glsl":gl_text(0x8B8C)});
    assert_eq!(
        identity["glsl"].as_str().unwrap().starts_with("OpenGL ES"),
        profile == "es2",
        "actual context differs from requested profile"
    );
    let texture_limits = Texture::probe(&main).expect("actual planar GPU capability unavailable");
    assert!(plx_net::net::global_init());
    crate::ff::boot();
    assert!(available());
    let mut server = FixtureServer::open(root.clone());
    let (mut readers, _latches) = open_mixed_readers(&root, &server);
    readers.0[0].set_audio(true);
    let mut raw_frames: Vec<Option<FrameYuv>> = (0..9).map(|_| None).collect();
    let mut audio = Vec::new();
    next_frames(
        &mut readers,
        &mut raw_frames,
        &mut audio,
        Instant::now() + Duration::from_secs(6),
    );
    let frames: Vec<FrameYuv> = raw_frames.into_iter().map(Option::unwrap).collect();
    let before_textures = gfx::tex_ledger::totals();
    let mut textures = Vec::new();
    for frame in &frames {
        let mut texture = Texture::new_shared(&main, textures.first()).unwrap();
        upload(&mut texture, &main, frame);
        assert_eq!(texture.dimensions(), Some((96, 64)));
        textures.push(texture);
    }
    let allocated = gfx::tex_ledger::totals();
    assert_eq!(allocated.0, before_textures.0 + 27);
    assert_eq!(allocated.1, before_textures.1 + 9 * (96 * 64 + 2 * 48 * 32));
    let baseline = render(&textures, &main);
    let hashes: Vec<_> = (0..9).map(|i| tile_hash(&baseline, i)).collect();
    assert_eq!(
        hashes.iter().collect::<HashSet<_>>().len(),
        9,
        "nine output tiles are not distinct"
    );
    for (index, frame) in frames.iter().enumerate() {
        check_reference(frame, &baseline, index);
        let (x, y) = ((index % 3) * 128, (index / 3) * 72);
        assert!(
            (y..y + 72).any(
                |row| baseline[(row * WIDTH + x) * 4..(row * WIDTH + x + 128) * 4]
                    .chunks_exact(4)
                    .any(|pixel| pixel[..3] != [0, 0, 0])
            ),
            "slot{index} framebuffer is black"
        );
    }
    for session in readers.0.iter_mut().skip(1) {
        session.set_paused(true);
    }
    let old = picture_hash(&frames[0]);
    let end = Instant::now() + Duration::from_secs(3);
    let mut updated = None;
    while updated.is_none() && Instant::now() < end {
        match readers.0[0].poll() {
            Some(Event::Video(frame)) if picture_hash(&frame) != old => updated = Some(frame),
            Some(Event::Audio(chunk)) => {
                if audio.len() < 12 {
                    audio.push(chunk);
                }
            }
            Some(Event::Failure(e)) => panic!("update source failed:{e}"),
            Some(_) | None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let updated = updated.expect("actual moving source did not produce a new picture");
    upload(&mut textures[0], &main, &updated);
    let changed = render(&textures, &main);
    assert_ne!(tile_hash(&changed, 0), hashes[0]);
    for (i, expected) in hashes.iter().enumerate().skip(1) {
        assert_eq!(
            tile_hash(&changed, i),
            *expected,
            "updating0 corrupted texture{i}"
        );
    }
    check_reference(&updated, &changed, 0);
    assert!(textures[8]
        .upload(
            &main,
            &frames[8].y[..frames[8].y.len() - 1],
            &frames[8].u,
            &frames[8].v,
            96,
            64,
            48,
            32,
            Range::Limited,
            Matrix::Bt601
        )
        .is_err());
    assert_eq!(
        render(&textures, &main),
        changed,
        "rejected plane changed an admitted picture"
    );
    let black = [0.0, 0.0, 0.0, 1.0];
    gfx::draw_rect(0.0, 0.0, 10.0, 10.0, 0.0, 0.0, black.as_ptr(), black.as_ptr(), 0.0, 1.0);
    check_scoped_bindings(&textures, &main, &gl);
    assert_eq!(
        render(&textures, &main),
        changed,
        "foreign context disturbed original texture ownership"
    );
    let green = [0.0, 1.0, 0.0, 1.0];
    gfx::draw_rect(
        0.0,
        0.0,
        50.0,
        50.0,
        0.0,
        0.0,
        green.as_ptr(),
        green.as_ptr(),
        0.0,
        1.0,
    );
    let ui = readback();
    let p = &ui[(5 * WIDTH + 5) * 4..(5 * WIDTH + 5) * 4 + 3];
    assert!(
        p[0] <= 3 && p[1] >= 252 && p[2] <= 3,
        "ordinary UI aftervideo used wrong GL state:{p:?}"
    );
    let mut sink = Sink::open(&main).expect("actual virtual PulseAudio SDL sink unavailable");
    let driver = unsafe {
        let p = SDL_GetCurrentAudioDriver();
        assert!(!p.is_null());
        CStr::from_ptr(p).to_string_lossy().into_owned()
    };
    assert_eq!(
        driver, "pulseaudio",
        "dummy/disk/unexpected SDL backend cannot qualify this test"
    );
    let first = OwnerToken {
        scene: 1,
        slot: 0,
        generation: 1,
    };
    sink.set_owner(&main, Some(first)).unwrap();
    let real_pcm = captured_pcm(&mut readers, &mut audio, 144_000);
    readers.0[0].set_paused(true);
    let gain_measurements = measured_gain_phases(&proof, &mut sink, &main, first, &real_pcm);
    let count0 = real_pcm.len() * 4;
    let last0 = &real_pcm[..2048];
    readers.0[0].set_audio(false);
    readers.0[1].set_paused(false);
    readers.0[1].set_audio(true);
    let second = OwnerToken {
        scene: 1,
        slot: 1,
        generation: 2,
    };
    sink.set_owner(&main, Some(second)).unwrap();
    assert_eq!(sink.queued_bytes(&main), 0);
    assert!(sink.queue(&main, first, last0).is_err());
    assert!(
        sink.set_gain(&main, first, 0).is_err(),
        "stale audio owner changed gain"
    );
    let (count1, last1) = queue_real_audio(
        &mut readers,
        1,
        &mut sink,
        &main,
        second,
        &mut Vec::new(),
        4096,
    );
    drain(&sink, &main);
    sink.clear(&main);
    assert_eq!(sink.queued_bytes(&main), 0);
    sink.set_owner(&main, None).unwrap();
    assert!(sink.queue(&main, second, &last1).is_err());
    drop(sink);
    let mut reopened = Sink::open(&main).expect("retired actual SDL sink did not reopen");
    let third = OwnerToken {
        scene: 2,
        slot: 1,
        generation: 3,
    };
    reopened.set_owner(&main, Some(third)).unwrap();
    reopened.queue(&main, third, &last1).unwrap();
    drain(&reopened, &main);
    drop(reopened);
    for session in &mut readers.0 {
        session.cancel();
    }
    let end = Instant::now() + Duration::from_secs(4);
    while readers.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(readers.0.iter().all(Session::is_retired));
    assert!(server.stop());
    // Replacing the window that first linked the program must leave its
    // siblings' shader, uniforms and texture names valid.
    drop(textures.remove(0));
    {
        let mut batch = DrawBatch::new(&main);
        for (index, texture) in textures.iter().enumerate() {
            assert!(texture.draw_uv_batched(&mut batch, rect(index + 1), gfx::UV_FULL, 1.0).unwrap());
        }
    }
    let remaining = readback();
    for (index, expected) in hashes.iter().enumerate().skip(1) {
        assert_eq!(tile_hash(&remaining, index), *expected);
    }
    drop(textures);
    assert_eq!(gfx::tex_ledger::totals(), before_textures);
    clean_gl();
    std::fs::write(proof.join("output.rgba"), &baseline).unwrap();
    std::fs::write(proof.join("output-updated.rgba"), &changed).unwrap();
    std::fs::write(proof.join("output-ui.rgba"), &ui).unwrap();
    let metadata = serde_json::json!({"schema":1,"scope":"hosted software OpenGL+virtual PulseAudio production Texture/Sink primitives; no App navigation, shared scene geometry, TV GPU or audible hardware claim","width":WIDTH,"height":HEIGHT,"pixel_format":"RGBA8-top-down","gl":identity,"gl_profile":profile,"planar_format":"YUV420P8 planar three planes","texture_format":if profile=="es2"{"GL_LUMINANCE"}else{"GL_RED"},"max_texture_dimension":texture_limits.max_dimension,"texture_units":texture_limits.texture_units,"decoded_formats":frames.iter().map(|f|serde_json::json!({"width":f.width,"height":f.height,"chroma_width":f.chroma_width,"chroma_height":f.chroma_height,"strides":[f.width,f.chroma_width,f.chroma_width],"range":if f.range==ColorRange::Limited{"limited"}else{"full"},"matrix":if f.matrix==ColorMatrix::Bt601{"BT601"}else{"BT709"},"pts_us":f.pts_us})).collect::<Vec<_>>(),"sdl_audio_driver":driver,"pcm_format":"S16 native-endian stereo48000","pcm_samples_queued":count0+count1+last1.len(),"gain_measurements":gain_measurements,"gain_phase_samples":real_pcm.len(),"gain_scope":"same real decoded stereo PCM at100/50/0/100; external virtual monitor steady window, not instantaneous hardware silence","checks":{"nine_readback":true,"cpu_yuv_reference":true,"single_region_update":true,"ordinary_ui_after_video":true,"rejected_plane_unchanged":true,"foreign_context_rejected":true,"gain_mute_measured":true,"stale_gain_owner":true,"stale_audio_owner":true,"clear":true,"drain":true,"reopen":true,"cancel_retired":true,"texture_ledger_retired":true,"gl_errors_zero":true}});
    std::fs::write(
        proof.join("OUTPUT.json"),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();
    eprintln!(
        "qualified actual hosted output: nine decoded YUV textures, {} PCM samples, GL={}, SDL={}",
        count0 + count1 + last1.len(),
        metadata["gl"]["renderer"],
        driver
    );
}

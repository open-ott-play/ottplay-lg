//! Explicit real bundled-decoder qualification. Run alone with --ignored --exact.

use super::*;
use crate::ff::{ExternalSource, HeldLocalFile};
use std::{
    collections::HashSet,
    fs::File,
    path::PathBuf,
    time::{Duration, Instant},
};
struct Readers(Vec<Session>);
impl Drop for Readers {
    fn drop(&mut self) {
        for s in &mut self.0 {
            s.cancel();
        }
        let end = Instant::now() + Duration::from_secs(4);
        while self.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
#[derive(Default)]
struct Seen {
    frames: usize,
    last: Option<i64>,
    hash: Option<u64>,
    audio: usize,
    failure: bool,
}
fn pump(readers: &mut Readers, seen: &mut [Seen], revoked: Option<usize>) {
    for (index, session) in readers.0.iter_mut().enumerate() {
        for _ in 0..24 {
            let Some(event) = session.poll() else {
                break;
            };
            match event {
                Event::Ready => {}
                Event::Video(frame) => {
                    assert_eq!(
                        (
                            frame.width,
                            frame.height,
                            frame.chroma_width,
                            frame.chroma_height
                        ),
                        (96, 64, 48, 32)
                    );
                    assert_eq!(
                        (frame.y.len(), frame.u.len(), frame.v.len()),
                        (96 * 64, 48 * 32, 48 * 32)
                    );
                    if let Some(last) = seen[index].last {
                        assert!(frame.pts_us > last, "slot {index} PTS did not advance");
                    }
                    seen[index].last = Some(frame.pts_us);
                    seen[index].frames += 1;
                    seen[index].hash.get_or_insert_with(|| {
                        frame
                            .y
                            .iter()
                            .chain(&frame.u)
                            .chain(&frame.v)
                            .fold(1469598103934665603u64, |h, b| {
                                (h ^ u64::from(*b)).wrapping_mul(1099511628211)
                            })
                    });
                }
                Event::Audio(chunk) => {
                    assert!(!chunk.samples.is_empty());
                    assert_eq!(chunk.samples.len() % 2, 0);
                    assert!(chunk.samples.len() <= 2048);
                    assert!(chunk.samples.iter().any(|v| *v != 0));
                    seen[index].audio += 1;
                }
                Event::Failure(error) => {
                    assert_eq!(
                        Some(index),
                        revoked,
                        "unexpected slot {index} failure: {error}"
                    );
                    seen[index].failure = true;
                }
                Event::Ended => {
                    panic!("fixture slot {index} ended before qualification finished")
                }
            }
        }
        let mailbox = session.control.mailbox.lock().unwrap();
        assert!(mailbox.audio.len() <= 12);
        if let Some((_, frame)) = &mailbox.video {
            assert!(
                frame.y.len() + frame.u.len() + frame.v.len() <= Limits::default().max_frame_bytes
            );
        }
    }
}
fn until(
    readers: &mut Readers,
    seen: &mut [Seen],
    duration: Duration,
    revoked: Option<usize>,
    condition: impl Fn(&[Seen]) -> bool,
) {
    let end = Instant::now() + duration;
    while !condition(seen) && Instant::now() < end {
        pump(readers, seen, revoked);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(condition(seen), "actual decode condition timed out");
}
#[test]
#[ignore = "Explicit real bundled-decoder qualification; requires generated fixtures and PLXNATIVE_APP_DIR"]
fn nine_independent_real_decoders_video_audio_seek_withdrawal_and_retirement() {
    let _serial = plx_base::testlock::serial();
    let root = PathBuf::from(
        std::env::var_os("PLXNATIVE_LOCAL_VIDEO_FIXTURES").expect("fixture directory required"),
    );
    assert!(
        std::env::var_os("PLXNATIVE_APP_DIR").is_some(),
        "new decoder prefix required"
    );
    assert!(plx_net::net::global_init());
    crate::ff::boot();
    assert!(available(), "bundled H264 decoder unavailable");
    let mut readers = Readers(Vec::new());
    let mut latches = Vec::new();
    let off = plx_base::task::OffFrame::for_test();
    for index in 0..9 {
        let path = root.join(format!("slot{index}.mp4"));
        let latch = Arc::new(AtomicBool::new(false));
        let held = HeldLocalFile::from_authorized(
            &off,
            File::open(&path).expect("fixture missing"),
            path,
            Arc::clone(&latch),
        )
        .unwrap();
        readers
            .0
            .push(Session::open(ExternalSource::HeldLocal(held), Limits::default()).unwrap());
        latches.push(latch);
    }
    let path = root.join("slot0.mp4");
    let held = HeldLocalFile::from_authorized(
        &off,
        File::open(&path).unwrap(),
        path,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(
        matches!(
            Session::open(ExternalSource::HeldLocal(held), Limits::default()),
            Err(Error::Budget)
        ),
        "live worker bound not enforced"
    );
    readers.0[0].set_audio(true);
    let mut seen: Vec<Seen> = (0..9).map(|_| Seen::default()).collect();
    until(&mut readers, &mut seen, Duration::from_secs(6), None, |s| {
        s.iter().all(|s| s.frames >= 3) && s[0].audio > 0
    });
    assert_eq!(
        seen.iter()
            .map(|s| s.hash.unwrap())
            .collect::<HashSet<_>>()
            .len(),
        9,
        "pictures are not independent"
    );
    let audio_epoch = readers.0[0]
        .control
        .audio_generation
        .load(Ordering::Acquire);
    readers.0[0].set_audio(true);
    assert_eq!(
        readers.0[0]
            .control
            .audio_generation
            .load(Ordering::Acquire),
        audio_epoch
    );
    readers.0[0].set_audio(false);
    readers.0[1].set_audio(true);
    let zero_audio = seen[0].audio;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[1].audio > 0
    });
    assert_eq!(
        seen[0].audio, zero_audio,
        "old owner PCM escaped after switch"
    );
    readers.0[2].set_paused(true);
    let paused = seen[2].frames;
    let other = seen[5].frames;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[5].frames >= other + 3
    });
    assert_eq!(seen[2].frames, paused, "paused source published frames");
    readers.0[2].set_paused(false);
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[2].frames > paused
    });
    readers.0[3].seek(2_000_000).unwrap();
    seen[3].last = None;
    let before_seek = seen[3].frames;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[3].frames > before_seek && s[3].last.is_some_and(|p| p >= 1_900_000)
    });
    latches[4].store(true, Ordering::Release);
    until(
        &mut readers,
        &mut seen,
        Duration::from_secs(2),
        Some(4),
        |s| s[4].failure,
    );
    for s in &mut readers.0 {
        s.cancel();
    }
    let end = Instant::now() + Duration::from_secs(4);
    while readers.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        readers.0.iter().all(Session::is_retired),
        "cancel was incorrectly reported as retirement"
    );
    let path = root.join("slot0.mp4");
    let held = HeldLocalFile::from_authorized(
        &off,
        File::open(&path).unwrap(),
        path,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let mut reopened = Session::open(ExternalSource::HeldLocal(held), Limits::default())
        .expect("retired permits leaked");
    reopened.cancel();
    let end = Instant::now() + Duration::from_secs(3);
    while !reopened.is_retired() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reopened.is_retired());
}

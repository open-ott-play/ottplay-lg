//! Actual independent loopback publishers and bundled TS demux, no provider/device.
use super::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex/artifacts");
        let path = root.join(format!("live-audio-fixture-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap(); Self(path)
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn run(program: &str, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new(program).args(args).output().unwrap();
    assert!(output.status.success(), "{program}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}
#[derive(Clone)]
struct Part { name: String, start_ns: i64, duration_ns: i64 }
fn parts(root: &Path, prefix: &str) -> Vec<Part> {
    let mut paths: Vec<_> = std::fs::read_dir(root).unwrap().map(|p| p.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix)
            && p.extension().is_some_and(|ext| ext == "ts")).collect();
    paths.sort();
    let mut result = Vec::new();
    for path in paths {
        let raw = run("ffprobe", &["-v", "error", "-show_packets", "-show_entries", "packet=pts_time,duration_time",
            "-of", "json", path.to_str().unwrap()]);
        let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let packets = json["packets"].as_array().unwrap();
        let pts = |p: &serde_json::Value| (p["pts_time"].as_str().unwrap().parse::<f64>().unwrap() * 1e9).round() as i64;
        let first = packets.iter().map(pts).min().unwrap();
        let end = packets.iter().map(|p| pts(p) + (p["duration_time"].as_str().unwrap().parse::<f64>().unwrap() * 1e9).round() as i64).max().unwrap();
        result.push(Part { name: path.file_name().unwrap().to_string_lossy().into(), start_ns: first, duration_ns: end-first });
    }
    let origin = result[0].start_ns;
    for part in &mut result { part.start_ns -= origin; }
    result
}
fn playlist(parts: &[Part], first: usize, last: usize, sequence: u64, ended: bool) -> String {
    let mut out = format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-DISCONTINUITY-SEQUENCE:0\n", sequence+first as u64);
    for part in &parts[first..last] {
        out.push_str(&format!("#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:{:02}.{:09}Z\n#EXTINF:{}.{:09},\n{}\n",
            part.start_ns / 1_000_000_000, part.start_ns % 1_000_000_000,
            part.duration_ns / 1_000_000_000, part.duration_ns % 1_000_000_000, part.name));
    }
    if ended { out.push_str("#EXT-X-ENDLIST\n"); } out
}
struct Server {
    url: String, stop: Arc<AtomicBool>, worker: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<std::collections::BTreeMap<String,usize>>>,
}
impl Server {
    fn new(root: PathBuf, video: Vec<Part>, audio: Vec<Part>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/master.m3u8", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false)); let stopped = stop.clone();
        let requests = Arc::new(Mutex::new(std::collections::BTreeMap::<String,usize>::new()));
        let seen = requests.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut stream,_)) = listener.accept() else { std::thread::sleep(Duration::from_millis(2)); continue; };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new(); let mut byte = [0];
                while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 { break; } request.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/");
                let count = { let mut seen = seen.lock().unwrap(); let n = seen.entry(path.into()).or_default(); let count = *n; *n+=1; count };
                let body = match path {
                    "/master.m3u8" => b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100,AUDIO=\"audio\"\nvideo.m3u8\n".to_vec(),
                    "/video.m3u8" if count == 0 => playlist(&video,0,4,100,false).into_bytes(),
                    "/video.m3u8" => playlist(&video,4,video.len(),100,true).into_bytes(),
                    "/audio.m3u8" if count == 0 => playlist(&audio,0,4,700,false).into_bytes(),
                    "/audio.m3u8" => playlist(&audio,3,audio.len(),700,true).into_bytes(),
                    _ => std::fs::read(root.join(path.trim_start_matches('/'))).unwrap_or_default(),
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _ = stream.write_all(response.as_bytes()); let _ = stream.write_all(&body);
            }
        });
        Self { url, stop, worker: Some(worker), requests }
    }
}
impl Drop for Server { fn drop(&mut self) { self.stop.store(true,Ordering::Release); self.worker.take().unwrap().join().unwrap(); } }

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn separate_live_audio_refreshes_independent_windows_and_starts_near_edge_at_zero() {
    let _serial = plx_base::testlock::serial();
    let _ff = crate::ff::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init()); crate::curlio::boot();
    let dir = Scratch::new();
    run("ffmpeg", &["-v","error","-nostdin","-y","-f","lavfi","-i","testsrc2=size=160x90:rate=25",
        "-t","5","-an","-c:v","libx264","-preset","fast","-bf","2","-g","25","-sc_threshold","0",
        "-f","segment","-segment_time","1","-segment_format","mpegts",dir.0.join("v%02d.ts").to_str().unwrap()]);
    run("ffmpeg", &["-v","error","-nostdin","-y","-f","lavfi","-i","sine=frequency=440:sample_rate=48000",
        "-t","5","-c:a","aac","-ar","48000","-ac","1","-f","segment","-segment_time","0.5",
        "-segment_format","mpegts",dir.0.join("a%02d.ts").to_str().unwrap()]);
    let video = parts(&dir.0,"v"); let audio = parts(&dir.0,"a");
    assert_eq!(video.len(),5); assert!(audio.len() >= 10);
    let server = Server::new(dir.0.clone(),video,audio);
    let info = crate::ff::external_hls::inspect(&server.url,Arc::new(AtomicBool::new(false))).unwrap();
    let playback = info.hls_playback.unwrap(); assert!(playback.is_live_renditions());
    SHARED.reset_session(); SHARED.seek_to_ns.store(-1,Ordering::Release);
    let previous_feed = FEED_AUDIO.swap(true,Ordering::Relaxed);
    let mut video_q = crate::aq::aq_new(4*1024*1024); let mut audio_q = crate::aq::aq_new(4*1024*1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let result = demux(&playback,&mut *video_q,&mut *audio_q,&mut *hs);
    let mut stamps = Vec::new();
    for queue in [&mut *video_q,&mut *audio_q] {
        loop { let node = crate::aq::aq_pop(queue,std::ptr::null_mut()); if node.is_null() { break; }
            unsafe { let (es,_,pts,_,_) = crate::aq::au_fields(node); stamps.push((es,pts)); libc::free(node.cast()); }
        }
    }
    plx_net::stream::http_close(&mut *hs); crate::aq::aq_destroy(&mut *video_q); crate::aq::aq_destroy(&mut *audio_q);
    FEED_AUDIO.store(previous_feed,Ordering::Relaxed);
    assert!(result.is_ok(),"{result:?}");
    assert_eq!(stamps.iter().map(|(_,pts)| *pts).min(),Some(0));
    assert_eq!(stamps.iter().filter(|(es,_)| *es==1).count(),100,"near edge skips the first of five video segments");
    let first_video = stamps.iter().filter(|(es,_)| *es==1).map(|(_,pts)| *pts).min().unwrap();
    assert!((0..25_000_000).contains(&first_video),"preserve only the genuine AAC lead: {first_video}");
    assert!(stamps.iter().filter(|(es,_)| *es==2).count() > 150);
    let requests = server.requests.lock().unwrap();
    assert!(requests["/video.m3u8"] >= 2 && requests["/audio.m3u8"] >= 2);
    assert!(playback.live_subtitle_revision().is_some(),"paired feed publishes a committed subtitle clock");
}

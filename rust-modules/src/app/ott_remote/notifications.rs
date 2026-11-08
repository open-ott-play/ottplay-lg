//! Ephemeral, authenticated notification feed. It never owns input or playback.
//! Mirrors the shared web v1 contract; no durable content or display ACKs.
use super::{
    protocol::{Config, Request, Response},
    transport::{self, Outcome, Task},
};
use plx_appkit::notification_banner::{Banner, Renderer, Severity};
use plx_ui::tex::Decoded;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{mpsc::TryRecvError, Arc},
};

const POLL_LIMIT: usize = 32 * 1024;
pub(super) const MEDIA_LIMIT: usize = 224 * 1024;
const JPEG_LIMIT: usize = 160 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MediaDescriptor {
    pub id: String,
    pub width: u16,
    pub height: u16,
    pub bytes: usize,
}
struct Flight {
    task: Task,
    epoch: u64,
    started: u64,
}
struct PendingImage {
    descriptor: MediaDescriptor,
    banner: Banner,
}
struct MediaFlight {
    flight: Flight,
    pending: PendingImage,
}
pub(super) struct LoadedImage {
    pub decoded: Decoded,
    pub remaining_ms: u64,
}

#[derive(Default)]
pub(super) struct State {
    authority: u64,
    epoch: u64,
    session: String,
    enabled: bool,
    active: bool,
    supports_enabled: bool,
    supports_media: bool,
    next_poll: u64,
    poll: Option<Flight>,
    media: Option<MediaFlight>,
    pending_image: Option<PendingImage>,
    seen: VecDeque<String>,
    banners: Vec<Banner>,
    renderer: Renderer,
}
impl State {
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn show_for_test(&mut self, now: u64) {
        self.push(Banner {
            id: "1".repeat(32),
            title: "Home".into(),
            message: "Passive notification".into(),
            severity: Severity::Info,
            started_ms: now,
            expires_ms: now + 5_000,
            offset_px: 0,
            image: None,
        });
    }
    fn retire(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if let Some(flight) = &self.poll {
            flight.task.cancel();
        }
        if let Some(flight) = &self.media {
            flight.flight.task.cancel();
        }
        // Retain canceled worker slots until they settle; repeated sleep/toggles
        // cannot create an unbounded collection of in-flight native requests.
        self.pending_image = None;
        self.banners.clear();
        self.seen.clear();
        self.renderer.clear();
        self.next_poll = 0;
        plx_machine::idle::invalidate();
    }
    pub(super) fn suspend(&mut self) {
        self.retire();
        self.active = false;
        self.session.clear();
    }
    pub(super) fn draw(
        &mut self,
        now: u64,
        osd_opacity: f32,
        reduced_motion: bool,
        measure: &dyn plx_machine::machine::Measure,
    ) {
        self.renderer.draw(
            &self.banners,
            now,
            osd_opacity,
            reduced_motion,
            plx_ui::Painter::root(),
            measure,
        );
    }
    pub(super) fn needs_frame(&self, now: u64) -> bool {
        plx_appkit::notification_banner::needs_frame(&self.banners, now)
    }
    fn push(&mut self, banner: Banner) {
        if banner.image.is_some() {
            self.banners.retain(|item| item.image.is_none());
        }
        if self.banners.len() >= 3 {
            self.banners.remove(0);
        }
        self.banners.push(banner);
        plx_machine::idle::invalidate();
    }
    pub(super) fn tick(
        &mut self,
        config: Option<&Config>,
        authority: u64,
        runtime: &str,
        enabled: bool,
        active: bool,
        now: u64,
    ) {
        self.tick_with(
            config,
            authority,
            runtime,
            enabled,
            active,
            now,
            transport::send_bounded,
            transport::notification_media,
        );
    }
    fn tick_with(
        &mut self,
        config: Option<&Config>,
        authority: u64,
        runtime: &str,
        enabled: bool,
        active: bool,
        now: u64,
        mut send: impl FnMut(Request, usize) -> Option<Task>,
        mut fetch_image: impl FnMut(Request, String, MediaDescriptor) -> Option<Task>,
    ) {
        let active = active && enabled && config.is_some();
        if self.authority != authority || self.enabled != enabled || active != self.active {
            self.retire();
            if self.authority != authority {
                self.supports_enabled = false;
                self.supports_media = false;
            }
            self.authority = authority;
            self.enabled = enabled;
            self.session.clear();
        }
        self.active = active;
        if self.session.is_empty() {
            self.session = format!("n{runtime}-{:x}", self.epoch);
        }
        let count = self.banners.len();
        self.banners
            .retain(|banner| now < banner.expires_ms && active);
        if count != self.banners.len() {
            plx_machine::idle::invalidate();
        }
        if let Some(flight) = self.poll.take() {
            match flight.task.poll() {
                Err(TryRecvError::Empty) => self.poll = Some(flight),
                Ok(Outcome::Http {
                    response: Ok(response),
                    ..
                }) if flight.epoch == self.epoch => {
                    self.receive(response, now, now.saturating_sub(flight.started))
                }
                _ => (),
            }
        }
        if let Some(flight) = self.media.take() {
            match flight.flight.task.poll() {
                Err(TryRecvError::Empty) => self.media = Some(flight),
                result
                    if flight.flight.epoch == self.epoch
                        && active
                        && now < flight.pending.banner.expires_ms =>
                {
                    let mut banner = flight.pending.banner;
                    if let Ok(Outcome::NotificationMedia(Some(image))) = result {
                        let remaining = image
                            .remaining_ms
                            .saturating_sub(now.saturating_sub(flight.flight.started));
                        banner.expires_ms = banner.expires_ms.min(now.saturating_add(remaining));
                        banner.image = Some(Arc::new(image.decoded));
                        banner.started_ms = now;
                    }
                    // The original text remains a bounded fallback when a snapshot
                    // fails. A successful camera banner renders the image alone.
                    if now < banner.expires_ms {
                        self.push(banner);
                    }
                }
                _ => (),
            }
        }
        if let Some(config) = config {
            if active && self.media.is_none() {
                if let Some(pending) = self
                    .pending_image
                    .take()
                    .filter(|p| now < p.banner.expires_ms)
                {
                    let request = Request::new(
                        config,
                        "GET",
                        &format!(
                            "/api/notifications/media?id={}&runtime={}&device_id={}",
                            pending.descriptor.id, self.session, config.device_id
                        ),
                        None,
                    );
                    if let Some(task) =
                        fetch_image(request, self.session.clone(), pending.descriptor.clone())
                    {
                        self.media = Some(MediaFlight {
                            flight: Flight {
                                task,
                                epoch: self.epoch,
                                started: now,
                            },
                            pending,
                        });
                    } else {
                        self.push(pending.banner);
                    }
                }
            }
            if self.poll.is_none() && now >= self.next_poll {
                let mut path = format!(
                    "/api/notifications?runtime={}&active={}&device_id={}",
                    self.session, active, config.device_id
                );
                if self.supports_enabled {
                    path.push_str(&format!("&enabled={enabled}"));
                }
                if self.supports_media {
                    path.push_str("&media=jpeg-v1");
                }
                self.next_poll = now.saturating_add(60_000);
                if let Some(task) = send(Request::new(config, "GET", &path, None), POLL_LIMIT) {
                    self.poll = Some(Flight {
                        task,
                        epoch: self.epoch,
                        started: now,
                    });
                }
            }
        }
    }
    fn receive(&mut self, response: Response, now: u64, elapsed: u64) {
        if matches!(response.status, 400 | 404) && (self.supports_enabled || self.supports_media) {
            self.supports_enabled = false;
            self.supports_media = false;
            self.next_poll = now.saturating_add(5_000);
        }
        if response.status != 200 || response.body.len() > POLL_LIMIT {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&response.body) else {
            return;
        };
        let Some(server_time) = timestamp(&value["server_time"]) else {
            return;
        };
        let Some(delay) = integer(&value["poll_after_ms"], 5_000, 60_000) else {
            return;
        };
        let Some(items) = value["notifications"]
            .as_array()
            .filter(|items| items.len() <= 4)
        else {
            return;
        };
        if value["version"] != 1 {
            return;
        }
        self.next_poll = now.saturating_add(delay);
        self.supports_enabled = value["capabilities"]["enabled"] == true;
        self.supports_media = value["capabilities"]["media"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "jpeg-v1"));
        for item in items {
            let Some((mut banner, descriptor)) =
                item_banner(item, &self.session, server_time, elapsed, now)
            else {
                continue;
            };
            if self.seen.contains(&banner.id) {
                continue;
            }
            self.seen.push_back(banner.id.clone());
            if self.seen.len() > 64 {
                self.seen.pop_front();
            }
            if !self.active || !self.enabled || now >= banner.expires_ms {
                continue;
            }
            // Stable per-new-item offset; never cumulatively shift existing cards.
            banner.offset_px = banner.id.bytes().fold(0u32, |sum, b| {
                sum.wrapping_mul(33).wrapping_add(u32::from(b))
            }) as u8
                % 6;
            if let Some(descriptor) = descriptor.filter(|_| self.supports_media) {
                if let Some(flight) = &self.media {
                    flight.flight.task.cancel();
                }
                // Retire the old image result without retiring the independent text feed.
                if let Some(flight) = &mut self.media {
                    flight.flight.epoch = self.epoch.wrapping_sub(1);
                }
                self.banners.retain(|item| item.image.is_none());
                self.renderer.clear();
                self.pending_image = Some(PendingImage { descriptor, banner });
            } else {
                self.push(banner);
            }
        }
    }
}
fn integer(value: &Value, min: u64, max: u64) -> Option<u64> {
    value.as_u64().filter(|v| (min..=max).contains(v))
}
fn timestamp(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v > 0.0)
}
fn hex_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(value: &Value, min: usize, max: usize) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.contains('\0') && (min..=max).contains(&s.encode_utf16().count()))
        .map(str::to_owned)
}
fn remaining(expires: &Value, server: f64, elapsed: u64) -> Option<u64> {
    let delta = (timestamp(expires)? - server) * 1000.0;
    Some((delta.max(0.0).min(15_000.0) as u64).saturating_sub(elapsed))
}
fn media_descriptor(value: &Value) -> Option<MediaDescriptor> {
    if value.get("url").is_some()
        || value["kind"] != "image"
        || value["content_type"] != "image/jpeg"
    {
        return None;
    }
    Some(MediaDescriptor {
        id: value["id"].as_str().filter(|s| hex_id(s))?.into(),
        width: integer(&value["width"], 1, 640)? as u16,
        height: integer(&value["height"], 1, 360)? as u16,
        bytes: integer(&value["bytes"], 1, JPEG_LIMIT as u64)? as usize,
    })
}
fn item_banner(
    item: &Value,
    session: &str,
    server: f64,
    elapsed: u64,
    now: u64,
) -> Option<(Banner, Option<MediaDescriptor>)> {
    if item["runtime"] != session {
        return None;
    }
    let id = item["id"].as_str().filter(|id| hex_id(id))?.to_owned();
    let severity = match item["severity"].as_str()? {
        "info" => Severity::Info,
        "warning" => Severity::Warning,
        "critical" => Severity::Critical,
        _ => return None,
    };
    let duration = integer(&item["duration_ms"], 1_000, 15_000)?;
    let life = duration.min(remaining(&item["expires_at"], server, elapsed)?);
    Some((
        Banner {
            id,
            title: text(&item["title"], 0, 128)?,
            message: text(&item["message"], 1, 512)?,
            severity,
            started_ms: now,
            expires_ms: now.saturating_add(life),
            offset_px: 0,
            image: None,
        },
        media_descriptor(&item["media"]),
    ))
}

/// Decode only the exact descriptor returned by this same authenticated session.
/// JPEG dimensions are checked before the shared decoder can allocate pixels.
pub(super) fn decode_media(
    body: &[u8],
    session: &str,
    descriptor: &MediaDescriptor,
) -> Option<LoadedImage> {
    if body.len() > MEDIA_LIMIT
        || !body
            .iter()
            .all(|b| matches!(b, b'\t' | b'\n' | b'\r' | b' '..=b'~'))
    {
        return None;
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    if value["version"] != 1
        || value["id"] != descriptor.id
        || value["runtime"] != session
        || value["content_type"] != "image/jpeg"
        || value["width"].as_u64()? != u64::from(descriptor.width)
        || value["height"].as_u64()? != u64::from(descriptor.height)
        || value["bytes"].as_u64()? != descriptor.bytes as u64
    {
        return None;
    }
    let remaining_ms = remaining(&value["expires_at"], timestamp(&value["server_time"])?, 0)?;
    if remaining_ms == 0 {
        return None;
    }
    let bytes = decode_base64(value["data_base64"].as_str()?, descriptor.bytes)?;
    if jpeg_dimensions(&bytes)? != (descriptor.width, descriptor.height) {
        return None;
    }
    let (w, h, rgba) = plx_gfx::img::img_decode_owned(&bytes)?;
    if w != u32::from(descriptor.width)
        || h != u32::from(descriptor.height)
        || rgba.len() != usize::from(descriptor.width) * usize::from(descriptor.height) * 4
    {
        return None;
    }
    Some(LoadedImage {
        decoded: Decoded {
            w: w as u16,
            h: h as u16,
            rgba: rgba.into_boxed_slice(),
        },
        remaining_ms,
    })
}
fn decode_base64(text: &str, expected: usize) -> Option<Vec<u8>> {
    if expected > JPEG_LIMIT || text.len() != expected.div_ceil(3) * 4 {
        return None;
    }
    let value = |b| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(expected);
    for (index, chunk) in text.as_bytes().chunks_exact(4).enumerate() {
        let a = value(chunk[0])?;
        let b = value(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        let last = (index + 1) * 4 == text.len();
        if chunk[2] == b'=' {
            if !last || chunk[3] != b'=' || b & 15 != 0 {
                return None;
            }
        } else {
            let c = value(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk[3] == b'=' {
                if !last || c & 3 != 0 {
                    return None;
                }
            } else {
                out.push((c << 6) | value(chunk[3])?);
            }
        }
    }
    (out.len() == expected).then_some(out)
}
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 12
        || bytes.get(..2)? != [0xff, 0xd8]
        || bytes.get(bytes.len() - 2..)? != [0xff, 0xd9]
    {
        return None;
    }
    let mut at = 2;
    let mut found = None;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xff {
            return None;
        }
        while bytes.get(at) == Some(&0xff) {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        if marker == 0xda {
            return found;
        }
        if marker == 0xd9 {
            return None;
        }
        let len = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
        if len < 2 || at.checked_add(len)? > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0 | 0xc2) {
            if found.is_some() || len < 8 || bytes[at + 2] != 8 {
                return None;
            }
            let h = u16::from_be_bytes([bytes[at + 3], bytes[at + 4]]);
            let w = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
            if !(1..=640).contains(&w) || !(1..=360).contains(&h) {
                return None;
            }
            found = Some((w, h));
        } else if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            return None;
        }
        at += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn item(index: u8) -> Value {
        json!({"id":format!("{index:032x}"),"runtime":"n-session",
        "title":"Home","message":"A literal <test> update","severity":"info","duration_ms":5000,"expires_at":106.0})
    }
    fn response(items: Vec<Value>) -> Response {
        Response {
            status: 200,
            body: serde_json::to_vec(&json!({
        "version":1,"server_time":100.0,"poll_after_ms":5000,
        "capabilities":{"enabled":true,"media":["jpeg-v1"]},"notifications":items}))
            .unwrap(),
        }
    }
    fn state() -> State {
        State {
            session: "n-session".into(),
            active: true,
            enabled: true,
            ..Default::default()
        }
    }
    #[cfg(feature = "hostsim")]
    #[derive(Default)]
    struct Io {
        polls: Vec<(Request, std::sync::mpsc::SyncSender<Outcome>)>,
        images: Vec<(
            Request,
            String,
            MediaDescriptor,
            std::sync::mpsc::SyncSender<Outcome>,
        )>,
    }
    #[cfg(feature = "hostsim")]
    impl Io {
        fn tick(
            &mut self,
            state: &mut State,
            config: &Config,
            authority: u64,
            enabled: bool,
            active: bool,
            now: u64,
        ) {
            state.tick_with(
                Some(config),
                authority,
                "native-test",
                enabled,
                active,
                now,
                |request, limit| {
                    assert_eq!(limit, POLL_LIMIT);
                    let (task, sender) = Task::pending_for_test();
                    self.polls.push((request, sender));
                    Some(task)
                },
                |request, session, descriptor| {
                    let (task, sender) = Task::pending_for_test();
                    self.images.push((request, session, descriptor, sender));
                    Some(task)
                },
            );
        }
        fn reply(&self, index: usize, state: &State, mut items: Vec<Value>) {
            for item in &mut items {
                item["runtime"] = json!(state.session);
            }
            self.polls[index]
                .1
                .send(Outcome::Http {
                    response: Ok(response(items)),
                    elapsed_ms: 0,
                })
                .unwrap_or_else(|_| panic!("retained poll receiver"));
        }
    }
    #[cfg(feature = "hostsim")]
    fn config() -> Config {
        Config::parse(
            json!({"enabled":true,"address":"https://controller.invalid/native",
            "token":"a".repeat(32),"device_id":"fixture-device"}),
        )
        .unwrap()
    }
    #[cfg(feature = "hostsim")]
    #[test]
    fn notification_feed_negotiates_and_retires_late_callbacks_without_replay_or_extra_workers() {
        let mut state = State::default();
        let mut io = Io::default();
        let config = config();
        io.tick(&mut state, &config, 1, true, true, 1000);
        let first_session = state.session.clone();
        assert_eq!(io.polls[0].0.url,format!("https://controller.invalid:443/native/api/notifications?runtime={first_session}&active=true&device_id=fixture-device"));
        assert_eq!(io.polls[0].0.token, config.token);
        assert_eq!(io.polls[0].0.method, "GET");
        assert!(io.polls[0].0.body.is_none());
        io.reply(0, &state, vec![item(1)]);
        io.tick(&mut state, &config, 1, true, true, 1500);
        assert_eq!(state.banners.len(), 1);
        assert_eq!(state.next_poll, 6500);
        io.tick(&mut state, &config, 1, true, true, 6500);
        assert!(io.polls[1].0.url.ends_with("&enabled=true&media=jpeg-v1"));
        io.tick(&mut state, &config, 1, false, true, 6501);
        assert!(!state.active && state.banners.is_empty());
        assert_ne!(state.session, first_session);
        let disabled_session = state.session.clone();
        assert_eq!(
            io.polls.len(),
            2,
            "cancel retains its worker slot until it settles"
        );
        io.reply(1, &state, vec![item(2)]);
        io.tick(&mut state, &config, 1, false, true, 6502);
        assert!(state.banners.is_empty());
        assert_eq!(io.polls.len(), 3);
        assert!(io.polls[2].0.url.contains("active=false"));
        assert!(io.polls[2].0.url.ends_with("&enabled=false&media=jpeg-v1"));
        io.tick(&mut state, &config, 1, true, true, 6503);
        assert_ne!(state.session, disabled_session);
        assert_eq!(io.polls.len(), 3);
        io.reply(2, &state, vec![item(3)]);
        io.tick(&mut state, &config, 1, true, true, 6504);
        assert!(state.banners.is_empty());
        assert_eq!(io.polls.len(), 4);
        state.suspend();
        let old_epoch = state.epoch;
        io.reply(3, &state, vec![item(4)]);
        io.tick(&mut state, &config, 2, true, true, 6505);
        assert!(state.epoch > old_epoch && state.banners.is_empty());
        assert_eq!(io.polls.len(), 5);
        assert!(
            !io.polls[4].0.url.contains("enabled="),
            "new authority renegotiates optional capabilities"
        );
    }
    #[cfg(feature = "hostsim")]
    #[test]
    fn notification_camera_replacement_is_bounded_and_failures_use_unexpired_text_only() {
        let mut state = State::default();
        let mut io = Io::default();
        let config = config();
        let camera = |id| {
            let mut item = item(id);
            item["media"] = json!({"id":format!("{id:032x}"),
            "kind":"image","content_type":"image/jpeg","width":3,"height":2,"bytes":776});
            item
        };
        io.tick(&mut state, &config, 1, true, true, 100);
        io.reply(0, &state, vec![camera(1)]);
        io.tick(&mut state, &config, 1, true, true, 200);
        assert_eq!(io.images.len(), 1);
        assert!(state.banners.is_empty());
        assert_eq!(io.images[0].0.token, config.token);
        assert!(io.images[0]
            .0
            .url
            .starts_with("https://controller.invalid:443/native/api/notifications/media?"));
        assert_eq!(io.images[0].1, state.session);
        io.images[0]
            .3
            .send(Outcome::NotificationMedia(Some(LoadedImage {
                decoded: Decoded {
                    w: 3,
                    h: 2,
                    rgba: vec![0; 24].into_boxed_slice(),
                },
                remaining_ms: 4000,
            })))
            .unwrap_or_else(|_| panic!("image receiver"));
        io.tick(&mut state, &config, 1, true, true, 300);
        assert_eq!(state.banners.len(), 1);
        assert_eq!(state.banners[0].expires_ms, 4200);
        let old = Arc::downgrade(state.banners[0].image.as_ref().unwrap());
        let mut replacement = camera(2);
        replacement["runtime"] = json!(state.session);
        state.receive(response(vec![replacement]), 400, 0);
        assert!(
            state.banners.is_empty() && old.upgrade().is_none(),
            "replace old camera rather than show redundant text"
        );
        io.tick(&mut state, &config, 1, true, true, 401);
        assert_eq!(io.images.len(), 2);
        io.images[1]
            .3
            .send(Outcome::NotificationMedia(None))
            .unwrap_or_else(|_| panic!("image receiver"));
        io.tick(&mut state, &config, 1, true, true, 500);
        assert_eq!(state.banners.len(), 1);
        assert!(state.banners[0].image.is_none());
        assert_eq!(state.banners[0].message, "A literal <test> update");
        io.tick(&mut state, &config, 1, true, true, 5400);
        assert!(
            state.banners.is_empty(),
            "image failure does not extend original TTL"
        );
    }
    #[cfg(feature = "hostsim")]
    #[test]
    fn notification_inactive_poll_cannot_become_a_visible_banner_after_wake() {
        let mut state = State::default();
        let mut io = Io::default();
        let config = config();
        io.tick(&mut state, &config, 1, true, false, 100);
        let inactive_session = state.session.clone();
        assert!(io.polls[0].0.url.contains("active=false"));
        let mut inactive_item = item(1);
        inactive_item["runtime"] = json!(inactive_session);
        io.tick(&mut state, &config, 1, true, true, 200);
        assert_ne!(state.session, inactive_session);
        assert_eq!(
            io.polls.len(),
            1,
            "wake preserves the canceled worker bound"
        );
        io.polls[0]
            .1
            .send(Outcome::Http {
                response: Ok(response(vec![inactive_item])),
                elapsed_ms: 0,
            })
            .unwrap_or_else(|_| panic!("retained inactive receiver"));
        io.tick(&mut state, &config, 1, true, true, 300);
        assert!(state.banners.is_empty() && state.seen.is_empty());
        assert_eq!(io.polls.len(), 2);
        assert!(io.polls[1].0.url.contains("active=true"));
    }
    #[test]
    fn exact_session_expiry_dedup_and_three_card_limit_match_web_contract() {
        let mut state = state();
        state.receive(response((0..4).map(item).collect()), 1000, 2000);
        assert_eq!(state.banners.len(), 3);
        assert_eq!(state.banners[0].id, format!("{:032x}", 1));
        assert!(state
            .banners
            .iter()
            .all(|b| b.expires_ms == 5000 && b.offset_px <= 5));
        state.receive(response(vec![item(3)]), 2000, 0);
        assert_eq!(state.banners.len(), 3);
        let mut wrong = item(9);
        wrong["runtime"] = json!("old-session");
        state.receive(response(vec![wrong]), 2000, 0);
        assert_eq!(state.seen.len(), 4);
        state.receive(response(vec![item(8)]), 2000, 6001);
        assert_eq!(state.banners.len(), 3);
        assert!(
            state.seen.contains(&format!("{:032x}", 8)),
            "expired items stay consumed"
        );
        for index in 10..100 {
            state.receive(response(vec![item(index)]), 2000, 0);
        }
        assert_eq!(state.seen.len(), 64);
        assert_eq!(state.banners.len(), 3);
        state.suspend();
        assert!(state.banners.is_empty() && state.seen.is_empty());
        assert!(!state.active && state.session.is_empty());
    }
    #[test]
    fn malformed_text_utf16_limits_and_feed_shape_never_produce_cards() {
        let mut state = state();
        let mut value = item(1);
        value["title"] = json!("😀".repeat(65));
        state.receive(response(vec![value]), 1, 0);
        assert!(state.banners.is_empty());
        let mut value = item(1);
        value["message"] = json!("");
        state.receive(response(vec![value]), 1, 0);
        assert!(state.banners.is_empty());
        state.receive(response((0..5).map(item).collect()), 1, 0);
        assert!(state.banners.is_empty());
        state.active = false;
        state.receive(response(vec![item(1)]), 1, 0);
        assert!(state.banners.is_empty());
        state.active = true;
        state.receive(response(vec![item(1)]), 1, 0);
        assert!(state.banners.is_empty(), "inactive reads are not replayed");
    }
    #[test]
    fn controller_rollback_retires_optional_fields_and_schedules_legacy_retry() {
        let mut state = state();
        state.supports_media = true;
        state.supports_enabled = true;
        state.receive(
            Response {
                status: 400,
                body: vec![],
            },
            1000,
            0,
        );
        assert!(!state.supports_enabled && !state.supports_media);
        assert_eq!(state.next_poll, 6000);
    }
    fn encode(bytes: &[u8]) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for bytes in bytes.chunks(3) {
            let a = bytes[0];
            let b = bytes.get(1).copied().unwrap_or(0);
            let c = bytes.get(2).copied().unwrap_or(0);
            out.push(alphabet[(a >> 2) as usize] as char);
            out.push(alphabet[(((a & 3) << 4) | (b >> 4)) as usize] as char);
            out.push(if bytes.len() > 1 {
                alphabet[(((b & 15) << 2) | (c >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if bytes.len() > 2 {
                alphabet[(c & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }
    #[test]
    fn authenticated_jpeg_descriptor_envelope_header_and_actual_decode_must_all_match() {
        let jpeg = include_bytes!("notification-test.jpg");
        let descriptor = MediaDescriptor {
            id: format!("{:032x}", 42),
            width: 3,
            height: 2,
            bytes: jpeg.len(),
        };
        let mut envelope = json!({"version":1,"id":descriptor.id,"runtime":"n-session","content_type":"image/jpeg",
            "width":3,"height":2,"bytes":jpeg.len(),"data_base64":encode(jpeg),"server_time":100.0,"expires_at":105.0});
        let image = decode_media(
            &serde_json::to_vec(&envelope).unwrap(),
            "n-session",
            &descriptor,
        )
        .unwrap();
        assert_eq!(
            (
                image.decoded.w,
                image.decoded.h,
                image.decoded.rgba.len(),
                image.remaining_ms
            ),
            (3, 2, 24, 5000)
        );
        assert!(decode_media(
            &serde_json::to_vec(&envelope).unwrap(),
            "retired",
            &descriptor
        )
        .is_none());
        envelope["width"] = json!(640);
        assert!(decode_media(
            &serde_json::to_vec(&envelope).unwrap(),
            "n-session",
            &descriptor
        )
        .is_none());
        envelope["width"] = json!(3);
        envelope["data_base64"] = json!("AAAA");
        assert!(decode_media(
            &serde_json::to_vec(&envelope).unwrap(),
            "n-session",
            &descriptor
        )
        .is_none());
        assert_eq!(decode_base64("YQ==", 1), Some(vec![b'a']));
        assert!(decode_base64("YR==", 1).is_none());
        assert!(decode_base64("YQ==AAAA", 6).is_none());
        let oversized = [
            0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 1, 0, 0xff, 0xff, 1, 1, 17, 0,
        ];
        assert!(jpeg_dimensions(&oversized).is_none());
        assert!(
            jpeg_dimensions(&jpeg[..jpeg.len() - 2]).is_none(),
            "final EOI required before decode"
        );
        let frame = jpeg
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .unwrap();
        let mut extended = jpeg.to_vec();
        extended[frame + 1] = 0xc1;
        assert!(
            jpeg_dimensions(&extended).is_none(),
            "only baseline/progressive frame types admitted"
        );
        let length = u16::from_be_bytes([jpeg[frame + 2], jpeg[frame + 3]]) as usize + 2;
        let mut duplicate = jpeg.to_vec();
        duplicate.splice(frame..frame, jpeg[frame..frame + length].iter().copied());
        assert!(
            jpeg_dimensions(&duplicate).is_none(),
            "a later SOF cannot bypass pre-allocation bounds"
        );
        let descriptor = json!({"id":"a".repeat(32),"kind":"image","content_type":"image/jpeg",
            "width":3,"height":2,"bytes":776,"url":"https://untrusted.invalid/image"});
        assert!(
            media_descriptor(&descriptor).is_none(),
            "controller grants never carry a direct URL"
        );
    }
}

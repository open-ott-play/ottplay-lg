//! One queued software-audio output for local multi-view. SDL is resolved from
//! the running process; absent optional audio symbols never prevent app startup.
//! No environment/driver override, native-player state or external process.
use plx_base::{dynlib::Handle, task::MainThread};
use std::{
    borrow::Cow,
    ffi::{c_char, c_void, CStr},
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
const AUDIO: u32 = 0x10;
#[cfg(target_endian = "little")]
const S16_SYS: u16 = 0x8010;
#[cfg(target_endian = "big")]
const S16_SYS: u16 = 0x9010;
pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_QUEUED_BYTES: u32 = 48_000 * 4 * 300 / 1000;
pub const MAX_CHUNK_SAMPLES: usize = 4_800 * 2;
static CLAIMED: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerToken {
    pub scene: u64,
    pub slot: u8,
    pub generation: u64,
}
#[repr(C)]
struct AudioSpec {
    freq: i32,
    format: u16,
    channels: u8,
    silence: u8,
    samples: u16,
    padding: u16,
    size: u32,
    callback: Option<unsafe extern "C" fn(*mut c_void, *mut u8, i32)>,
    userdata: *mut c_void,
}
type Init = unsafe extern "C" fn(u32) -> i32;
type Quit = unsafe extern "C" fn(u32);
type Open = unsafe extern "C" fn(*const c_char, i32, *const AudioSpec, *mut AudioSpec, i32) -> u32;
type Close = unsafe extern "C" fn(u32);
type Pause = unsafe extern "C" fn(u32, i32);
type Queue = unsafe extern "C" fn(u32, *const c_void, u32) -> i32;
type Clear = unsafe extern "C" fn(u32);
type Queued = unsafe extern "C" fn(u32) -> u32;
type Driver = unsafe extern "C" fn() -> *const c_char;
struct Api {
    init: Init,
    quit: Quit,
    open: Open,
    close: Close,
    pause: Pause,
    queue: Queue,
    clear: Clear,
    queued: Queued,
    driver: Driver,
}
impl Api {
    fn load() -> Result<Self, &'static str> {
        let h = Handle::self_handle();
        // Each T below is one explicitly typed SDL C function pointer.
        unsafe fn get<T: Copy>(h: &Handle, n: &str) -> Result<T, &'static str> {
            let p = h
                .sym(n)
                .filter(|p| !p.is_null())
                .ok_or("SDL queued audio API unavailable")?;
            if std::mem::size_of::<T>() != std::mem::size_of_val(&p) {
                return Err("Unsupported SDL audio ABI");
            }
            Ok(std::mem::transmute_copy(&p))
        }
        unsafe {
            Ok(Self {
                init: get(&h, "SDL_InitSubSystem")?,
                quit: get(&h, "SDL_QuitSubSystem")?,
                open: get(&h, "SDL_OpenAudioDevice")?,
                close: get(&h, "SDL_CloseAudioDevice")?,
                pause: get(&h, "SDL_PauseAudioDevice")?,
                queue: get(&h, "SDL_QueueAudio")?,
                clear: get(&h, "SDL_ClearQueuedAudio")?,
                queued: get(&h, "SDL_GetQueuedAudioSize")?,
                driver: get(&h, "SDL_GetCurrentAudioDriver")?,
            })
        }
    }
}
struct Policy {
    owner: Option<OwnerToken>,
    gain: u8,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            owner: None,
            gain: 100,
        }
    }
}
impl Policy {
    fn set_gain(&mut self, owner: OwnerToken, level: u8) -> Result<bool, &'static str> {
        if self.owner != Some(owner) {
            return Err("Local audio owner changed");
        }
        if level > 100 {
            return Err("Invalid local audio gain");
        }
        let changed = self.gain != level;
        self.gain = level;
        Ok(changed)
    }
    fn set(&mut self, owner: Option<OwnerToken>) -> Result<bool, &'static str> {
        if owner.is_some_and(|owner| owner.slot >= 9) {
            return Err("Invalid local audio slot");
        }
        let changed = self.owner != owner;
        self.owner = owner;
        Ok(changed)
    }
    fn admit(&self, token: OwnerToken, samples: usize, queued: u32) -> Result<u32, &'static str> {
        if self.owner != Some(token) {
            return Err("Local audio owner changed");
        }
        if samples == 0 || samples % 2 != 0 || samples > MAX_CHUNK_SAMPLES {
            return Err("Invalid stereo PCM chunk");
        }
        let bytes = u32::try_from(samples)
            .ok()
            .and_then(|n| n.checked_mul(2))
            .ok_or("PCM size overflow")?;
        if queued
            .checked_add(bytes)
            .is_none_or(|n| n > MAX_QUEUED_BYTES)
        {
            return Err("Local audio backpressure");
        }
        Ok(bytes)
    }
}
/// Bounded S16 attenuation. The decoder's shared PCM is always left unchanged.
/// Unity gain borrows the original chunk without allocating or copying it.
fn attenuate(samples: &[i16], level: u8) -> Result<Cow<'_, [i16]>, &'static str> {
    if level > 100 {
        return Err("Invalid local audio gain");
    }
    if samples.is_empty() || samples.len() % 2 != 0 || samples.len() > MAX_CHUNK_SAMPLES {
        return Err("Invalid stereo PCM chunk");
    }
    match level {
        100 => Ok(Cow::Borrowed(samples)),
        0 => Ok(Cow::Owned(vec![0; samples.len()])),
        _ => Ok(Cow::Owned(
            samples
                .iter()
                .map(|sample| (i32::from(*sample) * i32::from(level) / 100) as i16)
                .collect(),
        )),
    }
}
/// !Send/!Sync; only one Sink may own the queued output. Closing releases only
/// this SDL device and the audio-subsystem reference acquired by this instance.
pub struct Sink {
    api: Api,
    device: u32,
    policy: Policy,
    _main: PhantomData<Rc<()>>,
}
impl Sink {
    pub fn open(_main: &MainThread) -> Result<Self, &'static str> {
        if CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Local audio sink already owned");
        }
        let result = (|| {
            let api = Api::load()?;
            if unsafe { (api.init)(AUDIO) } != 0 {
                return Err("SDL audio subsystem unavailable");
            }
            // A dummy/disk driver can test queue plumbing, but cannot establish
            // a real playback sink and is never advertised as audio readiness.
            let driver = unsafe { (api.driver)() };
            let usable = !driver.is_null()
                && !matches!(
                    unsafe { CStr::from_ptr(driver) }.to_bytes(),
                    b"dummy" | b"disk"
                );
            if !usable {
                unsafe { (api.quit)(AUDIO) };
                return Err("No real SDL audio output driver");
            }
            let desired = AudioSpec {
                freq: SAMPLE_RATE as i32,
                format: S16_SYS,
                channels: 2,
                silence: 0,
                samples: 1024,
                padding: 0,
                size: 0,
                callback: None,
                userdata: std::ptr::null_mut(),
            };
            let mut obtained = AudioSpec {
                freq: 0,
                format: 0,
                channels: 0,
                silence: 0,
                samples: 0,
                padding: 0,
                size: 0,
                callback: None,
                userdata: std::ptr::null_mut(),
            };
            let device = unsafe { (api.open)(std::ptr::null(), 0, &desired, &mut obtained, 0) };
            if device == 0 {
                unsafe { (api.quit)(AUDIO) };
                return Err("SDL audio output unavailable");
            }
            if obtained.freq != SAMPLE_RATE as i32
                || obtained.format != S16_SYS
                || obtained.channels != 2
            {
                unsafe {
                    (api.close)(device);
                    (api.quit)(AUDIO)
                };
                return Err("SDL output rejected stereo PCM48k");
            }
            Ok(Self {
                api,
                device,
                policy: Policy::default(),
                _main: PhantomData,
            })
        })();
        if result.is_err() {
            CLAIMED.store(false, Ordering::Release);
        }
        result
    }
    pub fn set_owner(
        &mut self,
        _main: &MainThread,
        owner: Option<OwnerToken>,
    ) -> Result<(), &'static str> {
        if self.policy.set(owner)? {
            unsafe {
                (self.api.pause)(self.device, 1);
                (self.api.clear)(self.device);
            }
        }
        Ok(())
    }
    /// Player-owned output level, independent of slot ownership. A changed
    /// level withdraws already queued PCM so an old unmuted tail cannot escape.
    /// Only the currently acknowledged slot owner may change it.
    /// This controls this software sink only, never the television's volume.
    pub fn set_gain(
        &mut self,
        main: &MainThread,
        owner: OwnerToken,
        level: u8,
    ) -> Result<(), &'static str> {
        if self.policy.set_gain(owner, level)? {
            self.clear(main);
        }
        Ok(())
    }
    pub fn clear(&mut self, _main: &MainThread) {
        unsafe {
            (self.api.pause)(self.device, 1);
            (self.api.clear)(self.device);
        }
    }
    pub fn queued_bytes(&self, _main: &MainThread) -> u32 {
        unsafe { (self.api.queued)(self.device) }
    }
    /// Input is copied by SDL before return. Backpressure leaves the existing
    /// queue intact; the caller owns retry/drop timing and must keep its token.
    pub fn queue(
        &mut self,
        main: &MainThread,
        owner: OwnerToken,
        samples: &[i16],
    ) -> Result<(), &'static str> {
        let bytes = self
            .policy
            .admit(owner, samples.len(), self.queued_bytes(main))?;
        let adjusted = attenuate(samples, self.policy.gain)?;
        if unsafe { (self.api.queue)(self.device, adjusted.as_ptr().cast(), bytes) } != 0 {
            return Err("SDL audio queue failed");
        }
        unsafe { (self.api.pause)(self.device, 0) };
        Ok(())
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        unsafe {
            (self.api.pause)(self.device, 1);
            (self.api.clear)(self.device);
            (self.api.close)(self.device);
            (self.api.quit)(AUDIO);
        }
        CLAIMED.store(false, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn token() -> OwnerToken {
        OwnerToken {
            scene: 4,
            slot: 2,
            generation: 7,
        }
    }
    #[test]
    fn ownership_rejects_every_stale_identity_and_mute() {
        let mut p = Policy::default();
        let t = token();
        assert!(p.admit(t, 2, 0).is_err());
        assert_eq!(p.set(Some(t)), Ok(true));
        assert_eq!(p.set(Some(t)), Ok(false));
        assert_eq!(p.admit(t, 4, 0), Ok(8));
        for other in [
            OwnerToken { scene: 5, ..t },
            OwnerToken { slot: 3, ..t },
            OwnerToken { generation: 8, ..t },
        ] {
            assert!(p.admit(other, 4, 0).is_err());
        }
        assert!(p.set(Some(OwnerToken { slot: 9, ..t })).is_err());
        assert_eq!(p.owner, Some(t));
        assert_eq!(p.set(None), Ok(true));
        assert!(p.admit(t, 4, 0).is_err());
    }
    #[test]
    fn stereo_queue_policy_is_bounded_and_never_truncates_chunks() {
        let mut p = Policy::default();
        let t = token();
        p.set(Some(t)).unwrap();
        assert!(p.admit(t, 0, 0).is_err());
        assert!(p.admit(t, 3, 0).is_err());
        assert!(p.admit(t, MAX_CHUNK_SAMPLES + 2, 0).is_err());
        assert_eq!(p.admit(t, MAX_CHUNK_SAMPLES, 0), Ok(19_200));
        assert!(p.admit(t, 2, MAX_QUEUED_BYTES).is_err());
        assert!(p.admit(t, 2, u32::MAX).is_err());
        assert_eq!(p.admit(t, 2, MAX_QUEUED_BYTES - 4), Ok(4));
    }
    #[test]
    fn gain_unity_borrows_extremes_and_half_gain_is_safe_and_non_mutating() {
        let samples = [i16::MIN, i16::MAX, -1, 1, -24000, 24000];
        let original = samples;
        let unity = attenuate(&samples, 100).unwrap();
        assert!(matches!(&unity, Cow::Borrowed(_)));
        assert_eq!(unity.as_ptr(), samples.as_ptr());
        assert_eq!(unity.as_ref(), samples);
        assert_eq!(
            attenuate(&samples, 50).unwrap().as_ref(),
            [-16384, 16383, 0, 0, -12000, 12000]
        );
        assert_eq!(samples, original);
        for level in 0..=100 {
            for (output, input) in attenuate(&samples, level).unwrap().iter().zip(samples) {
                assert!(i32::from(*output).abs() <= i32::from(input).abs());
            }
        }
    }
    #[test]
    fn mute_and_unmute_restore_gain_without_rebinding_the_owner() {
        let mut policy = Policy::default();
        assert_eq!(policy.gain, 100);
        let owner = token();
        policy.set(Some(owner)).unwrap();
        assert_eq!(policy.set_gain(owner, 37), Ok(true));
        assert_eq!(policy.set_gain(owner, 37), Ok(false));
        let samples = [i16::MIN, i16::MAX];
        let remembered = attenuate(&samples, policy.gain).unwrap().into_owned();
        assert_eq!(policy.set_gain(owner, 0), Ok(true));
        assert_eq!(attenuate(&samples, policy.gain).unwrap().as_ref(), [0, 0]);
        assert_eq!(policy.owner, Some(owner));
        assert_eq!(policy.set_gain(owner, 37), Ok(true));
        assert_eq!(
            attenuate(&samples, policy.gain).unwrap().as_ref(),
            remembered
        );
        policy.set(Some(OwnerToken { slot: 3, ..owner })).unwrap();
        assert_eq!(policy.gain, 37);
        policy.set(None).unwrap();
        assert_eq!(policy.gain, 37);
    }
    #[test]
    fn gain_rejects_invalid_levels_and_pcm_bounds_before_allocating() {
        let mut policy = Policy::default();
        policy.set(Some(token())).unwrap();
        policy.set_gain(token(), 42).unwrap();
        for invalid in [101, u8::MAX] {
            assert!(policy.set_gain(token(), invalid).is_err());
            assert_eq!(policy.gain, 42);
            assert_eq!(policy.owner, Some(token()));
            assert!(attenuate(&[1, 2], invalid).is_err());
        }
        for level in [0, 50, 100] {
            assert!(attenuate(&[], level).is_err());
            assert!(attenuate(&[1], level).is_err());
            assert!(attenuate(&vec![1; MAX_CHUNK_SAMPLES + 2], level).is_err());
            assert_eq!(
                attenuate(&vec![i16::MIN; MAX_CHUNK_SAMPLES], level)
                    .unwrap()
                    .len(),
                MAX_CHUNK_SAMPLES
            );
        }
    }
    #[test]
    fn stale_or_withdrawn_owner_cannot_change_gain_or_request_a_flush() {
        let owner = token();
        let mut policy = Policy::default();
        assert!(policy.set_gain(owner, 25).is_err());
        assert_eq!(policy.gain, 100);
        policy.set(Some(owner)).unwrap();
        assert_eq!(policy.set_gain(owner, 62), Ok(true));
        for stale in [
            OwnerToken { scene: 5, ..owner },
            OwnerToken { slot: 3, ..owner },
            OwnerToken {
                generation: 8,
                ..owner
            },
        ] {
            assert!(policy.set_gain(stale, 0).is_err());
            assert_eq!(policy.gain, 62);
            assert_eq!(policy.owner, Some(owner));
        }
        policy.set(None).unwrap();
        assert!(policy.set_gain(owner, 0).is_err());
        assert_eq!(policy.gain, 62);
        let next = OwnerToken {
            generation: 8,
            ..owner
        };
        policy.set(Some(next)).unwrap();
        assert_eq!(policy.gain, 62);
        assert!(policy.set_gain(owner, 100).is_err());
        assert_eq!(policy.gain, 62);
        assert_eq!(policy.set_gain(next, 62), Ok(false));
    }
    #[test]
    fn sdl_audio_spec_layout_matches_pointer_width_abi() {
        assert_eq!(std::mem::offset_of!(AudioSpec, callback), 16);
        assert_eq!(
            std::mem::size_of::<AudioSpec>(),
            if cfg!(target_pointer_width = "64") {
                32
            } else {
                24
            }
        );
    }
}

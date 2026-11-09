//! Logical/native ownership captured before the media thread is spawned.
use super::*;
use std::cell::RefCell;
#[derive(Clone)]
struct Owner { playback: crate::route::PlaybackIdentity, epoch: u32 }
thread_local! { static OWNER: RefCell<Option<Owner>> = const { RefCell::new(None) }; }
/// Bind this demux invocation to its actual logical item and native Load.
pub fn with_observed_hls_worker<T>(epoch: u32, playback: crate::route::PlaybackIdentity, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Owner>);
    impl Drop for Restore { fn drop(&mut self) {
        OWNER.with(|slot| *slot.borrow_mut() = self.0.take());
    } }
    let previous = OWNER.with(|slot| slot.replace(Some(Owner { playback, epoch })));
    let _restore = Restore(previous);
    run()
}
pub(super) fn scope(source: &str, variant: &str, discontinuity: u64) -> Option<hls_observed::Scope> {
    let owner = OWNER.with(|slot| slot.borrow().clone())?;
    hls_observed::Scope::new(source, variant, u64::from(owner.epoch), discontinuity).ok()
        .map(|scope| scope.with_playback(owner.playback))
}

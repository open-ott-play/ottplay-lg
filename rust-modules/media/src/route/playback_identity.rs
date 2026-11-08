//! Process-local identity of one accepted logical playback, independent of URL,
//! provider keys, PMS sessions, decoder epochs and individual retry requests.
use std::sync::Arc;

#[derive(Clone)]
pub struct PlaybackIdentity(Arc<()>);

impl PlaybackIdentity {
    pub(super) fn fresh() -> Self {
        Self(Arc::new(()))
    }

    pub(super) fn for_admission(current: Option<&Self>, retry: bool) -> Option<Self> {
        if retry {
            current.cloned()
        } else {
            Some(Self::fresh())
        }
    }
}

impl PartialEq for PlaybackIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for PlaybackIdentity {}

impl std::fmt::Debug for PlaybackIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlaybackIdentity(<opaque>)")
    }
}

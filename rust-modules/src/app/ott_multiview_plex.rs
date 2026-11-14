//! Explicit-server Plex metadata preflight for an independently authorized compositor source broker.
//! No native session, active-server switch, watch-state update or transcode session is created.
//! A native credential witness is NOT a grant for a remote server to fetch this URL.
use super::*;
use plx_plex::plex::{self, BoundedRead, DirectSourceWitness, Metadata};
use crate::app::ott_plex_prepare::CapturedAuthority;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error { Invalid, Unsupported, Unavailable, Changed, Cancelled }

/// Captured on the main owner from an already bound exact profile/Favorite. No IO here.
/// No Clone/Debug/Serialize: one source worker owns the read and captured credential lifetime.
pub(super) struct Capture {
    authority: Arc<CapturedAuthority>,
    scope: plex::grant::GrantScope,
    read: BoundedRead,
    favorite: Favorite,
    session: String,
}
impl Capture {
    pub fn capture(profile: &ProviderProfile, favorite: &Favorite, session: String) -> Result<Self, Error> {
        use plx_data::ott::favorites::Locator;
        profile.validate().map_err(|_| Error::Invalid)?;
        favorite.validate().map_err(|_| Error::Invalid)?;
        let Locator::Plex { server_id, rating_key } = &favorite.locator else { return Err(Error::Invalid); };
        if profile.kind != ProviderKind::Plex || profile.source != *server_id
            || !decimal(rating_key) || session.is_empty() || session.len() > 128
            || !session.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) {
            return Err(Error::Invalid);
        }
        let server = plex::id_of_machine(server_id).ok_or(Error::Unavailable)?;
        let client = plex::client_for(server).ok_or(Error::Unavailable)?;
        let scope = plex::grant::scope();
        let authority = Arc::new(CapturedAuthority::capture(client).map_err(|_| Error::Unavailable)?);
        if !authority.matches(client) || authority.machine_id != *server_id { return Err(Error::Changed); }
        let read = client.capture_current_bounded_read(authority.client_key.token).ok_or(Error::Changed)?;
        let capture = Self { authority, scope, read, favorite: favorite.clone(), session };
        if !capture.current() { return Err(Error::Changed); }
        Ok(capture)
    }
    fn current(&self) -> bool {
        self.scope == plex::grant::scope() && self.read.current()
            && plex::client_for(self.authority.server).is_some_and(|client| self.authority.matches(client))
            && self.scope == plex::grant::scope()
    }
    pub fn prepare_metadata(self, off: &plx_base::task::OffFrame, cancel: &AtomicBool) -> Result<PreparedMetadata, Error> {
        self.prepare_with(off, cancel, |read, key| read.metadata(key))
    }
    fn prepare_with(self, _off: &plx_base::task::OffFrame, cancel: &AtomicBool,
        fetch: impl FnOnce(&BoundedRead, &str) -> Option<Metadata>) -> Result<PreparedMetadata, Error>
    {
        if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
        if !self.current() { return Err(Error::Changed); }
        let plx_data::ott::favorites::Locator::Plex { server_id, rating_key } = &self.favorite.locator
            else { return Err(Error::Invalid); };
        // Existing BoundedRead freezes the token and bounds this one exact metadata GET to
        // 8 MiB / 8 s. Cancellation is rechecked after the bounded synchronous read.
        let metadata = fetch(&self.read, rating_key).ok_or(Error::Unavailable)?;
        if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
        if !self.current() { return Err(Error::Changed); }
        if metadata.rating_key != *rating_key || !matches!(metadata.kind.as_str(), "movie" | "episode" | "clip") {
            return Err(Error::Invalid);
        }
        let media = metadata.media.first().ok_or(Error::Unsupported)?;
        if media.part.len() != 1 { return Err(Error::Unsupported); }
        let part = &media.part[0];
        if !part_key(&part.key) { return Err(Error::Unsupported); }
        let client = plex::client_for(self.authority.server).filter(|client| self.authority.matches(client))
            .ok_or(Error::Changed)?;
        let (url, witness) = client.direct_play_source(&part.key, &self.session).ok_or(Error::Changed)?;
        let entry = CatalogEntry { id: self.favorite.identity(), title: self.favorite.title.clone(),
            category: String::new(), description: String::new(), poster: String::new(),
            synopsis: None, protection: None, duration_ms: None, provider_id: None,
            provider_id_field: None, live: None,
            media: MediaLocator::Plex { server_id: server_id.clone(), rating_key: rating_key.clone() } };
        // Use exactly the ordinary source-attested metadata classification, not a portable title.
        let entry = crate::app::ott_metadata::apply_plex_metadata(entry, metadata).map_err(|_| Error::Invalid)?;
        let url = url.to_url();
        let authority = InputAuthority { authority: self.authority, scope: self.scope, witness, expected_url: url };
        if cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
        if !authority.current() { return Err(Error::Changed); }
        Ok(PreparedMetadata { entry, authority })
    }
}

/// Retain this witness with the active input, never serialize its private URL or authority.
/// Revocation must cancel the compositor's independently admitted source grant/HTTP transfer.
pub(super) struct InputAuthority {
    authority: Arc<CapturedAuthority>,
    scope: plex::grant::GrantScope,
    witness: DirectSourceWitness,
    expected_url: String,
}
impl InputAuthority {
    pub fn current(&self) -> bool {
        self.scope == plex::grant::scope() && self.witness.current_for(&self.expected_url)
            && plex::client_for(self.authority.server).is_some_and(|client| self.authority.matches(client))
            && self.scope == plex::grant::scope()
    }
}

/// Metadata-only preflight. The private witness binds current native authority but is never
/// exported as a URL or mistaken for the server broker's independently authenticated grant.
pub(super) struct PreparedMetadata { entry: CatalogEntry, authority: InputAuthority }
impl PreparedMetadata {
    pub fn into_parts(self) -> Result<(CatalogEntry, InputAuthority), Error> {
        if !self.authority.current() { return Err(Error::Changed); }
        Ok((self.entry, self.authority))
    }
}
fn decimal(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
}
fn part_key(value: &str) -> bool {
    if value.len() > 4096 { return false; }
    let Some(rest) = value.strip_prefix("/library/parts/") else { return false; };
    let parts: Vec<_> = rest.split('/').collect();
    if !(2..=3).contains(&parts.len()) || !decimal(parts[0]) { return false; }
    if parts.len() == 3 && !decimal(parts[1]) { return false; }
    let file = parts[parts.len() - 1];
    (file == "file" || file.starts_with("file."))
        && file.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_multiview_plex_tests.rs"]
mod tests;

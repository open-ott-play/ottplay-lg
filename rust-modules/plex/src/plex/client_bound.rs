//! Immutable credentials for the existing bounded, read-only library operations.
//! Never serialize or Debug this value. Registry authority remains with its caller.
use super::*;

#[derive(Clone)]
pub struct BoundedRead {
    client: &'static Client,
    generation: u32,
    token: String,
}
impl Client {
    /// Freeze the current credential only for the caller's already-captured
    /// generation. Token replacement publishes its generation under this lock,
    /// so a replacement credential cannot be captured under an old authority.
    pub fn capture_current_bounded_read(&'static self, generation: u32) -> Option<BoundedRead> {
        let token = self.token.read().ok()?;
        if token.is_empty() || self.token_gen() != generation || self.data_io_disabled() {
            return None;
        }
        Some(BoundedRead { client: self, generation, token: token.clone() })
    }
    /// Capture a known exact credential under the token lock. A later retoken
    /// cannot substitute its credential into an already-captured request.
    pub fn capture_bounded_read(&'static self, generation: u32, expected: &str) -> Option<BoundedRead> {
        let token = self.token.read().ok()?;
        if expected.is_empty() || token.as_str() != expected || self.token_gen() != generation {
            return None;
        }
        Some(BoundedRead { client: self, generation, token: token.clone() })
    }
}
impl BoundedRead {
    pub fn current(&self) -> bool {
        self.client.token_gen() == self.generation && !self.client.data_io_disabled()
    }
    fn path(&self, path: &str) -> Option<String> {
        self.current().then(|| with_captured_token(path, &self.token))
    }
    pub(in crate::plex) fn get_json_bounded(&self, path: &str) -> Option<MediaContainer> {
        let path = self.path(path)?;
        let reply = self.client.get_json_reply_bounded_path(&path, 8 * 1024 * 1024,
            std::time::Duration::from_secs(8))?;
        if !self.current() || !reply.ok() { return None; }
        serde_json::from_slice::<Envelope>(&reply.body).ok().map(|value| value.media_container)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_read_never_uses_a_replacement_token() {
        let _serial = plx_base::testlock::serial();
        let client: &'static Client = Box::leak(Box::new(Client::new(ServerId::from_raw(0), "captured-fixture",
            Origin::parse("https://captured.invalid:32400").unwrap(), "old-token", "fixture-install")));
        let generation = client.token_gen();
        assert!(client.capture_bounded_read(generation, "wrong-token").is_none());
        let read = client.capture_bounded_read(generation, "old-token").unwrap();
        assert_eq!(read.path("/library/sections").as_deref(), Some("/library/sections?X-Plex-Token=old-token"));
        // Deliberately mutate without publishing a generation: even this invalid
        // pair must not let an explicitly expected old secret capture a replacement.
        *client.token.write().unwrap() = "replacement-token".into();
        assert!(client.capture_bounded_read(generation, "old-token").is_none());
        assert_eq!(read.path("/library/sections").as_deref(), Some("/library/sections?X-Plex-Token=old-token"));
        client.set_token("replacement-token");
        assert!(read.path("/library/sections").is_none());
        // Stale generation returns before the transport helper, so this is a
        // pure rejection check and cannot issue a request to the fixture host.
        assert!(read.get_json_bounded("/library/sections").is_none());
        assert!(client.capture_bounded_read(generation, "replacement-token").is_none());
    }
    #[test]
    fn current_capture_rejects_old_authority_and_keeps_frozen_credentials() {
        let _serial = plx_base::testlock::serial();
        let client: &'static Client = Box::leak(Box::new(Client::new(ServerId::from_raw(0), "captured-fixture",
            Origin::parse("https://captured.invalid:32400").unwrap(), "old-token", "fixture-install")));
        let generation = client.token_gen();
        let original = client.capture_current_bounded_read(generation).unwrap();
        client.set_token("replacement-token");
        assert_ne!(client.token_gen(), generation);
        assert!(client.capture_current_bounded_read(generation).is_none());
        assert!(original.path("/library/sections").is_none());
        assert_eq!(original.token, "old-token"); // The captured value was never rewritten.
        let replacement = client.capture_current_bounded_read(client.token_gen()).unwrap();
        assert_eq!(replacement.path("/library/sections").as_deref(),
            Some("/library/sections?X-Plex-Token=replacement-token"));
        client.set_token("");
        assert!(client.capture_current_bounded_read(client.token_gen()).is_none());
        assert!(replacement.path("/library/sections").is_none());
    }

}

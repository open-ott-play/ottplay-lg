//! Revocable evidence for exactly the Direct URL produced at plan creation.
//! This borrows the existing registry-owned client; it owns no lease or worker.
use super::*;
use crate::plex::ClientKey;
use plx_base::sha256::sha256;

#[derive(Clone, Copy)]
pub struct DirectSourceWitness {
    client: &'static Client,
    key: ClientKey,
    grant: u32,
    token_hash: [u8; 32],
    url_hash: [u8; 32],
}
impl std::fmt::Debug for DirectSourceWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DirectSourceWitness(<opaque>)")
    }
}
impl DirectSourceWitness {
    /// Check the original object, never look up or adopt a replacement client.
    /// Registry re-pointing revokes this object's token before publishing the
    /// new one. Hash comparison also closes set_token's post-unlock generation
    /// window. No lock survives this observation or crosses an IO effect.
    pub fn current(&self) -> bool {
        let Ok(token) = self.client.token.read() else { return false; };
        !token.is_empty() && !self.client.data_io_disabled()
            && ClientKey::of(self.client) == self.key
            && self.client.grant_epoch() == self.grant
            && sha256(token.as_bytes()) == self.token_hash
    }
    pub fn current_for(&self, url: &str) -> bool {
        sha256(url.as_bytes()) == self.url_hash && self.current()
    }
}
impl Client {
    pub(in crate::plex) fn capture_direct_source(&'static self, path: &str)
        -> Option<(StreamUrl, DirectSourceWitness)>
    {
        let token = self.token.read().ok()?;
        if token.is_empty() || self.data_io_disabled() { return None; }
        let url = StreamUrl { origin: self.origin.clone(), path: with_captured_token(path, &token) };
        let witness = DirectSourceWitness {
            client: self, key: ClientKey::of(self), grant: self.grant_epoch(),
            token_hash: sha256(token.as_bytes()), url_hash: sha256(url.to_url().as_bytes()),
        };
        Some((url, witness))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { crate::plex::reset_servers_for_test(); } }
    fn register(host: &str) -> ServerId {
        // A secure synthetic origin keeps the credential eligible in every flavor;
        // this registry-only seam never starts a transport or session worker.
        let origin = Origin::parse(&format!("https://{host}:32400")).unwrap();
        crate::plex::register_pinned_with_client_id("direct-source-test", &origin,
            "original-token", None, "client-test", Default::default())
    }
    fn registered() -> (&'static Client, Reset) {
        let id = register("direct-source.invalid");
        (crate::plex::client_for(id).unwrap(), Reset)
    }
    #[test]
    fn paired_url_witness_rejects_the_pre_generation_token_swap_window() {
        let _serial = plx_base::testlock::serial();
        let (client, _reset) = registered();
        let (url, witness) = client.direct_play_source("/library/parts/77/file.mkv", "playback").unwrap();
        assert_eq!(url.to_url(), client.direct_play_url("/library/parts/77/file.mkv", "playback").to_url());
        assert!(witness.current_for(&url.to_url()));
        assert!(!witness.current_for(&format!("{}&other=1", url.to_url())));
        let generation = client.token_gen();
        *client.token.write().unwrap() = "replacement-before-generation".into();
        assert_eq!(generation, client.token_gen());
        assert!(!witness.current(), "the token lock, not a later generation write, fences the source");
        assert_eq!(format!("{witness:?}"), "DirectSourceWitness(<opaque>)");
    }
    #[test]
    fn real_repoint_and_regrant_cannot_revive_the_original_source() {
        let _serial = plx_base::testlock::serial();
        let (client, _reset) = registered();
        let (_, witness) = client.direct_play_source("/library/parts/77/file.mkv", "playback").unwrap();
        let same_id = register("replacement-source.invalid");
        assert_eq!(same_id, client.id());
        assert_ne!(crate::plex::client_for(same_id).unwrap().instance_gen(), client.instance_gen());
        assert!(!client.uses_token("original-token"));
        assert!(!witness.current());
        let replacement = crate::plex::client_for(same_id).unwrap();
        let (_, new_witness) = replacement.direct_play_source("/library/parts/77/file.mkv", "playback").unwrap();
        replacement.set_token("");
        replacement.set_token("original-token");
        assert!(!new_witness.current(), "same bytes cannot revive a revoked grant");
    }
    #[test]
    fn anonymous_url_stays_available_without_claiming_retry_authority() {
        let _serial = plx_base::testlock::serial();
        let (client, _reset) = registered();
        client.set_token("");
        let url = client.direct_play_url("/library/parts/77/file.mkv", "playback").to_url();
        assert!(url.contains("/library/parts/77/file.mkv?"));
        assert!(url.ends_with("X-Plex-Token="));
        assert!(client.direct_play_source("/library/parts/77/file.mkv", "playback").is_none());
    }
}

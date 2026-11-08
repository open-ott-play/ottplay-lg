//! Test-only transport completion injection and observation of real queued responses.
use super::*;

impl Remote {
    /// Stage an ordinary raw poll completion for the real Remote::tick path.
    /// This opens no transport and leaves the installation disabled.
    #[cfg(feature = "hostsim")]
    pub(crate) fn queue_poll_for_test(&mut self, body: &str) {
        assert!(self.flight.is_none());
        let (task, sender) = transport::Task::pending_for_test();
        sender.send(Outcome::Http {
            response: Ok(protocol::Response { status: 200, body: body.as_bytes().to_vec() }),
            elapsed_ms: 0,
        }).unwrap_or_else(|_| panic!("fixture transport receiver disappeared"));
        self.flight = Some(Flight { kind: Kind::Delivery(SendKind::Poll), generation: self.generation, task });
    }

    pub(crate) fn response_for_test(&self) -> Option<Value> {
        let (kind, request) = self.delivery.next(&self.config);
        if !matches!(kind, SendKind::Response) {
            return None;
        }
        serde_json::from_str(request.body.as_deref()?).ok()
    }
}

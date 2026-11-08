//! Single admitted install-language write. Only the existing session writer
//! performs persistence; the UI thread publishes its prepared locale afterward.
use plx_platform::i18n::{self, Preference, PreparedLocale};
use plx_screens::registry::LanguageApplyResult;
use std::sync::mpsc::{Sender, TryRecvError};

type Pending = Write<PreparedLocale, plx_base::storage_worker::TypedTicket<bool>>;

#[derive(Default)]
pub(in crate::app) struct State { pending: Option<Pending> }

struct Write<Token, Receipt> {
    prepared: Token,
    receipt: Receipt,
    reply: Sender<LanguageApplyResult>,
}

pub(super) fn begin(state: &mut State, choice: Preference, reply: Sender<LanguageApplyResult>) {
    begin_with(&mut state.pending, reply, || i18n::prepare(choice), || {
        plx_base::storage_worker::submit_retained(move || {
            let durable = plx_plex::plex::session::set_language(choice);
            plx_machine::idle::invalidate();
            durable
        })
    });
    plx_machine::idle::invalidate();
}

pub(super) fn poll(state: &mut State, live: bool) -> Option<u64> {
    poll_with(&mut state.pending, |receipt| receipt.try_recv(), |prepared| {
        publish_if_live(prepared, live, i18n::publish)
    })
}

fn publish_if_live<Token, Error>(prepared: Token, live: bool,
    publish: impl FnOnce(Token) -> Result<u64, Error>) -> Result<u64, ()> {
    if !live { return Err(()); }
    publish(prepared).map_err(|_| ())
}

// The same receipt coordinator is exercised with local mailboxes in tests;
// no test rewrites a global locale or grants persistence authority.
fn begin_with<Token, Receipt, Error>(
    pending: &mut Option<Write<Token, Receipt>>,
    reply: Sender<LanguageApplyResult>,
    prepare: impl FnOnce() -> Result<Token, Error>,
    submit: impl FnOnce() -> Receipt,
) -> bool {
    if pending.is_some() {
        let _ = reply.send(LanguageApplyResult::Failed);
        return false;
    }
    let Ok(prepared) = prepare() else {
        let _ = reply.send(LanguageApplyResult::Failed);
        return false;
    };
    let receipt = submit();
    *pending = Some(Write { prepared, receipt, reply });
    true
}

fn poll_with<Token, Receipt, Error>(
    pending: &mut Option<Write<Token, Receipt>>,
    receive: impl FnOnce(&Receipt) -> Result<bool, TryRecvError>,
    publish: impl FnOnce(Token) -> Result<u64, Error>,
) -> Option<u64> {
    let durable = match receive(&pending.as_ref()?.receipt) {
        Ok(durable) => durable,
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => false,
    };
    let write = pending.take().expect("the observed pending language write is retained");
    let epoch = if durable { publish(write.prepared).ok() } else { None };
    let result = if epoch.is_some() { LanguageApplyResult::LiveApplied }
        else if durable { LanguageApplyResult::RestartRequired }
        else { LanguageApplyResult::Failed };
    let _ = write.reply.send(result);
    epoch
}

#[cfg(test)]
#[path = "preferences_language_tests.rs"]
mod tests;

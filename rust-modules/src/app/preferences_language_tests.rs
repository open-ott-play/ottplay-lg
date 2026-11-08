use super::*;
use std::cell::Cell;
use std::sync::mpsc::{self, Receiver};

type LocalWrite = Option<Write<u64, Receiver<bool>>>;

fn start(pending: &mut LocalWrite, token: u64) -> (Sender<bool>, Receiver<LanguageApplyResult>) {
    let (stored, receipt) = mpsc::channel();
    let (reply, ui) = mpsc::channel();
    assert!(begin_with(pending, reply, || Ok::<_, ()>(token), || receipt));
    (stored, ui)
}

#[test]
fn preparation_failure_never_submits_a_write() {
    let mut pending: LocalWrite = None;
    let (reply, ui) = mpsc::channel();
    assert!(!begin_with(&mut pending, reply, || Err::<u64, _>(()),
        || -> Receiver<bool> { panic!("prepare failure cannot write") }));
    assert!(pending.is_none());
    assert_eq!(ui.try_recv().unwrap(), LanguageApplyResult::Failed);
}

#[test]
fn publication_waits_for_durable_success_and_runs_once() {
    let mut pending = None;
    let (stored, ui) = start(&mut pending, 19);
    assert_eq!(poll_with(&mut pending, Receiver::try_recv,
        |_| -> Result<u64, ()> { panic!("pending durability cannot publish") }), None);
    assert!(matches!(ui.try_recv(), Err(TryRecvError::Empty)));
    stored.send(true).unwrap();
    let calls = Cell::new(0);
    assert_eq!(poll_with(&mut pending, Receiver::try_recv, |token| {
        assert_eq!(token, 19);
        calls.set(calls.get() + 1);
        Ok::<_, ()>(20)
    }), Some(20));
    assert_eq!(ui.try_recv().unwrap(), LanguageApplyResult::LiveApplied);
    assert_eq!(poll_with(&mut pending, Receiver::try_recv,
        |_| -> Result<u64, ()> { panic!("completion cannot be applied twice") }), None);
    assert_eq!(calls.get(), 1);
}

#[test]
fn retired_page_receiver_does_not_drop_an_admitted_publication() {
    let mut pending = None;
    let (stored, ui) = start(&mut pending, 7);
    drop(ui);
    stored.send(true).unwrap();
    assert_eq!(poll_with(&mut pending, Receiver::try_recv,
        |token| { assert_eq!(token, 7); Ok::<_, ()>(8) }), Some(8));
    assert!(pending.is_none());
}

#[test]
fn failed_or_disconnected_storage_keeps_the_previous_locale() {
    for disconnected in [false, true] {
        let mut pending = None;
        let (stored, ui) = start(&mut pending, 3);
        if !disconnected { stored.send(false).unwrap(); }
        drop(stored);
        assert_eq!(poll_with(&mut pending, Receiver::try_recv,
            |_| -> Result<u64, ()> { panic!("unconfirmed storage cannot publish") }), None);
        assert_eq!(ui.try_recv().unwrap(), LanguageApplyResult::Failed);
        assert!(pending.is_none());
    }
}

#[test]
fn a_pending_choice_rejects_another_choice_before_prepare_or_write() {
    let mut pending = None;
    let (stored, first) = start(&mut pending, 5);
    let (reply, second) = mpsc::channel();
    assert!(!begin_with(&mut pending, reply,
        || -> Result<u64, ()> { panic!("second request cannot prepare") },
        || -> Receiver<bool> { panic!("second request cannot write") }));
    assert_eq!(second.try_recv().unwrap(), LanguageApplyResult::Failed);
    stored.send(true).unwrap();
    assert_eq!(poll_with(&mut pending, Receiver::try_recv, |token| {
        assert_eq!(token, 5); Ok::<_, ()>(6)
    }), Some(6));
    assert_eq!(first.try_recv().unwrap(), LanguageApplyResult::LiveApplied);
}

#[test]
fn durable_but_superseded_token_reports_restart_without_live_publication() {
    let mut pending = None;
    let (stored, ui) = start(&mut pending, 1);
    stored.send(true).unwrap();
    let current = Cell::new(4);
    assert_eq!(poll_with(&mut pending, Receiver::try_recv, |token| {
        if token != current.get() { return Err(()); }
        current.set(token + 1); Ok(current.get())
    }), None);
    assert_eq!(current.get(), 4, "a stale completion cannot overwrite a newer locale");
    assert_eq!(ui.try_recv().unwrap(), LanguageApplyResult::RestartRequired);
    assert!(pending.is_none());
    let (stored, next) = start(&mut pending, current.get());
    stored.send(true).unwrap();
    assert_eq!(poll_with(&mut pending, Receiver::try_recv, |token| {
        assert_eq!(token, current.get()); current.set(token + 1); Ok::<_, ()>(current.get())
    }), Some(5));
    assert_eq!(next.try_recv().unwrap(), LanguageApplyResult::LiveApplied);
}

#[test]
fn controlled_completion_keeps_durable_choice_for_restart_without_publishing() {
    let mut pending = None;
    let (stored, ui) = start(&mut pending, 10);
    stored.send(true).unwrap();
    assert_eq!(poll_with(&mut pending, Receiver::try_recv, |token| {
        publish_if_live(token, false, |_| -> Result<u64, ()> {
            panic!("a now-controlled run cannot publish an admitted live locale")
        })
    }), None);
    assert_eq!(ui.try_recv().unwrap(), LanguageApplyResult::RestartRequired);
    assert!(pending.is_none());
}

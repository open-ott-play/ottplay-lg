//! Real accepted-queue fixtures; no decoder, network or replacement state owner.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, BorrowedTarget, PinEntries, Stored};
use crate::app::ott_runtime::kiosk_source::QueuePin;
use std::collections::BTreeMap;

fn kiosk_setup() -> App {
    let mut app = super::setup();
    app.ott.as_mut().unwrap().resume_source_work();
    app
}

fn snapshot(app: &App) -> QueuePin {
    transient::kiosk_queue(app.ott.as_ref().unwrap(), app)
        .expect("valid accepted queue")
        .expect("explicit queue")
}
fn settings(pin: &QueuePin) -> BTreeMap<String, Value> {
    BTreeMap::from([(
        stored::KEY.into(),
        Stored::queue(pin.entries.clone(), true)
            .unwrap()
            .encode()
            .unwrap(),
    )])
}
fn pins(settings: &BTreeMap<String, Value>) -> PinEntries<'_> {
    let value = stored::borrow_validated(settings).unwrap().unwrap();
    let BorrowedTarget::Queue(entries) = value.target() else {
        panic!("queue fixture");
    };
    entries
}
fn move_to(app: &mut App, sid: plx_plex::plex::ServerId, index: usize) {
    let mut runtime = app.ott.take().unwrap();
    let entry = runtime.catalog.entries[index].clone();
    let MediaLocator::Plex { rating_key, .. } = &entry.media else {
        panic!("Plex fixture");
    };
    install_native_identity(app, sid, rating_key);
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut() else {
        panic!("queue fixture");
    };
    let token = queue::PlaybackToken(runtime.play_generation + index as u64 + 1);
    session
        .queue
        .begin(session.queue.owner(), token, index)
        .unwrap();
    assert!(session
        .queue
        .observe(session.queue.owner(), token, queue::PlaybackPhase::Playing));
    let playing = runtime.playing.as_mut().unwrap();
    playing.identity = plx_media::route::playback_identity(&app.player.session);
    playing.selection_id = entry.id.clone();
    playing.entry = entry;
    runtime.selected = Some(playing.selection_id.clone());
    app.ott = Some(runtime);
}

#[test]
fn snapshot_preserves_duplicate_positions_and_current_index_without_saved_changes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "42", "7"]);
    move_to(&mut app, sid, 1);
    let before = saved_value(&app);
    let pin = snapshot(&app);
    assert_eq!(pin.index, 1);
    assert_eq!(pin.entries.len(), 3);
    assert_eq!(pin.entries[0], pin.entries[1]);
    assert_ne!(pin.entries[1], pin.entries[2]);
    assert!(pin
        .entries
        .iter()
        .all(|entry| entry.origin == pin.entries[0].origin));
    assert_eq!(pin.entries[0].origin.provider, "plex");
    assert_eq!(pin.entries[0].origin.source_alias, "queue-machine");
    let wire = serde_json::to_string(&pin.entries).unwrap();
    for private in [
        "synthetic-token",
        "private-fixture",
        "plex-occurrence:",
        "plex-queue.invalid",
    ] {
        assert!(!wire.contains(private));
    }
    assert_eq!(saved_value(&app), before);
}

#[test]
fn plex_snapshot_does_not_depend_on_unloaded_local_inventory_or_visible_category() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let before = snapshot(&app);
    let runtime = app.ott.as_mut().unwrap();
    // Plex capture must not require the unrelated Local worker to finish.
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(8);
    assert_eq!(runtime.local_inventory.generation(), 0);
    transient::set_view(runtime, "Other category", true);
    let after = snapshot(&app);
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.index, before.index);
    assert_eq!(after.current_identity, before.current_identity);
}

#[test]
fn borrowed_pins_match_the_full_order_and_only_canonical_occurrence_ids() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "42", "7"]);
    let pin = snapshot(&app);
    let saved = settings(&pin);
    let runtime = app.ott.as_ref().unwrap();
    assert!(transient::kiosk_queue_matches_current(
        runtime,
        &app,
        pins(&saved)
    ));
    for row in &runtime.catalog.entries {
        assert!(transient::kiosk_queue_allows(
            runtime,
            &app,
            &row.id,
            pins(&saved)
        ));
    }
    let owner = pin.owner.0;
    for id in [
        format!("plex-occurrence:0{owner}:0"),
        format!("plex-occurrence:{owner}:00"),
        format!("plex-occurrence:+{owner}:0"),
        format!("plex-occurrence:{owner}:3"),
        format!("plex-occurrence:{}:0", owner + 1),
        format!("plex-occurrence:{owner}:184467440737095516160"),
        "favorite:42".into(),
    ] {
        assert!(
            !transient::kiosk_queue_allows(runtime, &app, &id, pins(&saved)),
            "{id}"
        );
    }
    let mut reordered = pin.clone();
    reordered.entries.swap(1, 2);
    let reordered = settings(&reordered);
    assert!(!transient::kiosk_queue_matches_current(
        runtime,
        &app,
        pins(&reordered)
    ));
}

#[test]
fn repeat_setter_revalidates_identity_and_preserves_cursor_then_wraps() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let stale = snapshot(&app);
    move_to(&mut app, sid, 1);
    let pin = snapshot(&app);
    let before = saved_value(&app);
    let mut runtime = app.ott.take().unwrap();
    assert!(transient::kiosk_repeat_all(&mut runtime, &app, &stale).is_err());
    assert_eq!(
        runtime.playing.as_ref().unwrap().identity.as_ref(),
        Some(&pin.current_identity)
    );
    transient::kiosk_repeat_all(&mut runtime, &app, &pin).unwrap();
    let Some(transient::TransientMedia::Plex(s)) = runtime.transient.accepted.as_mut() else {
        panic!("queue fixture");
    };
    assert_eq!(s.queue.index(), 1);
    assert_eq!(s.queue.owner().0, pin.owner.0);
    assert_eq!(
        s.queue.ended(s.queue.owner(), s.queue.playback().unwrap()),
        queue::PlexDecision::Start { index: 0 }
    );
    app.ott = Some(runtime);
    assert_eq!(saved_value(&app), before);
}

#[test]
fn snapshot_rejects_same_item_with_new_native_identity_and_changed_profile() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42"]);
    let pin = snapshot(&app);
    install_native_identity(&mut app, sid, "42");
    assert!(!transient::kiosk_queue_is_current(
        app.ott.as_ref().unwrap(),
        &app,
        &pin
    ));
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err());
    seed_queue(&mut app, sid, &["42"]);
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .profiles
        .profiles
        .iter_mut()
        .find(|p| p.kind == ProviderKind::Plex)
        .unwrap()
        .source_alias = "Changed alias".into();
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err());
}

#[test]
fn unavailable_or_ambiguous_saved_source_cannot_revalidate_pin() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42"]);
    let pin = snapshot(&app);
    let runtime = app.ott.as_mut().unwrap();
    let mut duplicate = runtime
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| p.kind == ProviderKind::Plex)
        .unwrap()
        .clone();
    duplicate.id = "duplicate".into();
    runtime.saved.profiles.profiles.push(duplicate);
    assert!(!transient::kiosk_queue_is_current(
        app.ott.as_ref().unwrap(),
        &app,
        &pin
    ));
    app.ott.as_mut().unwrap().saved.profiles.profiles.pop();
    plx_plex::plex::reset_servers_for_test();
    assert!(!transient::kiosk_queue_is_current(
        app.ott.as_ref().unwrap(),
        &app,
        &pin
    ));
}

#[test]
fn incomplete_or_reordered_catalog_is_not_a_full_queue_snapshot() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    app.ott.as_mut().unwrap().catalog_complete = false;
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err());
    app.ott.as_mut().unwrap().catalog_complete = true;
    app.ott.as_mut().unwrap().catalog.entries.swap(0, 1);
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err());
}

#[test]
fn truncated_library_receipt_is_refused_even_with_valid_visible_rows() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let Some(transient::TransientMedia::Plex(s)) =
        app.ott.as_mut().unwrap().transient.accepted.as_mut()
    else {
        panic!("queue fixture");
    };
    s.receipt = Some(transient::plex_remote::LibraryReceipt {
        request: queue::LibraryRequest::Prepare {
            kind: queue::PrepareKind::Play,
            selector: queue::LibrarySelector::Id(queue::DecimalId::parse("17").unwrap()),
            shuffle: false,
            allow_truncate: true,
        },
        library: queue::LibraryRow {
            id: "17".into(),
            title: "Library".into(),
            kind: queue::LibraryKind::Movie,
        },
        truncated: true,
    });
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err());
}

#[test]
fn pending_new_occurrence_blocks_snapshot_without_authorizing_other_targets() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7", "9"]);
    let saved = settings(&snapshot(&app));
    let mut runtime = app.ott.take().unwrap();
    let next = runtime.catalog.entries[1].id.clone();
    let other = runtime.catalog.entries[2].id.clone();
    runtime.play_with_policy(&mut app, &next, false).unwrap();
    assert!(runtime.play_target.is_some());
    assert!(transient::kiosk_queue(&runtime, &app).is_err());
    assert!(!transient::kiosk_queue_allows(
        &runtime,
        &app,
        &other,
        pins(&saved)
    ));
    assert!(transient::kiosk_queue_allows(
        &runtime,
        &app,
        &next,
        pins(&saved)
    ));
    app.ott = Some(runtime);
}

#[test]
fn actual_eof_keeps_membership_for_queued_next_after_decoder_stop() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let saved = settings(&snapshot(&app));
    // Exercise the real strict-kiosk admission gate once owner5 is composed.
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .extend(saved.clone());
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    let Some(transient::TransientMedia::Plex(s)) = runtime.transient.accepted.as_ref() else {
        panic!("queue fixture");
    };
    assert!(
        s.queue.playback().is_none(),
        "real EOF consumed the old token"
    );
    let next = &runtime.catalog.entries[1].id;
    assert_eq!(
        &runtime
            .play_target
            .as_ref()
            .expect("real successor request")
            .id,
        next
    );
    // The host fixture has no decoder object; explicitly expose the real EOF
    // engine phase as well, rather than depending on the fixture's stale state.
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Idle);
    assert!(transient::kiosk_queue_allows(
        runtime,
        &app,
        next,
        pins(&saved)
    ));
    assert!(transient::kiosk_queue(runtime, &app).is_err());
}

#[test]
fn borrowed_foreign_favorite_cannot_be_pinned_or_authorize_queue_membership() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    let other = register_source(&mut app, "other-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let saved = settings(&snapshot(&app));
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| p.source == "other-machine")
        .unwrap()
        .clone();
    let mut row = runtime.catalog.entries[0].clone();
    row.media = MediaLocator::Plex {
        server_id: "other-machine".into(),
        rating_key: "42".into(),
    };
    let favorite = selection::capture(&profile, &row).unwrap();
    install_native_identity(&mut app, other, "42");
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile;
    playing.entry = row;
    playing.selection_id = format!("favorite:{}", favorite.identity());
    playing.identity = plx_media::route::playback_identity(&app.player.session);
    runtime.checkpoint(&app, true);
    assert!(transient::kiosk_queue(&runtime, &app).unwrap().is_none());
    assert!(!transient::kiosk_queue_matches_current(
        &runtime,
        &app,
        pins(&saved)
    ));
    assert!(!transient::kiosk_queue_allows(
        &runtime,
        &app,
        &runtime.catalog.entries[1].id,
        pins(&saved)
    ));
    app.ott = Some(runtime);
}

#[test]
fn ordinary_provider_without_explicit_queue_has_no_queue_pin() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let app = kiosk_setup();
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), &app)
        .unwrap()
        .is_none());
}

#[test]
fn runtime_queue_ownership_uses_the_real_queue_without_synthetic_item() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let pin = snapshot(&app);
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .extend(settings(&pin));
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.play_generation;
    assert!(runtime.current_kiosk_pin_owned(&app));
    kiosk_source::apply_durable_policy(&mut runtime, &mut app);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.playing.is_some());
    assert!(app.player.pending_exit.is_none());
    app.ott = Some(runtime);
}
#[test]
fn runtime_queue_ownership_rejects_a_foreign_native_selection() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let pin = snapshot(&app);
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .extend(settings(&pin));
    install_native_identity(&mut app, sid, "99");
    assert!(!app.ott.as_ref().unwrap().current_kiosk_pin_owned(&app));
}


// A bounded loopback endpoint keeps the captured client authority live while
// refusing any actual media. No external host, decoder or media file is used.
pub(super) struct RetryEndpoint {
    port: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl RetryEndpoint {
    pub(super) fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            let off = plx_base::task::OffFrame::for_test();
            let budget = plx_base::task::time::Budget::start(&off, std::time::Duration::from_secs(15));
            while !stopped.load(Ordering::Acquire) && !budget.expired(&off) {
                if let Ok((socket, _)) = plx_base::testnet::accept_within(
                    &listener, std::time::Duration::from_millis(50),
                ) {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
            }
        });
        Self { port, stop, worker: Some(worker) }
    }
    fn register(&self, app: &mut App) -> plx_plex::plex::ServerId {
        self.register_named(app, "queue-machine")
    }
    pub(super) fn register_named(&self, app: &mut App, machine: &str) -> plx_plex::plex::ServerId {
        register_source(app, machine);
        let origin = plx_plex::plex::Origin::parse(&format!("https://127.0.0.1:{}", self.port)).unwrap();
        let sid = plx_plex::plex::register_pinned_with_client_id(
            machine, &origin, "synthetic-token", None, "synthetic-client", Default::default(),
        );
        let source = app.ott.as_mut().unwrap().saved.settings
            .get_mut("nativePlexSources").unwrap().as_array_mut().unwrap()
            .iter_mut().find(|source| source["machine_id"].as_str() == Some(machine)).unwrap();
        source["address"] = json!(origin.host());
        source["port"] = json!(origin.port());
        // SourceRef names this field `origin` on the wire, not `origin_url`.
        source["origin"] = json!(origin.base());
        let restored: plx_plex::plex::session::SourceRef =
            serde_json::from_value(source.clone()).unwrap();
        assert!(restored.origin().as_ref() == Some(&origin),
            "the saved source must match the captured loopback client");
        sid
    }
}
impl Drop for RetryEndpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

#[test]
fn failed_pinned_plex_keeps_its_owner_until_the_shared_ten_second_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = kiosk_setup();
    let endpoint = RetryEndpoint::new();
    let sid = endpoint.register(&mut app);
    seed_queue(&mut app, sid, &["42", "7"]);
    let pin = snapshot(&app);
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.settings.extend(settings(&pin));
    // Retain the real route descriptor the normal Plex admission stores. The
    // loopback endpoint refuses media; the retry may start only a resolver.
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    plx_media::route::refuse_for_test(
        &mut app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert_eq!(plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Error);
    assert!(transient::kiosk_queue(&runtime, &app).is_err(),
        "an errored source must not become a newly captured pin");
    assert!(runtime.current_kiosk_pin_owned(&app),
        "failure must not revoke the already pinned queue");
    // Preserve the installed pin even if the same frame also observes EOF.
    app.ott = Some(runtime);
    plx_media::player::SHARED.ended.store(true, Ordering::Release);
    let queue_before_error = queue_value(&app);
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(queue_value(&app), queue_before_error,
        "Error+EOF must not advance the pinned queue");
    let mut runtime = app.ott.take().unwrap();
    runtime.reconcile_failure(&mut app);
    assert!(runtime.playing.is_some(), "ordinary failure handling must retain the pin");
    assert_eq!(runtime.playing.as_ref().unwrap().identity.as_ref(), Some(&pin.current_identity));
    let now = runtime.now_ms();
    let ticket = plx_media::route::play_request_ticket();
    runtime.kiosk_recovery_tick_at(&mut app, now);
    runtime.kiosk_recovery_tick_at(&mut app, now + 9_999);
    assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 0);
    assert!(plx_media::route::play_request_is_current(ticket));
    runtime.kiosk_recovery_tick_at(&mut app, now + 10_000);
    let retries = runtime.kiosk_health(&app).ok().map(|health| health.retries);
    let retried = !plx_media::route::play_request_is_current(ticket);
    let retained = runtime.playing.as_ref().and_then(|p| p.identity.as_ref())
        == Some(&pin.current_identity);
    // Cancel/drain before assertions so a failure never leaves a resolver using
    // the synthetic registry after the serial guard is released.
    plx_media::route::cancel_play(&mut app.player.session);
    let drained = plx_media::route::wait_resolve_workers_for_test(std::time::Duration::from_secs(10));
    assert!(drained, "the inert retry resolver must finish");
    assert_eq!(retries, Some(1), "the existing kiosk watchdog must admit exactly one retry");
    assert!(retried, "the real native restart must acquire a new request ticket");
    assert!(retained, "retry must retain the same queue occurrence");
    app.ott = Some(runtime);
}

#[test]
fn failed_plex_cannot_retry_after_source_replacement_or_explicit_stop() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for stopped in [false, true] {
        let mut app = kiosk_setup();
        let sid = register_source(&mut app, "queue-machine");
        seed_queue(&mut app, sid, &["42", "7"]);
        let pin = snapshot(&app);
        let mut runtime = app.ott.take().unwrap();
        runtime.saved.settings.extend(settings(&pin));
        plx_media::route::refuse_for_test(
            &mut app.player.session,
            plx_media::route::PlayVerdict::DirectPlayDisabled,
        );
        if stopped {
            let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut() else {
                panic!("Plex fixture");
            };
            session.queue.stop(session.queue.owner()).unwrap();
        } else {
            runtime.saved.profiles.profiles.iter_mut()
                .find(|p| p.kind == ProviderKind::Plex).unwrap().source = "replacement".into();
        }
        assert!(!runtime.current_kiosk_pin_owned(&app));
        let ticket = plx_media::route::play_request_ticket();
        let now = runtime.now_ms();
        runtime.kiosk_recovery_tick_at(&mut app, now);
        runtime.kiosk_recovery_tick_at(&mut app, now + 60_000);
        assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 0);
        assert!(plx_media::route::play_request_is_current(ticket));
        app.ott = Some(runtime);
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

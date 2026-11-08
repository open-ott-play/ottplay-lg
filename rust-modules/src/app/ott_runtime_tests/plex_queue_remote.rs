//! Remote Plex queues at the real Runtime/command/completion boundaries.
//! Synthetic worker results stand in for PMS reads; no network or decoder runs.
//! These are source-only regressions until the common runtime gate executes them.
use super::*;
use crate::app::ott_plex_prepare as prepare;
use crate::app::ott_plex_queue as queue;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn permanent() -> ProviderProfile {
    ProviderProfile {
        id: "permanent".into(),
        name: "Permanent".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(),
        source_alias: "Permanent".into(),
        enabled: true,
    }
}

fn live_entry() -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: "live".into(),
        title: "Live fixture".into(),
        category: "News".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: "https://media.invalid/live".into(),
        },
        duration_ms: None,
        provider_id: Some("live".into()),
        provider_id_field: Some("id".into()),
        live: None,
    }
}

fn setup() -> App {
    plx_plex::plex::reset_servers_for_test();
    plx_plex::plex::grant::reset_for_test();
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.generation = 7;
    runtime.provider = ProviderKind::M3u;
    runtime.saved.profiles.profiles = vec![permanent()];
    runtime.saved.profiles.active_profile = Some("permanent".into());
    runtime.saved.category = "News".into();
    runtime.catalog.entries = vec![live_entry()];
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.catalog_dirty = false;
    // Keep the existing workers inert. Tests consume queued work and inject its
    // terminal event; they do not install an alternative worker or runtime.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.continuity = continuity::Continuity::new(7, None);
    app
}

fn request(app: &App, action: &str, extra: Value) -> Invocation {
    let runtime = app.ott.as_ref().unwrap();
    let mut remote = Remote::disabled_for_test("dev_fixture", runtime.remote.runtime());
    let mut params = json!({"runtime":runtime.remote.runtime()});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    let mut invocation = remote.request_for_test(action, params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
    invocation
}

fn command(app: &mut App, action: &str, extra: Value) -> Option<Value> {
    let invocation = request(app, action, extra);
    let mut runtime = app.ott.take().unwrap();
    let result = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    result
}

fn saved_value(app: &App) -> Value {
    serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap()
}

fn register_source(app: &mut App, machine: &str) -> plx_plex::plex::ServerId {
    let origin = plx_plex::plex::Origin::parse("https://plex-queue.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        machine,
        &origin,
        "synthetic-token",
        None,
        "synthetic-client",
        Default::default(),
    );
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: format!("plex-{machine}"),
        name: format!("Plex {machine}"),
        kind: ProviderKind::Plex,
        source: machine.into(),
        source_alias: format!("Plex {machine}"),
        enabled: true,
    });
    let mut sources: Vec<plx_plex::plex::session::SourceRef> = runtime
        .saved
        .settings
        .get("nativePlexSources")
        .map(|value| serde_json::from_value(value.clone()).unwrap())
        .unwrap_or_default();
    sources.push(plx_plex::plex::session::SourceRef {
        machine_id: machine.into(),
        name: format!("Plex {machine}"),
        address: origin.host().into(),
        port: i64::from(origin.port()),
        token: "synthetic-token".into(),
        origin_url: origin.base(),
        ..Default::default()
    });
    runtime.saved.settings.insert(
        "nativePlexSources".into(),
        serde_json::to_value(sources).unwrap(),
    );
    sid
}

fn selection(ids: &[&str]) -> prepare::Selection {
    selection_for(ids, false)
}
fn selection_for(ids: &[&str], restore: bool) -> prepare::Selection {
    let ids: Vec<_> = ids
        .iter()
        .map(|id| queue::DecimalId::parse(id).unwrap())
        .collect();
    let mut records: Vec<queue::PlexRecord> = Vec::new();
    let mut metadata = Vec::new();
    let mut occurrences = Vec::new();
    for id in &ids {
        let index = match records.iter().position(|record| record.id == id.as_str()) {
            Some(index) => index,
            None => {
                let index = records.len();
                records.push(queue::PlexRecord {
                    id: id.as_str().into(),
                    title: format!("Film {}", id.as_str()),
                    kind: queue::RecordKind::Movie,
                    playable: true,
                });
                metadata.push(serde_json::from_value(json!({
                    "ratingKey":id.as_str(), "title":format!("Film {}",id.as_str()),
                    "type":"movie", "librarySectionID":17,
                    "Media":[{"Part":[{"key":format!("/library/parts/{}/1/file.mp4",id.as_str()),
                        "file":format!("/private-fixture/{}.mp4",id.as_str())}]}]
                })).unwrap());
                index
            }
        };
        occurrences.push(index);
    }
    prepare::Selection {
        prepared: if restore {
            queue::prepare_restore(&queue::RestoreIds::new(ids).unwrap(), &records).unwrap()
        } else { queue::prepare(&ids, &records).unwrap() },
        metadata,
        occurrences,
    }
}

fn captured(sid: plx_plex::plex::ServerId) -> prepare::CapturedAuthority {
    let client = plx_plex::plex::client_for(sid).unwrap();
    prepare::CapturedAuthority {
        server: sid,
        machine_id: client.machine_id().into(),
        client_key: plx_plex::plex::ClientKey::of(client),
        grant_epoch: client.grant_epoch(),
    }
}

/// External test-support prerequisite, supplied by the route owner: unlike
/// the older untagged identity helper this admits a fresh logical identity.
fn install_native_identity(app: &mut App, sid: plx_plex::plex::ServerId, rating_key: &str) {
    plx_media::route::install_plex_playback_for_test(&mut app.player.session, sid, rating_key);
    assert!(
        plx_media::route::playback_identity(&app.player.session).is_some(),
        "premise: queue ownership tests need a tagged native playback identity"
    );
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    assert_eq!(
        plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Playing
    );
}

fn proposal(
    owner: OwnerToken,
    sid: plx_plex::plex::ServerId,
    prepared: prepare::Prepared,
) -> prepare::Proposal {
    prepare::Proposal {
        owner: queue::OwnerToken(owner.0),
        authority: captured(sid),
        prepared,
    }
}

fn candidate_id(app: &App) -> String {
    match app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .candidate
        .as_ref()
        .expect("candidate")
    {
        transient::Candidate::Local(candidate) => match &candidate.purpose {
            transient::LocalPurpose::Remote(invocation) => invocation.id.clone(),
            transient::LocalPurpose::KioskFolder(_) | transient::LocalPurpose::Profile(_) => panic!("expected remote Local fixture"),
        },
        transient::Candidate::Plex(candidate) => candidate.work.purpose.invocation().unwrap().id.clone(),
    }
}

fn begin_plex(app: &mut App, action: &str, extra: Value) -> (OwnerToken, String) {
    assert!(
        command(app, action, extra).is_none(),
        "preparation was not queued"
    );
    let runtime = app.ott.as_mut().unwrap();
    let transient::Candidate::Plex(candidate) = runtime
        .transient
        .candidate
        .as_ref()
        .expect("Plex candidate")
    else {
        panic!("shared slot did not contain the Plex candidate")
    };
    let key = (candidate.work.owner, candidate.work.purpose.invocation().unwrap().id.clone());
    assert!(runtime
        .resolver_next
        .take()
        .expect("existing resolver request")
        .plex
        .is_some());
    key
}

/// State after a successful native admission, using the same production
/// projection as the completion handler. Only native identity is injected;
/// no decoder or PMS request is launched, as in plex_failure.rs.
fn seed_queue(app: &mut App, sid: plx_plex::plex::ServerId, ids: &[&str]) {
    seed_selection(app, sid, ids, selection(ids));
}
fn seed_selection(app: &mut App, sid: plx_plex::plex::ServerId, ids: &[&str], selection: prepare::Selection) {
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| {
            p.kind == ProviderKind::Plex
                && p.source == plx_plex::plex::client_for(sid).unwrap().machine_id()
        })
        .unwrap()
        .clone();
    let owner = OwnerToken(runtime.generation);
    let prepared = proposal(
        owner,
        sid,
        prepare::Prepared::Queue {
            kind: queue::PrepareKind::Play,
            selection,
        },
    );
    let (mut session, catalog) =
        transient::plex_remote::build_session(profile.clone(), prepared).unwrap();
    let token = queue::PlaybackToken(runtime.play_generation);
    session
        .queue
        .begin(queue::OwnerToken(owner.0), token, 0)
        .unwrap();
    assert!(session.queue.observe(
        queue::OwnerToken(owner.0),
        token,
        queue::PlaybackPhase::Playing
    ));
    let first = catalog.entries[0].clone();
    runtime.catalog = catalog;
    runtime.selected = Some(first.id.clone());
    runtime.transient.accepted = Some(transient::TransientMedia::Plex(session));
    runtime.continuity.stop(runtime.generation);
    install_native_identity(app, sid, ids[0]);
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: Some(owner),
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        selection_id: first.id.clone(),
        entry: first,
        source: None,
        advance_on_failure: false,
        requires_server_lease: false,
    });
    let installed = runtime.playing.clone().unwrap();
    app.ott = Some(runtime);
    assert!(playing_matches(&installed, app));
}

fn queue_value(app: &App) -> Value {
    let runtime = app.ott.as_ref().unwrap();
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref() else {
        panic!("accepted Plex queue")
    };
    session.queue.snapshot(runtime.remote.runtime()).unwrap()
}

fn kiosk_entries(app: &App, machine: &str, ids: &[&str]) -> Vec<Favorite> {
    let profile = app
        .ott
        .as_ref()
        .unwrap()
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| p.kind == ProviderKind::Plex && p.source == machine)
        .unwrap();
    ids.iter()
        .map(|id| {
            let mut entry = live_entry();
            entry.title = format!("Film {id}");
            entry.media = MediaLocator::Plex {
                server_id: machine.into(),
                rating_key: (*id).into(),
            };
            selection::capture(profile, &entry).unwrap()
        })
        .collect()
}

fn begin_kiosk_restore(app: &mut App, entries: &[Favorite]) -> OwnerToken {
    use crate::app::ott_kiosk_stored as stored;
    let mut runtime = app.ott.take().unwrap();
    runtime.resume_source_work();
    let target = stored::Stored::queue(entries.to_vec(), true).unwrap();
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), target.encode().unwrap());
    transient::kiosk_restore_queue(&mut runtime, app, entries).unwrap();
    let Some(transient::Candidate::Plex(candidate)) = runtime.transient.candidate.as_ref() else {
        panic!("Plex restore candidate required")
    };
    assert!(candidate.work.purpose.invocation().is_none());
    assert!(runtime.pending_play.is_none());
    let owner = candidate.work.owner;
    assert!(runtime.resolver_next.take().unwrap().plex.is_some());
    app.ott = Some(runtime);
    owner
}

#[test]
fn kiosk_restore_binds_nonactive_server_without_a_remote_invocation_or_persistent_write() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let wanted = register_source(&mut app, "wanted");
    register_source(&mut app, "browsed");
    app.ott.as_mut().unwrap().saved.profiles.active_profile = Some("plex-browsed".into());
    let entries = kiosk_entries(&app, "wanted", &["42", "42", "7"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let before = saved_value(&app);
    let mut runtime = app.ott.take().unwrap();
    let Some(transient::Candidate::Plex(candidate)) = runtime.transient.candidate.as_ref() else {
        panic!("candidate required")
    };
    assert_eq!(candidate.work.authority.server, wanted);
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            wanted,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42", "42", "7"]),
            },
        )),
    );
    assert!(runtime.pending_play.is_none());
    assert_eq!(
        runtime.catalog.entries[0].id, "live",
        "preparation cannot publish the new catalog"
    );
    let request = runtime.play_target.as_ref().unwrap();
    let view = crate::app::ott_kiosk_stored::borrow_validated(&runtime.saved.settings)
        .unwrap()
        .unwrap();
    let crate::app::ott_kiosk_stored::BorrowedTarget::Queue(pins) = view.target() else {
        panic!("queue pin")
    };
    assert!(transient::kiosk_restore_allows(&runtime, &request.id, pins));
    assert!(!transient::kiosk_restore_allows(&runtime, "live", pins));
    // Inject only the same native admission verdict consumed by Job::Play.
    let response = transient::finish_native(&mut runtime, &mut app, Ok(()), None).unwrap();
    assert_eq!(response["status"], "ok");
    assert!(runtime.pending_play.is_none());
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref() else {
        panic!("accepted queue")
    };
    assert_eq!(
        session
            .queue
            .records()
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["42", "42", "7"]
    );
    assert_eq!(
        session.queue.snapshot(runtime.remote.runtime()).unwrap()["repeat"],
        "all"
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    app.ott = Some(runtime);
}

#[test]
fn changed_durable_pin_discards_a_late_restore_without_committing_its_catalog() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42", "7"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    runtime
        .saved
        .settings
        .remove(crate::app::ott_kiosk_stored::KEY);
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42", "7"]),
            },
        )),
    );
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.play_target.is_none() && runtime.pending_play.is_none());
    assert_eq!(runtime.catalog.entries[0].id, "live");
    app.ott = Some(runtime);
}

#[test]
fn restore_rejects_reordered_metadata_and_keeps_the_previously_accepted_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    seed_queue(&mut app, sid, &["9"]);
    let previous = app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .accepted
        .as_ref()
        .unwrap()
        .owner();
    let entries = kiosk_entries(&app, "wanted", &["42", "7"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["7", "42"]),
            },
        )),
    );
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == previous);
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn restore_cancellation_never_releases_the_real_resolver_early() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let cancel = match runtime.transient.candidate.as_ref().unwrap() {
        transient::Candidate::Plex(c) => c.work.cancel.clone(),
        _ => unreachable!(),
    };
    transient::plex_remote::cancel_candidate(&mut runtime, &mut app);
    assert!(cancel.load(Ordering::Acquire));
    assert!(
        runtime.resolver_busy,
        "only a real terminal event releases the resolver"
    );
    assert!(runtime.pending_play.is_none());
    runtime
        .tx
        .send(Job::PlexPrepared(
            owner,
            transient::plex_remote::PreparationId::KioskRestore,
            Ok(proposal(
                owner,
                sid,
                prepare::Prepared::Queue {
                    kind: queue::PrepareKind::Play,
                    selection: selection(&["42"]),
                },
            )),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().resolver_busy);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_none());
}

#[test]
fn late_restore_result_cannot_replace_a_newer_restore_with_the_same_typed_purpose() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let old = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::cancel_candidate(&mut runtime, &mut app);
    app.ott = Some(runtime);
    let new = begin_kiosk_restore(&mut app, &entries);
    assert!(old != new);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        old,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            old,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let Some(transient::Candidate::Plex(candidate)) = runtime.transient.candidate.as_ref() else {
        panic!("new candidate removed")
    };
    assert!(candidate.work.owner == new);
    assert!(candidate.staged.is_none());
    assert_eq!(runtime.catalog.entries[0].id, "live");
    app.ott = Some(runtime);
}

#[test]
fn failed_restore_native_admission_keeps_the_previous_accepted_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    seed_queue(&mut app, sid, &["9"]);
    let previous = app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .accepted
        .as_ref()
        .unwrap()
        .owner();
    let entries = kiosk_entries(&app, "wanted", &["42", "42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42", "42"]),
            },
        )),
    );
    let reply =
        transient::finish_native(&mut runtime, &mut app, Err("fixture native refusal"), None).unwrap();
    assert_eq!(reply["status"], "rejected");
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == previous);
    assert!(runtime.pending_play.is_none());
    app.ott = Some(runtime);
}

#[test]
fn legacy_control_cannot_mutate_an_oversized_queue_before_rejecting_its_state() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "large");
    let ids = vec!["42"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    let before_saved = saved_value(&app);
    let before_generation = app.ott.as_ref().unwrap().play_generation;
    let before_identity = plx_media::route::playback_identity(&app.player.session);
    for op in ["status", "next", "previous", "stop"] {
        let response = command(&mut app, "plex_queue", json!({"op":op})).unwrap();
        assert_eq!(response["status"], "rejected");
        assert!(response["data"]["error"]
            .as_str()
            .unwrap()
            .contains("state_version: 2"));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_generation, before_generation);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref()
        else {
            panic!("queue lost")
        };
        assert_eq!(session.queue.index(), 0);
        assert_eq!(session.queue.records().len(), 1000);
        assert_eq!(
            plx_media::route::playback_identity(&app.player.session),
            before_identity
        );
    }
    let response = command(
        &mut app,
        "plex_queue",
        json!({"op":"status","state_version":2}),
    )
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["version"], 2);
    assert_eq!(response["data"]["ids"].as_array().unwrap().len(), 1000);
    assert_eq!(saved_value(&app), before_saved);
}

#[test]
fn negotiated_play_completion_keeps_its_version_across_an_interleaved_legacy_status() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "requested");
    let (owner, id) = begin_plex(
        &mut app,
        "plex_queue",
        json!({"op":"play","ids":["42"],"state_version":2}),
    );
    let response = command(&mut app, "plex_queue", json!({"op":"status"})).unwrap();
    assert_eq!(response["data"]["version"], 1);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let reply = transient::finish_native(&mut runtime, &mut app, Ok(()), None).unwrap();
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["data"]["version"], 2);
    app.ott = Some(runtime);
}

#[test]
fn negotiated_stop_of_an_oversized_queue_returns_version_two_idle() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "large");
    let ids = vec!["42"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    let response = command(
        &mut app,
        "plex_queue",
        json!({"op":"stop","state_version":2}),
    )
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["version"], 2);
    assert_eq!(response["data"]["state"], "idle");
    assert_eq!(response["data"]["ids"], json!([]));
}

#[test]
fn idle_queue_status_does_not_replace_permanent_catalog_or_release_a_source_lease() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let before = saved_value(&app);
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());

    let reply = command(&mut app, "plex_queue", json!({"op":"status"}))
        .expect("status must be synchronous");

    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["data"]["version"], 1);
    assert_eq!(reply["data"]["active"], false);
    assert_eq!(reply["data"]["state"], "idle");
    assert_eq!(saved_value(&app), before);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries[0].id, "live");
    assert_eq!(runtime.active().unwrap().id, "permanent");
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime
        .active_lease
        .as_ref()
        .is_some_and(HeldLease::is_playback));
    assert!(
        released.try_recv().is_err(),
        "status consumed the live lease"
    );
}

#[test]
fn malformed_queue_request_does_not_cancel_or_reconfigure_current_media() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let before = saved_value(&app);
    let cancel = app.ott.as_ref().unwrap().play_cancel.clone();
    let generation = app.ott.as_ref().unwrap().play_generation;
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());

    let reply = command(&mut app, "plex_queue", json!({"op":"play","ids":["42",0]}))
        .expect("malformed request must fail before asynchronous preparation");

    assert_ne!(reply["status"], "ok");
    assert_eq!(saved_value(&app), before);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert!(runtime.transient.candidate.is_none() && runtime.transient.accepted.is_none());
    assert!(
        released.try_recv().is_err(),
        "invalid input consumed the live lease"
    );
}

#[test]
fn malformed_plex_request_does_not_supersede_an_admitted_local_candidate() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    assert!(command(
        &mut app,
        "local_media",
        json!({"op":"play","source":"/fixture/Movies","session":true,"progressive":true}),
    )
    .is_none());
    let before = saved_value(&app);
    let current_candidate = candidate_id(&app);
    let runtime = app.ott.as_ref().unwrap();
    let generation = runtime.generation;

    let rejected = command(&mut app, "plex_queue", json!({"op":"play","ids":[]}))
        .expect("invalid queue must be rejected before changing ownership");

    assert_ne!(rejected["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert_eq!(candidate_id(&app), current_candidate);
    assert!(runtime.transient.accepted.is_none());
    assert_eq!(saved_value(&app), before);
}

#[test]
fn unavailable_selected_plex_source_does_not_fall_back_to_another_registered_server() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let other = register_source(&mut app, "other-machine");
    let selected = register_source(&mut app, "selected-machine");
    plx_plex::plex::client_for(selected)
        .unwrap()
        .disable_data_io();
    assert!(!plx_plex::plex::client_for(other)
        .unwrap()
        .data_io_disabled());
    let before = saved_value(&app);

    let reply = command(&mut app, "plex_queue", json!({"op":"preview","ids":["42"]}))
        .expect("retired selected source must fail before launching a worker");

    assert_eq!(reply["data"]["state"], "error", "{reply}");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none() && runtime.transient.accepted.is_none());
    assert!(runtime.resolver_next.is_none());
    assert_eq!(runtime.catalog.entries[0].id, "live");
    assert_eq!(runtime.active().unwrap().id, "permanent");
    assert_eq!(saved_value(&app), before);
}

#[test]
fn preview_and_library_list_keep_current_lease_catalog_and_play_ticket() {
    let _serial = plx_base::testlock::serial();
    for library_list in [false, true] {
        let _reset = Reset;
        let mut app = setup();
        let sid = register_source(&mut app, "selected-machine");
        let before = saved_value(&app);
        let play_generation = app.ott.as_ref().unwrap().play_generation;
        let ticket = plx_media::route::play_request_ticket();
        let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
        app.ott.as_mut().unwrap().active_lease = Some(lease.into());
        let (owner, id) = if library_list {
            begin_plex(&mut app, "plex_library", json!({"op":"list"}))
        } else {
            begin_plex(
                &mut app,
                "plex_queue",
                json!({"op":"preview","ids":["42","42"]}),
            )
        };
        let Some(transient::Candidate::Plex(candidate)) =
            app.ott.as_ref().unwrap().transient.candidate.as_ref()
        else {
            panic!("Plex preparation candidate")
        };
        assert_eq!(candidate.work.authority.server, sid);
        assert_eq!(candidate.work.profile.source, "selected-machine");
        assert_eq!(app.ott.as_ref().unwrap().active().unwrap().id, "permanent");
        assert!(
            released.try_recv().is_err(),
            "read-only preparation took the live lease"
        );
        let prepared = if library_list {
            prepare::Prepared::Libraries {
                rows: vec![queue::LibraryRow {
                    id: "17".into(),
                    title: "Movies".into(),
                    kind: queue::LibraryKind::Movie,
                }],
            }
        } else {
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Preview,
                selection: selection(&["42", "42"]),
            }
        };
        app.ott
            .as_ref()
            .unwrap()
            .tx
            .send(Job::PlexPrepared(
                owner,
                transient::plex_remote::PreparationId::Remote(id),
                Ok(proposal(owner, sid, prepared)),
            ))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.transient.candidate.is_none() && runtime.transient.accepted.is_none());
        assert!(runtime
            .active_lease
            .as_ref()
            .is_some_and(HeldLease::is_playback));
        assert!(
            !runtime.resolver_busy,
            "read-only completion retained the worker slot"
        );
        assert_eq!(runtime.play_generation, play_generation);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(runtime.catalog.entries[0].id, "live");
        assert!(plx_media::route::play_request_is_current(ticket));
        assert!(
            released.try_recv().is_err(),
            "preview completion took the live lease"
        );
        assert_eq!(saved_value(&app), before);
    }
}

#[test]
fn repeated_rating_keys_have_distinct_occurrences_and_eof_is_consumed_once() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    let before = saved_value(&app);
    seed_queue(&mut app, sid, &["42", "42", "43"]);
    let rows: Vec<_> = app
        .ott
        .as_ref()
        .unwrap()
        .catalog
        .entries
        .iter()
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_ne!(
        rows[0], rows[1],
        "duplicate media IDs collapsed distinct queue positions"
    );
    assert_eq!(queue_value(&app)["ids"], json!(["42", "42", "43"]));

    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_target.as_ref().map(|r| r.id.as_str()),
        Some(rows[1].as_str())
    );
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    let generation = runtime.play_generation;
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_generation, generation,
        "repeated EOF admitted another occurrence"
    );
    assert_eq!(
        runtime.play_target.as_ref().map(|r| r.id.as_str()),
        Some(rows[1].as_str())
    );
    assert_eq!(saved_value(&app), before);
}

#[test]
fn one_item_repeat_none_ends_without_replaying_or_starting_permanent_provider() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    let before = saved_value(&app);
    seed_queue(&mut app, sid, &["42"]);
    assert_eq!(queue_value(&app)["repeat"], "none");

    assert!(finish_catalog_playback(&mut app));
    assert!(finish_catalog_playback(&mut app));

    assert_eq!(queue_value(&app)["state"], "ended");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert_eq!(runtime.active().unwrap().id, "permanent");
    assert_eq!(runtime.saved.category, "News");
    assert!(runtime.checkpoints.is_empty());
    assert_eq!(saved_value(&app), before);
}

#[test]
fn queue_boundaries_do_not_admit_another_request_or_drop_the_current_item() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42"]);
    let before = saved_value(&app);
    let current = app
        .ott
        .as_ref()
        .unwrap()
        .playing
        .as_ref()
        .unwrap()
        .selection_id
        .clone();
    for op in ["previous", "next"] {
        let response =
            command(&mut app, "plex_queue", json!({"op":op})).expect("boundary response");
        assert_eq!(response["data"]["state"], "error", "{response}");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.playing.as_ref().unwrap().selection_id, current);
        assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    }
    assert_eq!(queue_value(&app)["index"], 0);
    assert_eq!(saved_value(&app), before);
}

#[test]
fn native_queue_failure_stops_at_the_exact_occurrence_without_skipping() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    let before = saved_value(&app);
    plx_media::route::refuse_for_test(
        &mut app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    // A coincident EOF must not consume this failed queue occurrence as success.
    plx_media::player::SHARED.ended.store(true, Ordering::Release);
    let queue_before_error = queue_value(&app);
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(queue_value(&app), queue_before_error,
        "Error+EOF must preserve the queue ticket for failure handling");
    let mut runtime = app.ott.take().unwrap();
    runtime.reconcile_failure(&mut app);
    app.ott = Some(runtime);
    assert_eq!(queue_value(&app)["state"], "error");
    assert_eq!(queue_value(&app)["index"], 0);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert_eq!(saved_value(&app), before);
}

#[test]
fn local_and_plex_requests_use_one_pending_candidate_slot() {
    let _serial = plx_base::testlock::serial();
    for local_first in [false, true] {
        let _reset = Reset;
        let mut app = setup();
        register_source(&mut app, "selected-machine");
        let before = saved_value(&app);
        let local =
            json!({"op":"play","source":"/fixture/Movies","session":true,"progressive":true});
        let plex = json!({"op":"play","ids":["42"]});
        if local_first {
            assert!(command(&mut app, "local_media", local.clone()).is_none());
        } else {
            begin_plex(&mut app, "plex_queue", plex.clone());
        }
        let original_id = candidate_id(&app);
        let reply = if local_first {
            command(&mut app, "plex_queue", plex)
        } else {
            command(&mut app, "local_media", local)
        }
        .expect("occupied common slot must produce an immediate result");
        if local_first {
            assert_eq!(reply, rejected(queue::QueueError::Busy.message()));
        } else {
            assert_eq!(reply["data"]["state"], "error", "{reply}");
        }
        assert_eq!(candidate_id(&app), original_id);
        assert_eq!(
            matches!(
                app.ott.as_ref().unwrap().transient.candidate,
                Some(transient::Candidate::Local(_))
            ),
            local_first
        );
        assert!(app.ott.as_ref().unwrap().transient.accepted.is_none());
        assert_eq!(saved_value(&app), before);
    }
}

#[test]
fn failed_or_cancelled_preparation_completion_releases_the_existing_worker_slot() {
    let _serial = plx_base::testlock::serial();
    for (cancel_first, successful_late_reply) in [(false, false), (true, false), (true, true)] {
        let _reset = Reset;
        let mut app = setup();
        let sid = register_source(&mut app, "selected-machine");
        let before = saved_value(&app);
        let (owner, id) = begin_plex(&mut app, "plex_queue", json!({"op":"preview","ids":["42"]}));
        if cancel_first {
            command(&mut app, "plex_queue", json!({"op":"stop"})).expect("stop receipt");
            assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
        }
        let result = if successful_late_reply {
            Ok(proposal(
                owner,
                sid,
                prepare::Prepared::Queue {
                    kind: queue::PrepareKind::Preview,
                    selection: selection(&["42"]),
                },
            ))
        } else {
            Err(queue::WireError::Queue(
                queue::QueueError::ServerUnavailable,
            ))
        };
        app.ott
            .as_ref()
            .unwrap()
            .tx
            .send(Job::PlexPrepared(owner, transient::plex_remote::PreparationId::Remote(id), result))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(
            !runtime.resolver_busy,
            "every worker terminal event must release its slot"
        );
        assert!(runtime.transient.candidate.is_none() && runtime.transient.accepted.is_none());
        assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
        assert_eq!(runtime.catalog.entries[0].id, "live");
        assert_eq!(saved_value(&app), before);
    }
}

#[test]
fn shutdown_discards_an_already_queued_plex_proposal_without_installing_it() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    let (owner, id) = begin_plex(&mut app, "plex_queue", json!({"op":"play","ids":["42"]}));
    let cancel = match app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .candidate
        .as_ref()
        .unwrap()
    {
        transient::Candidate::Plex(candidate) => candidate.work.cancel.clone(),
        _ => panic!("Plex candidate"),
    };
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::PlexPrepared(
            owner,
            transient::plex_remote::PreparationId::Remote(id),
            Ok(proposal(
                owner,
                sid,
                prepare::Prepared::Queue {
                    kind: queue::PrepareKind::Play,
                    selection: selection(&["42"]),
                },
            )),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    super::super::shutdown_with_budget(&mut app, Duration::from_millis(20));
    assert!(app.ott.is_none());
    assert!(
        cancel.load(Ordering::Acquire),
        "shutdown retained preparation authority"
    );
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(app.bridge.metadata_view().now_playing().is_none());
}

#[test]
fn exact_first_item_failure_and_late_authority_retirement_never_skip_or_commit_the_candidate() {
    let _serial = plx_base::testlock::serial();
    for retire_authority in [false, true] {
        let _reset = Reset;
        let mut app = setup();
        let sid = register_source(&mut app, "selected-machine");
        let before = saved_value(&app);
        let (owner, id) = begin_plex(
            &mut app,
            "plex_queue",
            json!({"op":"play","ids":["42","43"]}),
        );
        let mut runtime = app.ott.take().unwrap();
        transient::plex_remote::prepared(
            &mut runtime,
            &mut app,
            owner,
            transient::plex_remote::PreparationId::Remote(id),
            Ok(proposal(
                owner,
                sid,
                prepare::Prepared::Queue {
                    kind: queue::PrepareKind::Play,
                    selection: selection(&["42", "43"]),
                },
            )),
        );
        let mut next = runtime
            .resolver_next
            .take()
            .expect("first native item request");
        assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
        assert!(
            runtime.transient.accepted.is_none(),
            "prefix preparation is not native admission"
        );
        let result = if retire_authority {
            plx_plex::plex::client_for(sid).unwrap().disable_data_io();
            Ok((
                next.entry.take().unwrap(),
                ReadySource::PreparedPlex(next.plex_choice.take().unwrap()),
            ))
        } else {
            Err("synthetic first-item failure")
        };
        runtime
            .tx
            .send(Job::Play(next.generation, next.profile, result))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        app.ott = Some(runtime);
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.transient.accepted.is_none() && runtime.transient.candidate.is_none());
        assert!(runtime.playing.is_none());
        assert!(
            runtime.resolver_next.is_none() && runtime.play_target.is_none(),
            "failed exact item skipped to its successor"
        );
        assert_eq!(runtime.catalog.entries[0].id, "live");
        assert_eq!(saved_value(&app), before);
        assert!(app.bridge.metadata_view().now_playing().is_none());
    }
}

#[test]
fn foreign_server_favorite_with_same_rating_key_cannot_consume_queue_eof_or_be_stopped_as_queue_media(
) {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let queue_sid = register_source(&mut app, "queue-machine");
    let favorite_sid = register_source(&mut app, "favorite-machine");
    seed_queue(&mut app, queue_sid, &["42", "43"]);
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| p.source == "favorite-machine")
        .unwrap()
        .clone();
    let mut entry = runtime.catalog.entries[0].clone();
    entry.id = "foreign-42".into();
    entry.media = MediaLocator::Plex {
        server_id: "favorite-machine".into(),
        rating_key: "42".into(),
    };
    let favorite = selection::capture(&profile, &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    // Global Favorites resumes are durable even over a temporary Plex queue.
    // The foreign queue must neither add its own key nor clear this bookmark.
    let expected_checkpoints = json!({"favorites": {
        "entry_id": selected.clone(),
        "position_ms": 23_000,
        "source_fingerprint": profile_fingerprint(&profile),
    }});
    runtime.saved.favorites.toggle(favorite).unwrap();
    // The queue predates the global preference; its private compatibility flag
    // deliberately stays false. EOF must consult the durable global selection.
    runtime.saved.favorites_selected = true;
    install_native_identity(&mut app, favorite_sid, "42");
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: runtime
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner),
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry,
        selection_id: selected.clone(),
        source: None,
        advance_on_failure: false,
        requires_server_lease: false,
    });
    runtime.selected = Some(selected.clone());
    // This is the ordinary checkpoint boundary used after the favorite has
    // actually been admitted, not a direct mutation of the Plex queue state.
    plx_media::player::SHARED
        .playpos_ns
        .store(23_000_000_000, Ordering::Release);
    runtime.checkpoint(&app, true);
    assert_eq!(serde_json::to_value(&runtime.checkpoints).unwrap(), expected_checkpoints);
    assert!(!transient::plex_remote::ended(&mut runtime, &mut app),
        "global Favorites delegates borrowed EOF to the common Favorites list");
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    app.ott = Some(runtime);
    assert_eq!(queue_value(&app)["index"], 0);
    assert_eq!(queue_value(&app)["state"], "paused");
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().checkpoints).unwrap(),
        expected_checkpoints,
        "foreign EOF changed the admitted Favorite's resume",
    );
    assert_eq!(
        queue_value(&app)["index"],
        0,
        "foreign favorite consumed the accepted queue's EOF"
    );
    let reply = command(&mut app, "plex_queue", json!({"op":"stop"})).expect("stop receipt");
    assert_eq!(reply["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(plx_media::route::cur_sid(&app.player.session), favorite_sid);
    assert_eq!(
        runtime.playing.as_ref().unwrap().selection_id,
        selected,
        "queue Stop cancelled unrelated admitted favorite media"
    );
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert_eq!(
        serde_json::to_value(&runtime.checkpoints).unwrap(),
        expected_checkpoints,
        "queue Stop changed the admitted Favorite's resume",
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn unrelated_saved_settings_and_queue_stop_preserve_permanent_profile_category_and_favorites() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    let favorite = selection::capture(&permanent(), &live_entry()).unwrap();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .favorites
        .toggle(favorite)
        .unwrap();
    seed_queue(&mut app, sid, &["42", "43"]);
    transient::set_view(app.ott.as_mut().unwrap(), "Movies", false);
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.settings.insert("test-setting".into(), json!(1));
    let expected = serde_json::to_value(&saved).unwrap();
    app.ott.as_mut().unwrap().saving = true;
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(saved),
            None,
            false,
            ok(json!({"persisted":true})),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(
        app.ott.as_ref().unwrap().view_selection(),
        ("Movies", false)
    );
    assert_eq!(saved_value(&app), expected);

    let response = command(&mut app, "plex_queue", json!({"op":"stop"})).expect("stop response");
    assert_eq!(response["status"], "ok");
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none());
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "stop autostarted a permanent profile"
    );
    assert_eq!(runtime.active().unwrap().id, "permanent");
    assert_eq!(runtime.saved.category, "News");
    assert!(runtime.checkpoints.is_empty() && runtime.saved.resume.is_empty());
    assert_eq!(saved_value(&app), expected);
}

#[test]
fn failed_next_preserves_the_previously_admitted_cursor_and_its_eof_ownership() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    let first = app.ott.as_ref().unwrap().catalog.entries[0].id.clone();
    let second = app.ott.as_ref().unwrap().catalog.entries[1].id.clone();
    let before = saved_value(&app);
    assert!(command(&mut app, "plex_queue", json!({"op":"next"})).is_none());
    let runtime = app.ott.as_mut().unwrap();
    let next = runtime
        .resolver_next
        .take()
        .expect("second occurrence request");
    assert_eq!(runtime.play_target.as_ref().unwrap().id, second);
    runtime
        .tx
        .send(Job::Play(
            next.generation,
            next.profile,
            Err("synthetic replacement refusal"),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(
        queue_value(&app)["index"],
        0,
        "failed replacement committed its cursor"
    );
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, first);
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "failure skipped automatically"
    );
    runtime.resolver_busy = true;
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some(second.as_str()),
        "the still-playing first occurrence lost its EOF ownership after replacement failed"
    );
    assert_eq!(saved_value(&app), before);
}

#[test]
fn late_save_from_before_explicit_plex_play_cannot_restore_the_permanent_catalog_over_it() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "selected-machine");
    let runtime = app.ott.as_mut().unwrap();
    // State captured by the existing save admission; no protected writer runs.
    runtime.saving = true;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
    let prior_epoch = runtime.playback_epoch;
    let saved = runtime.saved.clone();
    let (owner, id) = begin_plex(&mut app, "plex_queue", json!({"op":"play","ids":["42"]}));
    assert_ne!(
        app.ott.as_ref().unwrap().playback_epoch,
        prior_epoch,
        "explicit Plex play did not fence a preexisting save's playback intent"
    );
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(saved),
            None,
            true,
            ok(json!({"persisted":true})),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let Some(transient::Candidate::Plex(candidate)) = runtime.transient.candidate.as_ref() else {
        panic!("late Saved replaced the new Plex candidate")
    };
    assert!(candidate.work.owner == owner);
    assert_eq!(candidate.work.purpose.invocation().unwrap().id, id);
    assert!(
        runtime.catalog_next.is_none(),
        "late save restarted the permanent catalog"
    );
}

#[test]
fn local_to_plex_replaces_the_same_accepted_state_only_after_injected_native_success() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = super::local_integration::setup();
    let sid = register_source(&mut app, "selected-machine");
    let before = saved_value(&app);
    assert!(matches!(
        app.ott.as_ref().unwrap().transient.accepted,
        Some(transient::TransientMedia::Local(_))
    ));
    let (owner, id) = begin_plex(
        &mut app,
        "plex_queue",
        json!({"op":"play","ids":["42","43"]}),
    );
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42", "43"]),
            },
        )),
    );
    assert!(
        matches!(
            runtime.transient.accepted,
            Some(transient::TransientMedia::Local(_))
        ),
        "successful metadata preparation prematurely retired the admitted Local session"
    );
    assert!(matches!(
        runtime.transient.candidate,
        Some(transient::Candidate::Plex(_))
    ));
    assert_eq!(runtime.catalog.entries[0].id, "first");
    // Inject only the native admission verdict at the same finisher used by
    // Job::Play. This test does not claim that a decoder was actually started.
    let response =
        transient::finish_native(&mut runtime, &mut app, Ok(()), None).expect("admission response");
    assert_eq!(response["status"], "ok");
    assert!(runtime.transient.candidate.is_none());
    assert!(matches!(
        runtime.transient.accepted,
        Some(transient::TransientMedia::Plex(_))
    ));
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == owner);
    assert_eq!(runtime.catalog.entries.len(), 2);
    assert!(runtime
        .catalog
        .entries
        .iter()
        .all(|row| row.id.starts_with("plex-occurrence:")));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.active().unwrap().id, "permanent");
    app.ott = Some(runtime);
}

#[test]
fn failed_local_replacement_keeps_the_accepted_plex_state_and_native_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    let before = saved_value(&app);
    let old_owner = app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .accepted
        .as_ref()
        .unwrap()
        .owner();
    let old_entry = app
        .ott
        .as_ref()
        .unwrap()
        .playing
        .as_ref()
        .unwrap()
        .selection_id
        .clone();
    assert!(command(
        &mut app,
        "local_media",
        json!({"op":"play","source":"/missing/Movies","session":true,"progressive":true})
    )
    .is_none());
    let generation = app.ott.as_ref().unwrap().generation;
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::LocalBindings(
            generation,
            Err(crate::app::ott_local_session::LocalError::SourceUnavailable),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(matches!(
        runtime.transient.accepted,
        Some(transient::TransientMedia::Plex(_))
    ));
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == old_owner);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, old_entry);
    assert_eq!(plx_media::route::cur_sid(&app.player.session), sid);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "42");
    assert_eq!(queue_value(&app)["index"], 0);
    assert_eq!(saved_value(&app), before);
}

#[test]
fn old_queue_eof_cannot_replace_a_newer_explicit_occurrence_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43", "44"]);
    let third = app.ott.as_ref().unwrap().catalog.entries[2].id.clone();
    explicit_play(&mut app, &third);
    let generation = app.ott.as_ref().unwrap().play_generation;
    assert_eq!(
        app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id,
        third
    );

    assert!(finish_catalog_playback(&mut app));

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_generation, generation,
        "old EOF superseded newer viewer intent"
    );
    assert_eq!(runtime.play_target.as_ref().unwrap().id, third);
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .unwrap()
            .entry
            .as_ref()
            .unwrap()
            .id,
        third
    );
}

#[test]
fn failed_automatic_successor_settles_as_error_instead_of_staying_preparing_forever() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_mut().unwrap();
    let successor = runtime.resolver_next.take().expect("automatic successor");
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    runtime
        .tx
        .send(Job::Play(
            successor.generation,
            successor.profile,
            Err("synthetic successor failure"),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);

    assert_eq!(queue_value(&app)["state"], "error");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert!(runtime.checkpoints.is_empty());
}

#[test]
fn directly_opened_foreign_favorite_eof_cannot_wrap_into_the_retained_plex_catalog() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let queue_sid = register_source(&mut app, "queue-machine");
    let favorite_sid = register_source(&mut app, "favorite-machine");
    seed_queue(&mut app, queue_sid, &["42", "43"]);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime
        .saved
        .profiles
        .profiles
        .iter()
        .find(|p| p.source == "favorite-machine")
        .unwrap()
        .clone();
    let mut entry = runtime.catalog.entries[0].clone();
    entry.id = "foreign-42".into();
    entry.media = MediaLocator::Plex {
        server_id: "favorite-machine".into(),
        rating_key: "42".into(),
    };
    let favorite = selection::capture(&profile, &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();

    // The real selection path arms Continuity for a direct Favorite without
    // changing the queue's category view into the Favorites category.
    explicit_play(&mut app, &selected);
    let mut runtime = app.ott.take().unwrap();
    assert!(!runtime.view_selection().1);
    assert!(
        runtime
            .continuity
            .accepts_end(runtime.generation, &selected),
        "premise: ordinary continuity must be active, not suppressed by the fixture"
    );
    let request = runtime
        .resolver_next
        .take()
        .expect("direct Favorite resolution");
    assert!(request.favorite.is_some());
    let owner = runtime.play_target.as_ref().unwrap().transient_owner;
    // Supply the admitted native identity at the same boundary as the existing
    // plex_failure fixture, then commit the actual transient admission hook.
    install_native_identity(&mut app, favorite_sid, "42");
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: owner,
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry,
        selection_id: selected,
        source: None,
        advance_on_failure: false,
        requires_server_lease: false,
    });
    transient::plex_remote::admitted(&mut runtime);
    runtime.play_target = None;
    runtime.checkpoint(&app, true);
    app.ott = Some(runtime);

    assert!(finish_catalog_playback(&mut app));

    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "a Favorite outside the queue wrapped into the first retained Plex row"
    );
    assert_eq!(queue_value(&app)["index"], 0);
    assert!(runtime.checkpoints.is_empty());
    assert_eq!(saved_value(&app), before);
}

#[test]
fn global_favorite_borrowed_from_plex_keeps_resume_without_persisting_the_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    let mut runtime = app.ott.take().unwrap();
    let playing = runtime.playing.as_ref().unwrap().clone();
    let favorite = selection::capture(&playing.profile, &playing.entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.saved.favorites_selected = true;
    runtime.playing.as_mut().unwrap().selection_id = selected.clone();
    runtime.continuity.user_selected(runtime.generation, &selected);
    runtime.saving = true;
    plx_media::player::SHARED.playpos_ns.store(32_000_000_000, Ordering::Release);
    runtime.checkpoint(&app, true);
    assert_eq!(runtime.checkpoints["favorites"].entry_id, selected);
    assert_eq!(runtime.checkpoints["favorites"].position_ms, 32_000);
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("permanent"));
    assert!(!runtime.checkpoints.contains_key(&playing.profile.id));
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref()
        else { panic!("accepted Plex queue"); };
    assert_eq!(session.borrowed.as_ref().unwrap().position_ms, 32_000);
    app.ott = Some(runtime);
}


/// A real external admission without a decoder start. The retained lease is
/// installed separately, so these fixtures can model both sides of its handoff.
fn lease_fixture_source(app: &mut App, id: &str, requires_server_lease: bool) {
    playback::replace_player(
        &mut app.player, &mut app.adapters.player, &mut app.repause_at,
        &mut app.refresh_hubs, &mut app.pages,
    );
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let source = plx_media::ff::ExternalSource::Http(
        plx_data::ott::http_url(&format!("https://media.invalid/{id}")).unwrap(),
    );
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    plx_media::route::request_external_source(
        &mut app.player.session, &info, source.clone(), id, "Fixture",
        plx_platform::devcaps::assumed_shared(),
    ).unwrap();
    let mut entry = live_entry();
    entry.id = id.into();
    entry.media = MediaLocator::Http { url: format!("https://media.invalid/{id}") };
    app.ott.as_mut().unwrap().playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile: permanent(), entry, selection_id: id.into(), source: Some(source),
        advance_on_failure: false, requires_server_lease,
        transient_owner: None, local_selection: None,
    });
    assert!(playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), app));
}

fn prepare_lease_fixture_choice(app: &mut App, sid: plx_plex::plex::ServerId) {
    let (owner, id) = begin_plex(app, "plex_queue", json!({"op":"play","ids":["42"]}));
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime, app, owner, transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(owner, sid, prepare::Prepared::Queue {
            kind: queue::PrepareKind::Play, selection: selection(&["42"]),
        })),
    );
    assert!(runtime.resolver_next.as_ref().is_some_and(|request| request.plex_choice.is_some()));
    app.ott = Some(runtime);
}

/// Always unblock an actual fixture worker, even if a RED assertion panics.
struct FinishLeaseOnDrop(Option<std::sync::mpsc::Sender<()>>);
impl FinishLeaseOnDrop {
    fn finish(&mut self) {
        if let Some(sender) = self.0.take() { let _ = sender.send(()); }
    }
}
impl Drop for FinishLeaseOnDrop {
    fn drop(&mut self) { self.finish(); }
}

#[test]
fn prepared_plex_replacement_drains_previous_lease_before_posting_native_choice() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    lease_fixture_source(&mut app, "leased-old", true);
    let (lease, released, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let mut finish = FinishLeaseOnDrop(Some(finished));
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    prepare_lease_fixture_choice(&mut app, sid);
    assert!(released.try_recv().is_err(), "metadata preparation cannot stop current playback");
    let runtime = app.ott.as_mut().unwrap();
    runtime.resolver_busy = false;
    runtime.launch_resolver().unwrap();
    assert!(runtime.active_lease.is_none(), "replacement must transfer completion ownership to the worker");
    assert_eq!(released.recv_timeout(Duration::from_secs(2)), Ok(()));
    assert!(matches!(runtime.rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
        "native choice was posted before the old stop completed");
    finish.finish();
    let answer = runtime.rx.recv_timeout(Duration::from_secs(2)).expect("drained replacement result");
    match answer {
        Job::Play(generation, _, Ok((_, ReadySource::PreparedPlex(_)))) => {
            assert_eq!(generation, runtime.play_generation);
        }
        _ => panic!("expected one prepared native choice after the lease completion"),
    }
    runtime.resolver_busy = false;
    // This fixture intentionally stops before native/PMS admission.
}

#[test]
fn failed_prepared_plex_retires_only_a_matching_source_whose_lease_was_transferred() {
    let _serial = plx_base::testlock::serial();
    for transferred in [false, true] {
        let _reset = Reset;
        let mut app = setup();
        let sid = register_source(&mut app, "selected-machine");
        lease_fixture_source(&mut app, "leased-old", true);
        let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
        app.ott.as_mut().unwrap().active_lease = Some(lease.into());
        prepare_lease_fixture_choice(&mut app, sid);
        let mut runtime = app.ott.take().unwrap();
        // Model the existing worker handoff only; the barrier itself is tested above.
        if transferred { drop(runtime.active_lease.take()); }
        let response = transient::finish_native(&mut runtime, &mut app, Err("synthetic choice failure"), None);
        assert!(response.is_some());
        assert!(runtime.transient.candidate.is_none());
        if transferred {
            assert!(runtime.playing.is_none(), "a closed server session cannot remain a playback fallback");
            assert!(!plx_media::route::has_url(&app.player.session));
            assert_eq!(released.try_recv(), Ok(()));
        } else {
            assert!(runtime.playing.as_ref().is_some_and(|playing| playing_matches(playing, &app)));
            assert!(runtime.active_lease.as_ref().is_some_and(HeldLease::is_playback));
            assert!(released.try_recv().is_err(), "pre-handoff refusal must keep the current lease");
        }
        assert!(runtime.resolver_next.as_ref().is_some(), "fixture keeps the unexecuted choice explicit");
        app.ott = Some(runtime);
    }
}

#[test]
fn cancelled_prepared_plex_does_not_retire_a_newer_logical_source() {
    let _serial = plx_base::testlock::serial();
    for replaced in [false, true] {
        let _reset = Reset;
        let mut app = setup();
        let sid = register_source(&mut app, "selected-machine");
        lease_fixture_source(&mut app, "leased-old", true);
        prepare_lease_fixture_choice(&mut app, sid);
        // Retain the old Runtime owner as it is while a replacement is in flight.
        let old = app.ott.as_ref().unwrap().playing.clone().unwrap();
        if replaced {
            lease_fixture_source(&mut app, "newer", false);
            app.ott.as_mut().unwrap().playing = Some(old);
        }
        let identity = plx_media::route::playback_identity(&app.player.session);
        let mut runtime = app.ott.take().unwrap();
        transient::plex_remote::cancel_candidate(&mut runtime, &mut app);
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.play_target.is_none());
        if replaced {
            assert!(plx_media::route::has_url(&app.player.session));
            assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        } else {
            assert!(runtime.playing.is_none(), "matching closed-lease owner must retire on cancellation");
            assert!(!plx_media::route::has_url(&app.player.session));
        }
        app.ott = Some(runtime);
    }
}

#[test]
fn late_failed_prepared_plex_cannot_retire_a_newer_native_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    lease_fixture_source(&mut app, "leased-old", true);
    prepare_lease_fixture_choice(&mut app, sid);
    let old = app.ott.as_ref().unwrap().playing.clone().unwrap();
    lease_fixture_source(&mut app, "newer", false);
    app.ott.as_mut().unwrap().playing = Some(old);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let mut runtime = app.ott.take().unwrap();
    assert!(transient::finish_native(&mut runtime, &mut app, Err("stale prepared choice"), None).is_some());
    assert!(plx_media::route::has_url(&app.player.session));
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    app.ott = Some(runtime);
}


#[test]
fn observed_local_root_loss_preserves_the_accepted_plex_queue_and_native_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let root_dir = super::local_integration::TempDir::new();
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    seed_queue(&mut app, sid, &["42", "43"]);
    let before = queue_value(&app);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let selected = app.ott.as_ref().unwrap().selected.clone();
    let mut runtime = app.ott.take().unwrap();
    let owner = runtime.transient.accepted.as_ref().unwrap().owner();
    runtime.page = "channels";
    assert_eq!(runtime.search_view().unwrap().scope, "Plex selected-machine");
    let root = crate::app::ott_local_session::BoundSource {
        // Even a colliding profile label is not authority over a Plex session.
        id: "plex-selected-machine".into(), name: "Local folder".into(),
        kind: crate::app::ott_local_session::SourceKind::Folder,
        available: true, authority_ref: crate::app::ott_local_session::AuthorityRef(1),
        generation: runtime.generation, canonical_path: Some(root_dir.folder("Local")),
    };
    runtime.local_inventory.install(vec![super::local_integration::acquire_root(root.clone())], runtime.generation);
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
    let generation = runtime.generation;
    let mut lost = root; lost.available = false;
    transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
        sources: vec![crate::app::ott_local_root::LocalSource::unavailable(lost)], lookup: None, matched: Vec::new(), operation_error: None,
    }));
    assert!(!runtime.local_inventory.sources[0].descriptor().available);
    assert!(matches!(runtime.transient.accepted, Some(transient::TransientMedia::Plex(_))));
    assert_eq!(runtime.transient.accepted.as_ref().unwrap().owner().0, owner.0);
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.catalog.entries.len(), 2);
    assert!(runtime.playing.as_ref().is_some_and(|playing| playing_matches(playing, &app)));
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    app.ott = Some(runtime);
    assert_eq!(queue_value(&app), before);
}
#[path = "plex_kiosk.rs"]
mod kiosk_hooks;

#[path = "plex_preparation_lifecycle.rs"]
mod preparation_lifecycle;
#[path = "kiosk_boot.rs"]
mod kiosk_boot;

#[test]
fn generic_channel_movement_keeps_single_channel_receipt_for_a_large_plex_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for (operation, from_index, index) in [("next_channel", 0, 1), ("previous_channel", 1, 0)] {
        let mut app = setup();
        app.ott.as_mut().unwrap().resume_source_work();
        let sid = register_source(&mut app, "large");
        let ids = vec!["42"; 1000];
        seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
        if from_index != 0 {
            // Seed an already admitted nonzero occurrence; manual previous at
            // index zero is deliberately a boundary error, not a TV-style wrap.
            let runtime = app.ott.as_mut().unwrap();
            let entry = runtime.catalog.entries[from_index].clone();
            let token = queue::PlaybackToken(runtime.play_generation);
            let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut() else {
                panic!("accepted Plex fixture");
            };
            session.queue.begin(session.queue.owner(), token, from_index).unwrap();
            assert!(session.queue.observe(session.queue.owner(), token, queue::PlaybackPhase::Playing));
            let playing = runtime.playing.as_mut().unwrap();
            playing.selection_id = entry.id.clone();
            playing.entry = entry;
            runtime.selected = Some(playing.selection_id.clone());
        }
        let before = saved_value(&app);
        let mut invocation = request(&app, "playback", json!({"operation":operation}));
        // The Plex helper adds its queue protocol's runtime field. Generic
        // playback commands have the strict {operation} shape instead.
        invocation.params = json!({"operation":operation});
        let mut runtime = app.ott.take().unwrap();
        let selected = runtime.catalog.entries[index].id.clone();
        // Exercise the real generic command -> select_step -> play_reply path,
        // not plex_queue's full queue-state response path.
        let response = commands::command(&mut runtime, &mut app, &invocation);
        assert!(response.is_none(), "{operation} was rejected: {response:?}");
        assert_eq!(runtime.play_target.as_ref().unwrap().id, selected);
        assert_eq!(runtime.pending_play.as_ref().unwrap().id, invocation.id);
        assert!(runtime.resolver_next.as_ref().unwrap().plex_choice.is_some());
        // This is the addressed response stored for later native completion;
        // the test does not claim that a decoder or native start ran.
        let response = commands::play_response(&runtime, &invocation);
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["channel"]["id"], selected);
        assert_eq!(response["data"]["channel"]["number"], index + 1);
        assert_eq!(response["data"]["operation"], operation);
        assert_eq!(response["data"]["dispatched"], true);
        assert_eq!(response["data"].as_object().unwrap().len(), 3);
        assert!(response["data"].get("ids").is_none());
        assert!(response["data"].get("version").is_none());
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        app.ott = Some(runtime);
    }
}


#[test]
fn next_channel_uses_global_favorites_after_a_new_plex_queue_is_installed() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "selected-machine");
    let favorite = selection::capture(&permanent(), &live_entry()).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.favorites_selected = true;
    }
    seed_queue(&mut app, sid, &["42", "43"]);
    let mut inv = request(&app, "playback", json!({"operation":"next_channel"}));
    inv.params.as_object_mut().unwrap().remove("runtime");
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.view_selection().1);
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref()
        else { panic!("Plex session"); };
    assert!(!session.favorites_selected, "new queue retains its compatibility default");
    let response = commands::command(&mut runtime, &mut app, &inv);
    assert!(response.is_none(), "Next must prepare a global Favorite: {response:?}");
    assert_eq!(runtime.play_target.as_ref().unwrap().id, selected);
    assert!(runtime.resolver_next.as_ref().unwrap().favorite.is_some());
    app.ott = Some(runtime);
}

#[path = "kiosk_error_status.rs"]
mod kiosk_error_status;
#[test]
fn unavailable_replacement_play_is_rejected_without_replacing_the_accepted_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for version in [1, 2] {
        let mut app = setup();
        app.ott.as_mut().unwrap().resume_source_work();
        let previous = register_source(&mut app, "previous-machine");
        seed_queue(&mut app, previous, &["9"]);
        let unavailable = register_source(&mut app, "unavailable-machine");
        app.ott.as_mut().unwrap().saved.profiles.active_profile = Some("plex-unavailable-machine".into());
        // Retire only the requested replacement authority. The old queue's
        // client and native playback identity remain valid.
        plx_plex::plex::client_for(unavailable).unwrap().disable_data_io();
        let before_saved = saved_value(&app);
        let before_queue = queue_value(&app);
        let before_identity = plx_media::route::playback_identity(&app.player.session);
        let mut params = json!({"op":"play","ids":["42","42"]});
        if version == 2 { params["state_version"] = json!(2); }
        let reply = command(&mut app, "plex_queue", params).expect("immediate unavailable source");
        assert_eq!(reply, rejected(queue::QueueError::ServerUnavailable.message()));
        assert!(serde_json::to_vec(&reply).unwrap().len() < 1024);
        assert_eq!(queue_value(&app), before_queue);
        assert_eq!(saved_value(&app), before_saved);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), before_identity);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
}

#[test]
fn failed_native_replacement_play_returns_safe_rejection_for_both_versions() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for version in [1, 2] {
        let mut app = setup();
        app.ott.as_mut().unwrap().resume_source_work();
        let sid = register_source(&mut app, "selected-machine");
        seed_queue(&mut app, sid, &["9"]);
        let before_saved = saved_value(&app);
        let before_queue = queue_value(&app);
        let before_identity = plx_media::route::playback_identity(&app.player.session);
        let mut params = json!({"op":"play","ids":["42","42"]});
        if version == 2 { params["state_version"] = json!(2); }
        let (owner, id) = begin_plex(&mut app, "plex_queue", params);
        let mut runtime = app.ott.take().unwrap();
        transient::plex_remote::prepared(&mut runtime, &mut app, owner,
            transient::plex_remote::PreparationId::Remote(id),
            Ok(proposal(owner, sid, prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play, selection: selection(&["42", "42"]),
            })));
        assert!(runtime.play_target.is_some());
        // This is the production completion used after a failed native start;
        // no decoder is launched. Its raw diagnostic must never reach the wire.
        let reply = transient::finish_native(&mut runtime, &mut app,
            Err("fixture-private-token https://private.invalid/secret /private/media"), None)
            .expect("addressed replacement completion");
        assert_eq!(reply, rejected(queue::QueueError::StartFailed.message()));
        assert!(serde_json::to_vec(&reply).unwrap().len() < 1024);
        assert!(runtime.transient.candidate.is_none());
        assert_eq!(runtime.playing.as_ref().unwrap().identity, before_identity);
        // The real Job handler clears its consumed request after this callback.
        runtime.pending_play.take();
        runtime.play_target.take();
        runtime.resolver_next.take();
        app.ott = Some(runtime);
        assert_eq!(queue_value(&app), before_queue);
        assert_eq!(saved_value(&app), before_saved);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), before_identity);
    }
}

#[path = "plex_repeat.rs"]
mod ordinary_repeat;

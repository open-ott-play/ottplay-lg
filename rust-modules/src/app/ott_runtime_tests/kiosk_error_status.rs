//! Kiosk status crosses real command ingress with an accepted native queue.
//! A synthetic decoder refusal changes the real player phase; no resolver,
//! network request, protected write or watchdog retry is started by these tests.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored};
use crate::app::ott_runtime::kiosk_source::QueuePin;

fn pinned_error() -> (App, QueuePin) {
    let mut app = setup();
    app.ott.as_mut().unwrap().resume_source_work();
    let sid = register_source(&mut app, "queue-machine");
    seed_queue(&mut app, sid, &["42", "7"]);
    let pin = transient::kiosk_queue(app.ott.as_ref().unwrap(), &app)
        .expect("healthy queue can be captured")
        .expect("accepted explicit queue");
    app.ott.as_mut().unwrap().saved.settings.insert(
        stored::KEY.into(),
        Stored::queue(pin.entries.clone(), true)
            .unwrap()
            .encode()
            .unwrap(),
    );
    plx_media::route::refuse_for_test(
        &mut app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert_eq!(
        plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Error
    );
    assert!(
        transient::kiosk_queue(app.ott.as_ref().unwrap(), &app).is_err(),
        "a decoder error must not authorize capturing a new pin"
    );
    (app, pin)
}

fn read_status(app: &mut App) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"f0a71a900211442caff63d372ac74f69", "action":"kiosk",
        "params":{"mode":"status"}, "expires_at":1020
    }]})
    .to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1, "real ingress admits the status request");
    let request = requests.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&request));
    let reply = commands::command(&mut runtime, app, &request)
        .expect("status returns immediately without a worker");
    app.ott = Some(runtime);
    reply
}

#[test]
fn kiosk_status_reports_current_plex_error_health_without_recapturing_or_retrying() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, pin) = pinned_error();
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.current_kiosk_pin_owned(&app));
    let health = runtime.kiosk_health(&app).unwrap();
    assert_eq!(health.health.as_str(), "error");
    assert_eq!(health.retries, 0);
    let before_saved = saved_value(&app);
    let before_queue = queue_value(&app);
    let before_saving = runtime.saving;
    let ticket = plx_media::route::play_request_ticket();

    let reply = read_status(&mut app);

    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["data"]["enabled"], true);
    assert_eq!(reply["data"]["state"], "locked");
    assert_eq!(reply["data"]["strict"], true);
    assert_eq!(reply["data"]["provider"], "plex");
    assert_eq!(reply["data"]["health"], health.health.as_str());
    assert_eq!(reply["data"]["retries"], health.retries);
    assert_eq!(reply["data"]["retry_seconds"], 10);
    assert_eq!(reply["data"]["screen_suspended"], false);
    assert_eq!(reply["data"]["media"]["index"], pin.index);
    assert_eq!(reply["data"]["media"]["title"], pin.entries[pin.index].title);
    assert_eq!(reply["data"]["media"]["total"], pin.entries.len());
    assert_eq!(saved_value(&app), before_saved);
    assert_eq!(queue_value(&app), before_queue);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saving, before_saving);
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none());
    assert!(plx_media::route::play_request_is_current(ticket));
    assert_eq!(
        runtime.playing.as_ref().unwrap().identity.as_ref(),
        Some(&pin.current_identity)
    );
    assert!(transient::kiosk_queue(runtime, &app).is_err());
}

fn assert_unavailable(app: &mut App) {
    let before_saved = saved_value(app);
    let ticket = plx_media::route::play_request_ticket();
    assert!(transient::kiosk_queue(app.ott.as_ref().unwrap(), app).is_err());
    let reply = read_status(app);
    assert_eq!(reply["status"], "rejected");
    assert!(reply["data"]["media"].is_null());
    assert_eq!(saved_value(app), before_saved);
    assert!(plx_media::route::play_request_is_current(ticket));
}

#[test]
fn kiosk_error_status_rejects_an_explicitly_stopped_plex_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, _) = pinned_error();
    let runtime = app.ott.as_mut().unwrap();
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_mut()
    else {
        panic!("accepted Plex fixture");
    };
    session.queue.stop(session.queue.owner()).unwrap();
    assert!(!app.ott.as_ref().unwrap().current_kiosk_pin_owned(&app));
    assert_unavailable(&mut app);
}

#[test]
fn kiosk_error_status_rejects_a_replaced_plex_profile_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, _) = pinned_error();
    app.ott.as_mut().unwrap().saved.profiles.profiles.iter_mut()
        .find(|profile| profile.kind == ProviderKind::Plex)
        .unwrap().source = "replacement-machine".into();
    assert!(!app.ott.as_ref().unwrap().current_kiosk_pin_owned(&app));
    assert_unavailable(&mut app);
}

#[test]
fn kiosk_error_status_requires_the_exact_persisted_queue_order() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, mut pin) = pinned_error();
    pin.entries.reverse();
    app.ott.as_mut().unwrap().saved.settings.insert(
        stored::KEY.into(),
        Stored::queue(pin.entries, true).unwrap().encode().unwrap(),
    );
    assert_unavailable(&mut app);
}

#[test]
fn kiosk_error_status_rejects_an_incomplete_accepted_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (mut app, _) = pinned_error();
    app.ott.as_mut().unwrap().catalog_complete = false;
    assert_unavailable(&mut app);
}

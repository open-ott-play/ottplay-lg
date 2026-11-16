//! Real exact remote-selection failures through Job::Play and the runtime tick.
//! The resolver scheduler is held before any request; no socket, decoder,
//! protected writer or native playback start is permitted by this fixture.
use super::*;

fn live_fixture() -> Fixture {
    let mut f = Fixture::new(0);
    let runtime = f.app.ott.as_mut().unwrap();
    let profile = ProviderProfile {
        id: "live-recovery".into(), name: "Live recovery".into(),
        kind: ProviderKind::M3u, source: "https://live-recovery.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true,
    };
    runtime.catalog = plx_data::ott::parse_m3u(concat!(
        "#EXTM3U\n",
        "#EXTINF:-1 tvg-id=\"selected\" group-title=\"A\",Selected\n",
        "https://live-recovery.invalid/selected.m3u8\n",
        "#EXTINF:-1 tvg-id=\"neighbor\" group-title=\"A\",Neighbor\n",
        "https://live-recovery.invalid/neighbor.m3u8\n",
        "#EXTINF:-1 tvg-id=\"other\" group-title=\"B\",Other\n",
        "https://live-recovery.invalid/other.m3u8\n",
    ), &profile).unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id);
    runtime.provider = ProviderKind::M3u;
    runtime.saved.category = "A".into();
    runtime.page = "channels";
    runtime.catalog_revision += 1;
    runtime.entropy = Some([7; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.plex_refresh_pending = true;
    runtime.resolver_hold_for_test = true;
    runtime.continuity.stop(runtime.generation);
    lifecycle::set_paused(false);
    f
}

fn queue_exact(f: &mut Fixture) -> ResolveRequest {
    let cats = f.categories();
    let category = cats["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "A").unwrap()["id"].as_str().unwrap().to_owned();
    let page = f.channels(cats["catalog"].as_str().unwrap(), &category, 0);
    let id = page["channels"][0]["id"].as_str().unwrap().to_owned();
    assert!(f.request("play_catalog", json!({"catalog":page["catalog"],"id":id})).is_none());
    let request = {
        let runtime = f.app.ott.as_mut().unwrap();
        assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
        assert_eq!(runtime.resolver_launches_for_test, 0);
        runtime.resolver_next.take().expect("real remote command queues its resolver")
    };
    assert!(guide_ui::is_live(&request.profile, request.entry.as_ref().unwrap()));
    assert!(playback_step::pending_id(f.app.ott.as_ref().unwrap(), &f.app).is_some());
    request
}

fn frame(f: &mut Fixture, elapsed_ms: u64) {
    let sample = f.app.ott.as_ref().unwrap().clock_sample.unwrap()
        .advance_for_test(Duration::from_millis(elapsed_ms));
    tick_with_start(&mut f.app, sample, |_, _, _, _, _| {
        panic!("failure-only fixture cannot admit or start native media")
    });
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.resolver_launches_for_test, 0, "held scheduler cannot launch IO");
    assert!(runtime.playing.is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

fn fail(f: &mut Fixture, request: ResolveRequest, reason: &'static str) {
    f.app.ott.as_ref().unwrap().tx.send(Job::Play(request.generation, request.profile, Err(reason)))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    frame(f, 0);
}

fn due_retry(f: &mut Fixture, id: &str, failures: usize) -> ResolveRequest {
    let runtime = f.app.ott.as_ref().unwrap();
    let pending = runtime.continuity.pending_retry().expect("owned live failure must retain cooldown");
    assert_eq!(pending.after, id);
    assert_eq!(pending.failures, failures);
    let delay = pending.not_before_ms.checked_sub(runtime.now_ms()).unwrap();
    assert!((1_000..=30_000).contains(&delay), "paced capped live backoff, got {delay}");
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none(), "failure must not synchronously launch another request");
    frame(f, delay - 1);
    assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none(), "not before the fixed deadline");
    frame(f, 1);
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().expect("deadline queues the same source");
    assert_eq!(request.entry.as_ref().unwrap().id, id);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, id);
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    assert_eq!(runtime.live_retry_generation, Some(request.generation));
    request
}

#[test]
fn exact_remote_selected_playlist_timeout_retries_the_owned_channel_after_a_cooldown() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = live_fixture();
    let request = queue_exact(&mut f);
    let selected = request.entry.as_ref().unwrap().id.clone();
    fail(&mut f, request, "Selected HLS playlist request timed out");
    assert_eq!(f.app.ott.as_ref().unwrap().status, "Live stream unavailable; retrying the same channel");
    let _retry = due_retry(&mut f, &selected, 1);
}

#[test]
fn exact_live_startup_recovery_keeps_pacing_after_four_transient_failures() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = live_fixture();
    let mut request = queue_exact(&mut f);
    let selected = request.entry.as_ref().unwrap().id.clone();
    for attempt in 1..=8 {
        fail(&mut f, request, "Selected HLS playlist request timed out");
        request = due_retry(&mut f, &selected, attempt);
    }
}

#[test]
fn only_closed_timeout_and_server_startup_errors_keep_exact_live_recovery() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for reason in [
        "Initial HLS playlist request timed out", "HLS playlist request timed out",
        "HLS playlist body transfer timed out", "HLS playlist server returned HTTP 429",
        "HLS playlist server returned a server error",
        "HLS playlist transient transport failed",
        "Media source request timed out",
        "Media source network transport failed",
        "Media source server returned HTTP 429",
        "Media source server returned a server error",
    ] {
        let mut f = live_fixture();
        let request = queue_exact(&mut f);
        let selected = request.entry.as_ref().unwrap().id.clone();
        fail(&mut f, request, reason);
        let _retry = due_retry(&mut f, &selected, 1);
    }
    for reason in [
        "HLS playlist server returned HTTP 401", "HLS playlist server returned HTTP 403",
        "Invalid HLS playlist header", "HLS playlist network transport failed",
        "Could not fetch HLS playlist", "Unknown fixture failure",
    ] {
        let mut f = live_fixture();
        let request = queue_exact(&mut f);
        fail(&mut f, request, reason);
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(runtime.continuity.pending_retry().is_none(), "terminal cause: {reason}");
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        frame(&mut f, 60_000);
        assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none(), "terminal cause must not later retry: {reason}");
    }
}

#[test]
fn cancelled_changed_paused_and_standby_live_startups_cannot_create_recovery() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for case in ["cancel", "profile", "paused", "standby"] {
        let mut f = live_fixture();
        let request = queue_exact(&mut f);
        match case {
            "cancel" => {
                let runtime = f.app.ott.as_mut().unwrap();
                commands::cancel_pending_play(runtime);
                runtime.cancel_play_request();
            }
            "profile" => f.app.ott.as_mut().unwrap().saved.profiles.profiles[0].source =
                "https://replaced.invalid/list.m3u".into(),
            "paused" => lifecycle::set_paused(true),
            "standby" => f.app.ott_standby = true,
            _ => unreachable!(),
        }
        fail(&mut f, request, "Selected HLS playlist request timed out");
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(runtime.continuity.pending_retry().is_none(), "{case}");
        assert!(runtime.resolver_next.is_none(), "{case}");
        frame(&mut f, 60_000);
        assert!(f.app.ott.as_ref().unwrap().resolver_next.is_none(), "late retry after {case}");
    }
}

#[test]
fn stale_live_startup_job_cannot_replace_or_retry_the_current_request() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = live_fixture();
    let request = queue_exact(&mut f);
    let selected = request.entry.as_ref().unwrap().id.clone();
    let generation = request.generation;
    f.app.ott.as_ref().unwrap().tx.send(Job::Play(generation.wrapping_sub(1), request.profile,
        Err("Selected HLS playlist request timed out"))).unwrap_or_else(|_| panic!("fixture receiver"));
    frame(&mut f, 0);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, selected);
    assert!(runtime.continuity.pending_retry().is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn exact_live_startup_retry_keeps_its_source_queue_while_another_category_is_browsed() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = live_fixture();
    let request = queue_exact(&mut f);
    let selected = request.entry.as_ref().unwrap().id.clone();
    let original = f.app.ott.as_ref().unwrap().catalog.entries.iter()
        .filter(|entry| entry.category == "A").map(|entry| entry.id.clone()).collect::<Vec<_>>();
    // A browse-only category change does not withdraw the accepted remote play.
    // Keep the real captured source/list receipt; never rebind it to category B.
    f.app.ott.as_mut().unwrap().saved.category = "B".into();
    fail(&mut f, request, "Selected HLS playlist request timed out");
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.category, "B");
    assert_eq!(runtime.ordered_ids(), original);
    let _retry = due_retry(&mut f, &selected, 1);
    assert_eq!(f.app.ott.as_ref().unwrap().saved.category, "B");
}

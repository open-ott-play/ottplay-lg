//! A composed source-bound regression. Transport, media inspection and resource
//! preparation are observations; parser, form, resolver and Job admission are real.
use super::*;

fn queued_selection() -> (Box<App>, ResolveRequest, CatalogEntry, String) {
    let profile = synthetic_profile();
    let catalog = plx_data::ott::parse_vportal_response(&json!({
        "type":"videoportal", "request":{"key":"echo-not-authority"}, "items":[
            {"type":"category", "title":"Collection", "items":[
                {"type":"category", "title":"Nested", "items":[
                    {"type":"stream", "title":"First video", "id":"first", "request":{"cmd":"play","id":"first"}},
                    {"type":"stream", "title":"Selected video", "id":"selected", "request":{
                        "cmd":"play", "id":"selected", "key":"echo-not-authority", "app":"echo-app"
                    }}
                ]}
            ]},
            {"type":"category", "title":"Other", "items":[
                {"type":"stream", "title":"Other video", "id":"other", "request":{"cmd":"play","id":"other"}}
            ]}
        ]
    }).to_string(), &profile, "").expect("genuine normalized catalogue").catalog;
    let entry = catalog.entries.iter().find(|entry| entry.provider_id.as_deref() == Some("selected"))
        .expect("parsed nested stream").clone();
    let MediaLocator::VPortal { request } = &entry.media else { panic!("parser must retain deferred VPortal locator") };
    assert_eq!(request, &json!({"cmd":"play", "id":"selected"}));

    let mut app = Box::new(app_fixture());
    app.running = true;
    let category = catalog.category_choices().into_iter()
        .find(|category| category.label == "Collection / Nested")
        .expect("structural nested category").id;
    assert_ne!(category, "Collection / Nested", "selection binds a structural ID, not a label");
    let mut saved = app.ott.as_ref().unwrap()
        .interface_mode_candidate(plx_machine::machine::UiMode::Classic);
    saved.profiles.profiles = vec![profile.clone()];
    saved.category = category.clone();
    saved.profiles.active_profile = Some(profile.id.clone());
    saved.validate().unwrap();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.write_permit.revoke();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.plex_refresh_pending = true;
        runtime.saving = true; // No checkpoint writer is admitted by this fixture.
        runtime.tx.send(Job::Loaded(Ok((saved, [1; 16]))))
            .unwrap_or_else(|_| panic!("fixture load receiver disappeared"));
    }
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), plx_machine::machine::UiMode::Classic);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.active(), Some(&profile));
        runtime.continuity.stop(runtime.generation);
        let _ = runtime.catalog_next.take(); // Model the already-held catalogue worker.
        runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation, catalog))
            .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
        runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(())))
            .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    }
    runtime_tick(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saving = true; // Hold unrelated focus/checkpoint writes during real input.
        runtime.page = "channels";
        assert_eq!(runtime.effective_category(), category);
        assert_eq!(runtime.ordered_ids().len(), 2);
        assert_ne!(runtime.ordered_ids()[0], entry.id);
        assert_eq!(runtime.ordered_ids()[1], entry.id);
        assert!(runtime.playing.is_none());
    }
    bridge::show_page(&mut app.pages, AppArg::Ott);
    idle(&mut app);
    // Verify the actual focused form element is a media card before pressing OK.
    let focus = app.pages.focus().expect("real catalogue focus");
    {
        let parts = app.pages.parts(Tick { ms: app.loop_t, dt_us: 16_000 });
        let split = plx_ui::dispatch::Rig::<bridge::AppHost>::split(&mut app.bridge);
        let cx = parts.cx::<bridge::AppHost>(split.views, split.measure);
        let screen = &app.pages.nav.entry(focus.entry).unwrap().inst.as_ref().unwrap().screen;
        assert_eq!(screen.press_kind(&focus.elem, &cx), Some(plx_ui::screen::ElemKind::Card));
    }
    assert_ne!(app.ott.as_ref().unwrap().selected.as_deref(), Some(entry.id.as_str()));
    key(&mut app, Key::Down);
    assert_ne!(app.pages.focus(), Some(focus), "Down moves actual form focus to the second row");
    key(&mut app, Key::Ok);
    assert_eq!(app.ott.as_ref().unwrap().selected.as_deref(), Some(entry.id.as_str()));
    let queued = app.ott.as_mut().unwrap().resolver_next.take().expect("real row OK queued a resolver");
    assert_eq!(queued.profile, profile);
    assert_eq!(queued.entry.as_ref(), Some(&entry));
    assert!(matches!(queued.entry.as_ref().unwrap().media, MediaLocator::VPortal { .. }));
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(!route::has_url(&app.player.session));

    (app, queued, entry, category)
}

fn resolve(app: &App, queued: &ResolveRequest) -> ReadySource {
    let runtime = app.ott.as_ref().unwrap();
    let preferred = vportal_quality::preference(&runtime.saved, &queued.profile);
    let interactive = vportal_quality::interactive(runtime, queued.generation);
    assert!(runtime.saving && !interactive && preferred.is_none(),
        "held unrelated writer selects the actual automatic quality path");
    let mut fetches = 0;
    let (url, variants, sessions, failure) = ott_catalog::vportal_resolution_with_fetch_for_test(
        &queued.profile, queued.entry.as_ref().unwrap(), &queued.cancel,
        preferred.as_deref(), interactive,
        &mut |endpoint, body, cancel| {
            fetches += 1;
            assert_eq!(endpoint, "https://media.invalid:443/nas/api");
            assert_eq!(body, &json!({"cmd":"play", "id":"selected", "app":"ott-play", "key":"fixture"}));
            assert!(!cancel.load(Ordering::Acquire));
            Ok(json!({"type":"stream", "url":"/resolved/selected.mp4",
                "variants":{"Original":"/resolved/selected.mp4"},
                "key":"response-echo-is-not-authority"}).to_string())
        },
    ).expect("the actual deferred resolver accepts this locator");
    assert_eq!(fetches, 1);
    assert_eq!((variants, sessions, failure), (1, 0, None));
    assert_eq!(url, "https://media.invalid:443/resolved/selected.mp4");
    assert!(matches!(queued.entry.as_ref().unwrap().media, MediaLocator::VPortal { .. }));
    // Only the libavformat observation is supplied here; no inspection or IO is run.
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    info.duration_ns = 60_000_000_000;
    let resolved_source = plx_media::ff::ExternalSource::Http(url);
    ReadySource::External(resolved_source, info, None)
}

#[test]
fn parsed_vportal_row_resolves_admits_and_reopens_its_structural_category() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, queued, entry, category) = queued_selection();
    let profile = queued.profile.clone();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let catalog_generation = app.ott.as_ref().unwrap().generation;
    let ready = resolve(&app, &queued);
    let ReadySource::External(resolved_source, _, _) = &ready else { unreachable!() };
    let resolved_source = resolved_source.clone();
    app.ott.as_ref().unwrap().tx.send(Job::Play(queued.generation, queued.profile,
        Ok((queued.entry.unwrap(), ready))))
        .unwrap_or_else(|_| panic!("fixture play receiver disappeared"));
    let mut resources = CatalogAcceptedResources::default();
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |runtime, app, profile, entry, source| {
        runtime.start_resolved_with_kiosk_using(app, profile, entry, source,
            |runtime, app, profile, entry, source|
                start_resolved_with(runtime, app, profile, entry, source, &mut resources))
    });
    assert_eq!(resources.starts, 1, "real Job validation reaches native resource admission once");
    assert_eq!(route::external_source(&app.player.session), Some(resolved_source));
    let identity = route::playback_identity(&app.player.session).expect("native route identity");
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.resolver_busy = true; // Job settlement releases the held worker slot.
        let playing = runtime.playing.as_ref().expect("admission installed ownership");
        assert_eq!(playing.profile, profile);
        assert_eq!(playing.entry, entry);
        assert_eq!(playing.identity.as_ref(), Some(&identity));
        assert_eq!(runtime.generation, catalog_generation);
        assert_eq!(runtime.effective_category(), category);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
    // Accepted resources do not run a decoder. Supply the existing inert-engine
    // presentation observation so the real More door can test reopening readiness.
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED.duration_ns.store(60_000_000_000, Ordering::Release);
    player::SHARED.playpos_ns.store(0, Ordering::Release);
    player::SHARED.start_queued.store(false, Ordering::Release);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    settle_pages(&mut app);
    let now = app.loop_t;
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(now, 60_000);
    idle(&mut app);
    let player_page = app.pages.top_page();
    let player_origin = bridge::player(&app.pages).unwrap().origin;
    open_more(&mut app);
    choose_catalog(&mut app);
    assert_catalog_card_focus(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.active(), Some(&profile));
    assert_eq!(runtime.effective_category(), category);
    assert_eq!(runtime.ordered_ids().len(), 2);
    assert_eq!(runtime.ordered_ids()[1], entry.id);
    assert_eq!(runtime.generation, catalog_generation);
    assert_eq!(app.pages.top_page(), player_page);
    assert_eq!(bridge::player(&app.pages).unwrap().origin, player_origin);
    assert_eq!(runtime.selected.as_deref(), Some(entry.id.as_str()));
    assert_eq!(runtime.playing.as_ref().unwrap().entry, entry);
    assert_eq!(route::playback_identity(&app.player.session), Some(identity.clone()));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(resources.starts, 1);
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none(), "Back closes the catalogue, not Player");
    assert_eq!(app.pages.top_page(), player_page);
    assert_eq!(bridge::player(&app.pages).unwrap().origin, player_origin);
    assert_eq!(route::playback_identity(&app.player.session), Some(identity));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.active(), Some(&profile));
    assert_eq!(runtime.effective_category(), category);
    assert_eq!(runtime.selected.as_deref(), Some(entry.id.as_str()));
    assert_eq!(runtime.generation, catalog_generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(resources.starts, 1);
}

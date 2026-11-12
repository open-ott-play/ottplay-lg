//! Runtime event regressions. Resolver and writer completions are injected;
//! no network, decoder, protected storage or real session worker is started.
use super::*;
use crate::app::{bridge, run};
use plx_machine::machine::Tick;
use plx_screens::registry::AppArg;

fn profile() -> ProviderProfile {
    ProviderProfile { id:"quality-fixture".into(), name:"Quality fixture".into(), kind:ProviderKind::VPortal,
        source:"portal::[key:fixture]https://fixture.invalid/nas/api".into(), source_alias:String::new(), enabled:true }
}
fn entry() -> CatalogEntry {
    CatalogEntry { protection: None, synopsis: None, id:"quality-film".into(), title:"Quality film".into(), category:String::new(), description:String::new(),
        poster:String::new(), duration_ms:Some(12_000), media:MediaLocator::VPortal { request:json!({"id":"film"}) },
        provider_id:Some("film".into()), provider_id_field:Some("id".into()), live:None }
}
fn tick(app: &mut App) {
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    super::super::tick(app, clock);
}
fn settle(app: &mut App) {
    for _ in 0..24 {
        app.loop_t = app.loop_t.wrapping_add(16);
        bridge::frame(&mut app.pages, &mut app.bridge, Tick { ms:app.loop_t, dt_us:16_000 }, Vec::new());
    }
}
fn fixture() -> App {
    let mut app = run::lifecycle_regression_tests::app();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_quality", "quality");
    // First lifecycle sample has no source, so source rearming cannot start IO.
    app.ott = Some(runtime);
    tick(&mut app);
    let r = app.ott.as_mut().unwrap();
    r.loaded = true;
    r.entropy = Some([1;16]);
    r.saved.profiles.profiles = vec![profile()];
    r.saved.profiles.active_profile = Some(profile().id);
    r.provider = ProviderKind::VPortal;
    r.catalog = Catalog { entries:vec![entry()], hierarchy:None };
    r.catalog_complete = true;
    r.catalog_loading = false;
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    r.plex_refresh_pending = true;
    r.write_permit.revoke();
    r.page = "channels";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(&mut app); tick(&mut app); settle(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(current_ui_owner(&app), r.reply);
    assert!(r.reply.is_some() && r.play_work_allowed());
    app
}
/// Exercise ordinary local Play admission, then model the queued worker taking
/// the request. The navigation seal matches the runner's same-action ordering.
fn local_play(app: &mut App) -> u64 {
    let mut r = app.ott.take().unwrap();
    r.resolver_busy = true;
    handle_action(&mut r, app, OttAction::Play(entry().id)).unwrap();
    r.accepted_navigation(false);
    seal(&mut r);
    assert!(r.resolver_next.take().is_some());
    let generation = r.play_generation;
    assert!(interactive(&r, generation));
    app.ott = Some(r);
    generation
}
fn prepared(app: &mut App, generation: u64, leases: Vec<Lease>) {
    app.ott.as_ref().unwrap().tx.send(Job::VPortalPrepared(generation, profile(), entry(),
        ott_catalog::quality_result_for_test(leases))).ok().unwrap();
    tick(app);
}
fn choose_action(app: &App) -> OttAction {
    view(app.ott.as_ref().unwrap()).unwrap().rows[1].action.clone()
}
fn dispatch(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut r = app.ott.take().unwrap();
    let result = handle_action(&mut r, app, action);
    app.ott = Some(r);
    result
}
fn pending_save(app: &mut App) -> Saved {
    let r = app.ott.as_mut().unwrap();
    let pending = r.quality.pending.as_mut().unwrap();
    pending.selected = Some(1); pending.saving = true;
    let candidate = preference_candidate(&r.saved, &pending.context.profile, &pending.choices.options[1].key).unwrap();
    r.saving = true; r.save_ui_owner = Some(pending.context.owner);
    r.save_navigation = SaveNavigation::Preserve;
    r.save_playback_epoch = r.playback_epoch;
    candidate
}

#[test]
fn local_prepared_choices_are_opaque_context_scoped_and_do_not_change_saved() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let generation = local_play(&mut app); prepared(&mut app, generation, Vec::new());
    let r = app.ott.as_ref().unwrap(); let v = view(r).unwrap();
    assert!(v.page_id.starts_with("vportal-quality:"));
    assert_eq!(v.rows.iter().map(|row|row.title.as_str()).collect::<Vec<_>>(), ["High","Low"]);
    assert_eq!(v.selected_row.as_ref(), Some(&v.rows[0].id));
    for row in &v.rows {
        let OttAction::SelectVPortalQuality { context, variant } = &row.action else { panic!("typed quality action") };
        assert_eq!(context.len(),64); assert_eq!(variant.len(),64); assert_eq!(row.id.len(),64);
        assert_ne!(&row.id, variant); assert!(!row.detail.contains("https:"));
    }
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn late_a_result_preserves_b_intent_and_b_can_open_the_picker() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let a = local_play(&mut app); let b = local_play(&mut app); assert_ne!(a,b);
    let token = app.ott.as_ref().unwrap().quality.intent.as_ref().unwrap().token.clone();
    let (lease, release, _done) = Lease::pending_for_test();
    prepared(&mut app, a, vec![lease]);
    assert_eq!(release.try_recv(), Ok(()));
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.quality.intent.as_ref().unwrap().token, token);
    assert!(r.quality.pending.is_none());
    prepared(&mut app, b, Vec::new());
    assert_eq!(view(app.ott.as_ref().unwrap()).unwrap().page_id, format!("vportal-quality:{token}"));
}

#[test]
fn back_releases_all_prepared_sessions_without_any_next_play_or_tick() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(); let generation = local_play(&mut app);
    let (a, ar, _ad) = Lease::pending_for_test(); let (b, br, _bd) = Lease::pending_for_test();
    prepared(&mut app, generation, vec![a,b]);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    dispatch(&mut app, OttAction::Back).unwrap();
    assert_eq!(ar.try_recv(),Ok(())); assert_eq!(br.try_recv(),Ok(()));
    let r = app.ott.as_ref().unwrap(); assert_eq!(r.page,"channels");
    assert!(r.quality.pending.is_none() && r.play_target.is_none());
    assert_eq!(r.quality.retired.len(),2);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(),before);
}

#[test]
fn automatic_request_and_kiosk_never_install_interactive_intent() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut r = app.ott.take().unwrap();
    // The same ordinary API is used by remote, startup and continuity callers.
    r.play_request_with_policy(&mut app, PlaybackRequest::ordinary(&entry().id,0)).unwrap();
    assert!(r.quality.intent.is_none()); assert!(r.resolver_next.take().is_some());
    r.cancel_play_request();
    let origin = plx_data::ott::favorites::Origin { provider:"vportal".into(), profile:profile().name,
        source_alias:"Quality fixture".into(), subprofile:None };
    let stored = crate::app::ott_kiosk_stored::Stored::armed(origin, false).unwrap();
    r.saved.settings.insert(crate::app::ott_kiosk_stored::KEY.into(), stored.encode().unwrap());
    assert!(matches!(r.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Armed { .. })));
    assert!(!may_offer(&r,&app));
    app.ott = Some(r);
}

#[test]
fn old_choice_owner_navigation_ticket_and_catalog_changes_are_refused_before_save() {
    let _serial = plx_base::testlock::serial();
    for change in 0..5 {
        let mut app = fixture(); let generation = local_play(&mut app);
        prepared(&mut app,generation,Vec::new());
        // Local Play also requests a browse-focus save. Settle that independent,
        // pre-revoked writer before checking that a stale quality action saves nothing.
        if app.ott.as_ref().unwrap().saving {
            let failed = app.ott.as_ref().unwrap().rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("revoked browse-focus writer must settle");
            assert!(matches!(&failed, Job::Saved(Err(_), None, false, _)));
            app.ott.as_ref().unwrap().tx.send(failed)
                .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
            tick(&mut app);
        }
        let r = app.ott.as_ref().unwrap();
        let pending = r.quality.pending.as_ref().unwrap();
        assert!(!r.saving && !pending.saving);
        assert!(current(r, &app, &pending.context));
        let action = choose_action(&app);
        let r = app.ott.as_mut().unwrap();
        match change {
            0 => r.reply = None,
            1 => r.accepted_navigation(false),
            2 => r.play_ticket = None,
            3 => {
                r.catalog.entries[0].media = MediaLocator::VPortal { request:json!({"id":"replacement"}) };
                r.catalog_revision = r.catalog_revision.wrapping_add(1);
            },
            _ => { r.cancel_play_request(); }
        }
        assert!(dispatch(&mut app, action).is_err());
        let r = app.ott.as_ref().unwrap();
        assert!(!r.saving); assert_eq!(preference(&r.saved,&profile()),None);
    }
}

#[test]
fn failed_and_uncertain_writer_events_never_start_selected_probe() {
    let _serial = plx_base::testlock::serial();
    for uncertain in [false,true] {
        let mut app = fixture(); let generation = local_play(&mut app);
        let (lease,release,_done) = Lease::pending_for_test();
        prepared(&mut app,generation,vec![lease]); let _candidate = pending_save(&mut app);
        let job = if uncertain { Job::Uncertain(None,None) }
            else { Job::Saved(Err("fixture write rejected"),None,false,ok(json!({}))) };
        app.ott.as_ref().unwrap().tx.send(job).ok().unwrap(); tick(&mut app);
        let r = app.ott.as_ref().unwrap();
        assert!(!r.resolver_busy); assert!(!plx_media::route::has_url(&app.player.session));
        assert_eq!(preference(&r.saved,&profile()),None);
        if uncertain { assert!(r.quality.pending.is_none()); assert_eq!(release.try_recv(),Ok(())); }
        else { assert!(!r.quality.pending.as_ref().unwrap().saving); assert!(release.try_recv().is_err()); }
    }
}

#[test]
fn back_during_save_allows_durable_preference_but_never_reopens_or_plays() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(); let generation = local_play(&mut app);
    let (lease,release,_done) = Lease::pending_for_test(); prepared(&mut app,generation,vec![lease]);
    let candidate = pending_save(&mut app);
    dispatch(&mut app,OttAction::Back).unwrap(); assert_eq!(release.try_recv(),Ok(()));
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(candidate),None,false,ok(json!({})))).ok().unwrap();
    tick(&mut app);
    let r = app.ott.as_ref().unwrap(); assert_eq!(r.page,"channels");
    assert!(r.quality.pending.is_none() && r.play_target.is_none() && !r.resolver_busy);
    assert_eq!(preference(&r.saved,&profile()).as_deref(),Some("Low"));
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn preference_roundtrip_is_exact_and_bound_to_effective_profile_and_source() {
    let saved = Saved::default(); let p = profile();
    let candidate = preference_candidate(&saved,&p,"High").unwrap();
    assert_eq!(preference(&saved,&p),None);
    let restored: Saved = serde_json::from_value(serde_json::to_value(&candidate).unwrap()).unwrap();
    assert_eq!(preference(&restored,&p).as_deref(),Some("High"));
    let mut changed = p.clone(); changed.source.push_str("?different"); assert_eq!(preference(&restored,&changed),None);
    changed = p.clone(); changed.id.push_str("-other"); assert_eq!(preference(&restored,&changed),None);
    let encoded = serde_json::to_string(&candidate.settings).unwrap();
    assert!(!encoded.contains("fixture.invalid") && !encoded.contains("key:fixture"));
    assert!(preference_candidate(&saved,&p,"High\n").is_err());
}

#[test]
fn modal_picker_back_returns_to_channels_without_closing_or_retargeting_the_surface() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    crate::app::playback::enter_player(&mut app.pages, &mut app.bridge,
        crate::app::playback::Origin::Here, None);
    settle(&mut app);
    let mut r = app.ott.take().unwrap();
    // Exercise the existing presentation after its separately tested admission.
    player_browse::present(&mut r, &mut app, None, "channels");
    app.ott = Some(r);
    settle(&mut app); tick(&mut app); settle(&mut app);
    assert!(matches!(app.route(), AppArg::Player));
    let surface = bridge::ott_catalog_surface(&app.pages).expect("actual Player catalogue modal");
    let generation = local_play(&mut app);
    let (lease, release, _done) = Lease::pending_for_test();
    prepared(&mut app, generation, vec![lease]);
    assert_eq!(app.ott.as_ref().unwrap().page, "vportal-quality");
    dispatch(&mut app, OttAction::Back).unwrap();
    assert_eq!(release.try_recv(), Ok(()));
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.page, "channels");
    assert!(r.quality.pending.is_none() && r.play_target.is_none());
    assert_eq!(bridge::ott_catalog_surface(&app.pages), Some(surface));
    assert!(matches!(app.route(), AppArg::Player));
}

#[test]
fn unrelated_catalogue_append_preserves_preparing_and_prepared_quality_selection() {
    let _serial = plx_base::testlock::serial();
    for already_prepared in [false, true] {
        let mut app = fixture();
        app.ott.as_mut().unwrap().catalog_loading = true;
        app.ott.as_mut().unwrap().catalog_complete = false;
        let generation = local_play(&mut app);
        if already_prepared { prepared(&mut app, generation, Vec::new()); }
        let r = app.ott.as_ref().unwrap();
        let cancel = r.play_cancel.clone();
        let before_revision = r.catalog_revision;
        let before_saved = serde_json::to_value(&r.saved).unwrap();
        let mut unrelated = entry();
        unrelated.id = "unrelated-film".into();
        unrelated.media = MediaLocator::VPortal { request:json!({"id":"unrelated"}) };
        r.catalog_tx.send(CatalogEvent::Batch(r.generation,
            Catalog { entries:vec![unrelated], hierarchy:None })).unwrap();
        tick(&mut app);
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.catalog.entries.len(), 2, "batch must actually merge");
        assert_ne!(r.catalog_revision, before_revision);
        assert!(!cancel.load(Ordering::Acquire), "unrelated append cancelled selected source");
        assert_eq!(r.play_generation, generation);
        assert_eq!(r.play_target.as_ref().unwrap().id, entry().id);
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before_saved);
        if !already_prepared { prepared(&mut app, generation, Vec::new()); }
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.page, "vportal-quality");
        assert!(current(r, &app, &r.quality.pending.as_ref().unwrap().context));
        assert!(!plx_media::route::has_url(&app.player.session));
    }
}

#[test]
fn accepted_metadata_enrichment_preserves_preparing_and_prepared_quality_selection() {
    let _serial = plx_base::testlock::serial();
    for already_prepared in [false, true] {
        let mut app = fixture();
        let generation = local_play(&mut app);
        if already_prepared { prepared(&mut app, generation, Vec::new()); }
        let r = app.ott.as_mut().unwrap();
        let cancel = r.play_cancel.clone();
        let before_revision = r.catalog_revision;
        let before_saved = serde_json::to_value(&r.saved).unwrap();
        r.metadata_generation = 37;
        r.metadata_target = Some((profile().id.clone(), entry().id.clone()));
        r.metadata_refresh_ms = u64::MAX;
        r.metadata_tx.send(crate::app::ott_metadata::Event::Preview(crate::app::ott_metadata::Preview {
            generation:r.metadata_generation, profile_id:profile().id, entry_id:entry().id,
            entry:Some(entry()), result:Ok(json!({"description":"Enriched fixture description",
                "poster":"https://fixture.invalid/enriched.jpg"})),
        })).unwrap();
        tick(&mut app);
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.catalog.entries[0].description, "Enriched fixture description",
            "metadata completion must actually enrich the selected row");
        assert_eq!(r.catalog.entries[0].poster, "https://fixture.invalid:443/enriched.jpg");
        assert_ne!(r.catalog_revision, before_revision);
        assert!(!cancel.load(Ordering::Acquire), "display metadata cancelled selected source");
        assert_eq!(r.play_generation, generation);
        assert_eq!(r.play_target.as_ref().unwrap().id, entry().id);
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before_saved);
        if !already_prepared { prepared(&mut app, generation, Vec::new()); }
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.page, "vportal-quality");
        assert!(current(r, &app, &r.quality.pending.as_ref().unwrap().context));
        assert!(!plx_media::route::has_url(&app.player.session));
    }
}

#[test]
fn changed_selected_source_or_owner_still_retires_preparing_and_prepared_quality() {
    let _serial = plx_base::testlock::serial();
    for already_prepared in [false, true] {
        for change in 0..9 {
            let mut app = fixture();
            let generation = local_play(&mut app);
            if already_prepared { prepared(&mut app, generation, Vec::new()); }
            let mut r = app.ott.take().unwrap();
            let cancel = r.play_cancel.clone();
            match change {
                0 => r.catalog.entries[0].media = MediaLocator::VPortal { request:json!({"id":"replacement"}) },
                1 => r.catalog.entries.clear(),
                2 => r.saved.profiles.profiles[0].source.push_str("?changed"),
                3 => r.generation = r.generation.wrapping_add(1),
                4 => {
                    let mut other = entry(); other.id = "another-selection".into();
                    r.play_target.as_mut().unwrap().id = other.id.clone();
                    r.catalog.entries.push(other);
                },
                5 => r.play_cancel.store(true, Ordering::Release),
                6 => r.play_generation = r.play_generation.wrapping_add(1),
                7 => r.accepted_navigation(false),
                _ => {
                    let mut other = profile(); other.id = "another-profile".into();
                    r.saved.profiles.active_profile = Some(other.id.clone());
                    r.saved.profiles.profiles.push(other);
                },
            }
            // Deliberately do NOT change catalogue revision. Identity and owner
            // fences must independently reject the changed selection/source.
            observe(&mut r, &app);
            assert!(cancel.load(Ordering::Acquire), "change {change}, prepared {already_prepared}");
            assert!(r.play_target.is_none() && r.quality.intent.is_none() && r.quality.pending.is_none());
            assert_eq!(preference(&r.saved, &profile()), None);
            assert!(!plx_media::route::has_url(&app.player.session));
            app.ott = Some(r);
        }
    }
}


#[test]
fn preplay_quality_wait_and_back_preserve_the_attempt_and_release_behavior() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(fixture());
    let generation = local_play(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap()["phase"], "resolving");
    let (lease, release, _done) = Lease::pending_for_test();
    prepared(&mut app, generation, vec![lease]);
    let r = app.ott.as_ref().unwrap();
    let waiting = serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap();
    assert_eq!(waiting["phase"], "awaiting_quality");
    assert_eq!(waiting["attempt_generation"], generation);
    let saved = serde_json::to_value(&r.saved).unwrap();
    dispatch(&mut app, OttAction::Back).unwrap();
    assert_eq!(release.try_recv(), Ok(()));
    let r = app.ott.as_ref().unwrap();
    let stopped = serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap();
    assert_eq!(stopped["phase"], "cancelled");
    assert_eq!(stopped["cancel_reason"], "user_back");
    assert_eq!(stopped["attempt_generation"], generation);
    assert_ne!(r.play_generation, generation);
    assert!(r.play_target.is_none());
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
}

#[test]
fn preplay_quality_owner_change_reports_the_actual_cancel_decision() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(fixture());
    let generation = local_play(&mut app);
    prepared(&mut app, generation, Vec::new());
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    tick(&mut app);
    let r = app.ott.as_ref().unwrap();
    let stopped = serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap();
    assert!(r.play_target.is_none() && r.quality.pending.is_none());
    assert_eq!(stopped["phase"], "cancelled");
    assert_eq!(stopped["cancel_reason"], "owner_changed");
    assert_eq!(stopped["attempt_generation"], generation);
}

#[test]
fn preplay_retired_quality_result_cannot_publish_waiting_for_a_new_attempt() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(fixture());
    let a = local_play(&mut app);
    let b = local_play(&mut app);
    prepared(&mut app, a, Vec::new());
    let r = app.ott.as_ref().unwrap();
    let current = serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap();
    assert_eq!(current["phase"], "resolving");
    assert_eq!(current["attempt_generation"], b);
    assert!(r.quality.pending.is_none());
    prepared(&mut app, b, Vec::new());
    let r = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(r.preplay.snapshot(r.now_ms())).unwrap()["phase"], "awaiting_quality");
}

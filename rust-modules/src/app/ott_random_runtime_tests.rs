//! Actual dispatcher and ordinary Play binding, with existing inert fixtures.
//! Hold resolver/metadata slots; no media IO or successful protected writes.
use super::*;
use plx_data::ott::favorites::{Locator, Origin};

fn profile() -> ProviderProfile {
    ProviderProfile { id: "random-fixture".into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: "https://playlist.fixture.invalid/live.m3u".into(), source_alias: "Home".into(), enabled: true }
}
fn catalog() -> Catalog {
    plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"alpha\" group-title=\"News\",Alpha\nhttps://media.fixture.invalid/a.m3u8\n\
         #EXTINF:-1 tvg-id=\"beta\" group-title=\"News\",Beta\nhttps://media.fixture.invalid/b.m3u8\n\
         #EXTINF:-1 tvg-id=\"gamma\" group-title=\"Cinema\",Gamma\nhttps://media.fixture.invalid/c.m3u8\n", &profile()).unwrap()
}
fn favorite(title: &str) -> Favorite {
    Favorite { title: title.into(), origin: Origin { provider: "m3u".into(), profile: "Home".into(),
        source_alias: "Home".into(), subprofile: None }, locator: Locator::Channel { channel_id: title.to_lowercase() } }
}
fn mounted() -> App {
    let mut app = app_fixture();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    runtime.saved.profiles.profiles = vec![profile()];
    runtime.saved.profiles.active_profile = Some(profile().id);
    runtime.catalog = catalog();
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.continuity.stop(runtime.generation);
    runtime.page = "channels";
    runtime.sync_saved_context();
    runtime.changed();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
}
fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}
fn action(app: &App) -> OttAction {
    OttAction::Random(app.ott.as_ref().unwrap().view().random_context.unwrap())
}
fn query(app: &mut App, value: &str) {
    ui(app, OttAction::BeginSearch);
    let session = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    ui(app, OttAction::SubmitSearch { session, value: value.into() });
}
fn id(app: &App, title: &str) -> String {
    app.ott.as_ref().unwrap().catalog.entries.iter().find(|row| row.title == title).unwrap().id.clone()
}

#[test]
fn actual_dispatch_uses_current_category_and_search_without_saved_or_order_changes() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    app.ott.as_mut().unwrap().saved.category = "News".into();
    query(&mut app, "beta");
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let rows = app.ott.as_ref().unwrap().catalog.clone();
    let wanted = id(&app, "Beta");
    let random = action(&app); ui(&mut app, random);
    let runtime = app.ott.as_ref().unwrap();
    let target = runtime.play_target.as_ref().unwrap();
    assert_eq!(target.id, wanted);
    assert!(!target.advance_on_failure);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.catalog, rows);
    assert!(!runtime.saving);
}

#[test]
fn background_append_does_not_invalidate_a_source_and_view_bound_choice() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_complete = false; runtime.catalog_loading = true;
    runtime.catalog.entries.truncate(1);
    let old = action(&app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog = catalog(); runtime.catalog_revision += 1; runtime.changed();
    assert_eq!(action(&app), old);
    ui(&mut app, old);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.as_ref().is_some_and(|target|
        runtime.catalog.entries.iter().any(|entry| entry.id == target.id)));
    assert!(!runtime.saving);
}

#[test]
fn b01_membership_projection_is_the_random_candidate_set() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let wanted = id(&app, "Gamma");
    let runtime = app.ott.as_mut().unwrap();
    let source = catalog_organization::Source::new(&profile()).unwrap();
    let mut organization = catalog_organization::Document::read(&runtime.saved.settings).unwrap();
    let change = organization.apply(&source, &runtime.catalog, catalog_organization::Operation::Create { label: "Only Gamma".into() }).unwrap();
    let category = change.category.unwrap();
    organization.apply(&source, &runtime.catalog, catalog_organization::Operation::AddMember { category: category.clone(), item: wanted.clone() }).unwrap();
    organization.write_into(&mut runtime.saved.settings).unwrap();
    runtime.saved.category = category;
    runtime.changed();
    let random = action(&app); ui(&mut app, random);
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id, wanted);
}

#[test]
fn global_favorite_keeps_its_original_selection_key_and_binds_normally() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let f = favorite("Alpha"); let expected = format!("favorite:{}", f.identity());
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(f).unwrap();
    runtime.saved.favorites_selected = true; runtime.page = "favorites"; runtime.changed();
    let random = action(&app); ui(&mut app, random);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().unwrap().id, expected);
    assert!(runtime.saved.favorites_selected);
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("random-fixture"));
}

#[test]
fn missing_local_favorite_is_rejected_without_selecting_another_item() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(Favorite { title: "Detached file".into(),
        origin: Origin { provider: "local".into(), profile: "Local".into(), source_alias: "Detached".into(), subprofile: None },
        locator: Locator::Local { root_alias: "Detached".into(), relative_path: "Film.mp4".into() } }).unwrap();
    runtime.saved.favorites_selected = true; runtime.page = "favorites"; runtime.changed();
    let random = action(&app); ui(&mut app, random);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none()); assert!(runtime.resolver_next.is_none());
    assert!(!runtime.status.is_empty()); assert!(!runtime.saving);
}

#[test]
fn changed_source_or_navigation_rejects_old_action_before_play_mutation() {
    let _serial = plx_base::testlock::serial();
    for source in [false, true] {
        let mut app = mounted(); let old = action(&app);
        let runtime = app.ott.as_mut().unwrap();
        if source { runtime.generation += 1; } else { runtime.accepted_navigation(false); }
        let epoch = runtime.playback_epoch;
        ui(&mut app, old);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none()); assert_eq!(runtime.playback_epoch, epoch);
    }
}

#[test]
fn editing_saving_uncertain_empty_and_nonmedia_pages_have_no_random_entry() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    ui(&mut app, OttAction::BeginSearch);
    assert!(app.ott.as_ref().unwrap().view().random_context.is_none());
    let session = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    ui(&mut app, OttAction::CancelSearch { session });
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = true; assert!(runtime.view().random_context.is_none()); runtime.saving = false;
    runtime.persistence_uncertain = true; assert!(runtime.view().random_context.is_none()); runtime.persistence_uncertain = false;
    runtime.catalog.entries.clear(); assert!(runtime.view().random_context.is_none());
    runtime.catalog = catalog(); runtime.page = "settings"; assert!(runtime.view().random_context.is_none());
}

#[test]
fn ordinary_channel_pin_and_strict_kiosk_gate_the_resolved_play_action() {
    let _serial = plx_base::testlock::serial();
    for pin in [true, false] {
        let mut app = mounted();
        let runtime = app.ott.as_mut().unwrap();
        if pin {
            runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
            runtime.saved.settings.insert("psChannels".into(), json!(1));
        } else {
            let kiosk = crate::app::ott_kiosk_stored::Stored::item(favorite("Alpha"), true).unwrap();
            runtime.saved.settings.insert(crate::app::ott_kiosk_stored::KEY.into(), kiosk.encode().unwrap());
        }
        let random = action(&app); ui(&mut app, random);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none()); assert!(runtime.resolver_next.is_none());
        assert!(!runtime.status.is_empty()); assert!(!runtime.saving);
    }
}

#[test]
fn native_rebind_rejects_replaced_media_after_the_random_target_was_queued() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(); query(&mut app, "alpha");
    let random = action(&app); ui(&mut app, random);
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (_, profile, entry, _) = runtime.prepare_play_request(request).unwrap();
    let entry = entry.unwrap();
    runtime.catalog.entries.iter_mut().find(|row| row.id == entry.id).unwrap().media =
        MediaLocator::Http { url: "https://media.fixture.invalid/replaced.m3u8".into() };
    assert!(runtime.rebind_native_admission(&app, &profile, &entry).is_err());
    app.ott = Some(runtime);
}

#[test]
fn external_random_requests_remain_explicitly_unsupported() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let mut runtime = app.ott.take().unwrap();
    for (index, random) in [json!({}), json!({"search":"Alpha"}), json!({"range":[1, 3]})].into_iter().enumerate() {
        let now = runtime.now_ms();
        let body = json!({"commands":[], "server_time":1000, "requests":[{
            "id":format!("{:032x}", index + 1), "expires_at":1020,
            "action":"play_catalog", "params":{"catalog":"not-a-live-receipt", "random":random}
        }]}).to_string();
        let mut invocations = runtime.remote.raw_poll_for_test(&body, now);
        assert_eq!(invocations.len(), 1);
        let response = commands::command(&mut runtime, &mut app, &invocations.remove(0)).unwrap();
        assert_eq!(response["status"], "unsupported");
        assert!(runtime.play_target.is_none());
    }
    app.ott = Some(runtime);
}

//! Real Runtime/order/receipt paths; IO is limited to the existing temporary-root
//! fixture. Durable outcomes are explicitly modeled after a revoked writer.
use super::*;
use crate::app::ott_runtime::tests::local_integration;
use plx_data::ott::{catalog_from_local_files, LocalFile};

fn fixture(permanent: bool) -> (local_integration::TempDir, Box<App>) {
    let (directory, mut app, _) = local_integration::setup();
    app.running = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    for _ in 0..24 {
        app.loop_t += 16;
        bridge::frame(
            &mut app.pages,
            &mut app.bridge,
            plx_machine::machine::Tick {
                ms: app.loop_t,
                dt_us: 16_000,
            },
            Vec::new(),
        );
    }
    let mut runtime = app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut app);
    runtime.saving = false;
    runtime.entropy = Some([7; 16]);
    runtime.page = "channels";
    runtime.reply = app.pages.top_page();
    runtime.write_permit.revoke();
    let profile = runtime.effective_profile().unwrap().clone();
    if permanent {
        runtime.transient.accepted = None;
        runtime.saved.profiles.profiles.push(profile.clone());
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.provider = ProviderKind::Local;
        runtime.catalog = catalog_from_local_files(
            &["a", "b", "c"].map(|name| LocalFile {
                relative_path: format!("{name}.mp4"),
                size: 1,
                duration_ms: None,
            }),
            &profile,
        )
        .unwrap();
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
    } else {
        let first = runtime.catalog.entries[0].clone();
        runtime.catalog.entries = ["first", "second", "third"]
            .into_iter()
            .map(|id| {
                let mut entry = first.clone();
                entry.id = id.into();
                entry.title = id.into();
                entry.media = MediaLocator::Local {
                    relative_path: format!("{id}.mp4"),
                };
                entry
            })
            .collect();
        transient::accepted_catalog_update(&mut runtime);
    }
    runtime.plex_refresh_pending = true;
    runtime.sync_saved_context();
    app.ott = Some(runtime);
    (directory, app)
}
fn action(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let opens = matches!(action, OttAction::ShowActions(_));
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    if opens && result.is_ok() {
        runtime.accepted_navigation(false);
    }
    app.ott = Some(runtime);
    result
}
fn open(app: &mut App) -> u64 {
    let key = app
        .ott
        .as_ref()
        .unwrap()
        .local_order_key()
        .expect("current Local actions");
    action(app, OttAction::ShowActions(key)).unwrap();
    app.ott
        .as_ref()
        .unwrap()
        .organization
        .order_menu
        .as_ref()
        .unwrap()
        .revision
}
fn rows(runtime: &Runtime) -> Vec<String> {
    runtime
        .raw_view()
        .rows
        .into_iter()
        .filter_map(|row| {
            if let OttAction::Play(id) = row.action {
                Some(id)
            } else {
                None
            }
        })
        .collect()
}
fn candidate(runtime: &Runtime, revision: u64) -> Saved {
    let context = &runtime.organization.order_menu.as_ref().unwrap().context;
    let entropy = runtime.entropy.unwrap();
    let seed = u32::from_le_bytes(entropy[..4].try_into().unwrap())
        ^ (revision as u32).wrapping_mul(0x9e3779b9);
    let mut document = Organization::read(&runtime.saved.settings).unwrap();
    document
        .apply(
            &Source::new(&context.profile).unwrap(),
            &runtime.catalog,
            Operation::ShuffleOrder {
                category: category(runtime, context).unwrap(),
                seed: seed.max(1),
            },
        )
        .unwrap();
    let mut saved = runtime.saved.clone();
    document.write_into(&mut saved.settings).unwrap();
    saved
}
fn writer(app: &App) {
    assert!(
        matches!(
            app.ott
                .as_ref()
                .unwrap()
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            Job::Saved(Err(_), None, false, _)
        ),
        "revoked writer cannot persist test data"
    );
}
fn deliver(app: &mut App, result: Result<Saved, &'static str>) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            result,
            None,
            false,
            ok(json!({"persisted":true})),
        ))
        .unwrap();
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(app, sample);
}
// Use the same dispatch and receipt path as an authenticated read request;
// command-child modules intentionally remain private to the adapter.
fn request(app: &mut App, serial: u32, action: &str, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":format!("{serial:032x}"), "expires_at":1020, "action":action, "params":params
    }]})
    .to_string();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(
        incoming.len(),
        1,
        "fresh unique request must enter dispatch"
    );
    let inv = incoming.pop().unwrap();
    assert_eq!(inv.deadline_ms, now + 20_000);
    assert!(runtime.remote.invocation_is_current(&inv));
    let result = commands::command(&mut runtime, app, &inv).expect("synchronous read query");
    runtime.remote.complete(inv, result.clone());
    app.ott = Some(runtime);
    result
}
fn categories(app: &mut App, serial: u32) -> Value {
    let params = json!({"op":"list", "runtime":app.ott.as_ref().unwrap().remote.runtime(),
        "offset":0, "limit":200});
    request(app, serial, "categories", params)
}
fn channels(app: &mut App, serial: u32, catalog: &Value) -> Value {
    let params = json!({"runtime":app.ott.as_ref().unwrap().remote.runtime(), "catalog":catalog,
        "category":"all", "offset":0, "limit":200});
    request(app, serial, "channels", params)
}
fn channel_ids(response: &Value) -> Vec<String> {
    assert_eq!(response["status"], "ok");
    response["data"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}
#[test]
fn temporary_reorder_updates_ui_controls_and_catalog_fence_without_touching_saved_or_native() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(false);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let native = plx_media::route::play_request_ticket();
    let generation = app.ott.as_ref().unwrap().play_generation;
    let before_catalog = categories(&mut app, 1);
    assert_eq!(before_catalog["status"], "ok");
    let catalog = before_catalog["data"]["catalog"].clone();
    assert!(catalog.is_string());
    let revision = open(&mut app);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    let stale = channels(&mut app, 2, &catalog);
    assert_eq!(stale["status"], "rejected");
    assert_eq!(stale["data"]["error"], "catalog_changed");
    // Empty-parameter legacy channels dispatches through the controls projection.
    let controls = request(&mut app, 3, "channels", json!({}));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(rows(runtime), runtime.ordered_ids());
    assert_eq!(channel_ids(&controls), runtime.ordered_ids());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.play_generation, generation);
    assert!(plx_media::route::play_request_ticket() == native);
    assert!(!runtime.saving);
}
#[test]
fn close_actions_and_stale_menu_do_not_restart_or_replace_temporary_session() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(false);
    let original = app.ott.as_ref().unwrap().ordered_ids();
    let revision = open(&mut app);
    action(&mut app, OttAction::CloseActions).unwrap();
    assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err());
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_some());
    let revision = open(&mut app);
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err());
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
}
#[test]
fn withdrawn_root_and_favorites_do_not_offer_or_apply_local_order() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(false);
    let revision = open(&mut app);
    app.ott.as_mut().unwrap().local_inventory.sources[0].withdraw();
    assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err());
    app.ott.as_mut().unwrap().saved.favorites_selected = true;
    app.ott.as_mut().unwrap().page = "favorites";
    assert!(app.ott.as_ref().unwrap().local_order_key().is_none());
}
#[test]
fn durable_shuffle_publishes_only_after_saved_ack_and_preserves_profile_and_route() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(true);
    let before = app.ott.as_ref().unwrap().saved.clone();
    let order = app.ott.as_ref().unwrap().ordered_ids();
    let native = plx_media::route::play_request_ticket();
    let revision = open(&mut app);
    let saved = candidate(app.ott.as_ref().unwrap(), revision);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), order);
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, before.settings);
    deliver(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.profiles, before.profiles);
    assert_eq!(runtime.saved.favorites, before.favorites);
    assert_eq!(runtime.saved.category, before.category);
    assert_eq!(runtime.page, "channels");
    assert_eq!(rows(runtime), runtime.ordered_ids());
    assert!(plx_media::route::play_request_ticket() == native);
}
#[test]
fn failed_saved_ack_keeps_order_and_menu_retryable_and_late_ack_keeps_new_page() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(true);
    let original = app.ott.as_ref().unwrap().ordered_ids();
    let revision = open(&mut app);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    deliver(&mut app, Err("Fixture writer rejected"));
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
    assert_eq!(app.ott.as_ref().unwrap().page, "actions");
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .local_order_view()
        .rows
        .iter()
        .any(|row| row.id == "order.shuffle" && row.enabled));
    let saved = candidate(app.ott.as_ref().unwrap(), revision);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    app.ott.as_mut().unwrap().page = "profiles";
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    deliver(&mut app, Ok(saved));
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
}
#[test]
fn same_descriptor_new_root_grant_rejects_captured_order_action() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(true);
    let revision = open(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let descriptor = runtime.local_inventory.sources[0].descriptor().clone();
    let replacement = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        descriptor,
        &mut || true,
    )
    .unwrap();
    runtime
        .local_inventory
        .install(vec![replacement], runtime.generation);
    assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err());
}

#[test]
fn remote_catalog_uses_reordered_indices_and_rejects_the_old_receipt() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = fixture(false);
    let original = categories(&mut app, 1)["data"]["catalog"].clone();
    assert!(original.is_string());
    let revision = open(&mut app);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    let stale = channels(&mut app, 2, &original);
    assert_eq!(stale["status"], "rejected");
    assert_eq!(stale["data"]["error"], "catalog_changed");
    let fresh = categories(&mut app, 3)["data"]["catalog"].clone();
    let result = channels(&mut app, 4, &fresh);
    let actual = channel_ids(&result);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(actual, runtime.ordered_ids());
    assert_eq!(actual, rows(runtime));
}

// These ordinary-provider cases require the companion Organization model patch.
// Catalog records are synthetic metadata; no provider or decoder is contacted.
fn saved_provider_fixture(kind: ProviderKind) -> (local_integration::TempDir, Box<App>) {
    let (directory, mut app) = fixture(true);
    let runtime = app.ott.as_mut().unwrap();
    let profile = ProviderProfile {
        id: "order-provider".into(), name: "Saved provider".into(), kind,
        source: match kind {
            ProviderKind::M3u => "https://fixture.invalid/list.m3u",
            ProviderKind::Xtream => r#"{"server":"https://fixture.invalid","username":"fixture","password":"fixture"}"#,
            ProviderKind::Stalker => r#"{"portal":"https://fixture.invalid","mac":"00:1A:79:00:00:01"}"#,
            ProviderKind::Plex => "fixture-server",
            ProviderKind::VPortal => "portal::[key:fixture]https://fixture.invalid/portal",
            _ => panic!("ordinary provider fixture"),
        }.into(),
        source_alias: "Saved provider".into(), enabled: true,
    };
    profile.validate().unwrap();
    let mut parser_profile = profile.clone();
    parser_profile.kind = ProviderKind::M3u;
    parser_profile.source = "https://fixture.invalid/list.m3u".into();
    let mut catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Group\",One\nhttps://fixture.invalid/one\n\
         #EXTINF:-1 group-title=\"Group\",Two\nhttps://fixture.invalid/two\n\
         #EXTINF:-1 group-title=\"Group\",Three\nhttps://fixture.invalid/three\n\
         #EXTINF:-1 group-title=\"Other\",Four\nhttps://fixture.invalid/four\n",
        &parser_profile,
    ).unwrap();
    for (index, entry) in catalog.entries.iter_mut().enumerate() {
        let id = (index + 1).to_string();
        if kind == ProviderKind::Plex {
            entry.id = format!("{}:{id}", profile.id);
        }
        entry.media = match kind {
            ProviderKind::M3u => entry.media.clone(),
            ProviderKind::Xtream => MediaLocator::Xtream { stream_id: id,
                stream_kind: "live".into(), extension: "ts".into(), series_id: None,
                has_direct_source: false },
            ProviderKind::Stalker => MediaLocator::Stalker {
                cmd: format!("ffmpeg http://localhost/ch/{id}"),
                stream_kind: "itv".into(), series: "0".into() },
            ProviderKind::Plex => MediaLocator::Plex { server_id: profile.source.clone(), rating_key: id },
            ProviderKind::VPortal => MediaLocator::VPortal { request: json!({"id":id}) },
            _ => unreachable!(),
        };
    }
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.category = "Group".into();
    runtime.saved.favorites_selected = false;
    runtime.catalog = catalog;
    runtime.catalog_revision += 1;
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.sync_saved_context();
    (directory, app)
}

#[test]
fn saved_provider_shuffle_and_reset_commit_only_after_ack_without_changing_source_or_membership() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
        ProviderKind::Plex, ProviderKind::VPortal] {
        let (_directory, mut app) = saved_provider_fixture(kind);
        let before = app.ott.as_ref().unwrap().saved.clone();
        let raw = app.ott.as_ref().unwrap().catalog.clone();
        let original = app.ott.as_ref().unwrap().ordered_ids();
        let other = app.ott.as_ref().unwrap().organization_rows("Other")
            .iter().map(|(_, entry)| entry.id.clone()).collect::<Vec<_>>();
        let revision = open(&mut app);
        let saved = candidate(app.ott.as_ref().unwrap(), revision);
        action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
        writer(&app);
        assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
        assert_eq!(app.ott.as_ref().unwrap().saved.settings, before.settings);
        deliver(&mut app, Ok(saved));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert_eq!(runtime.saved.profiles, before.profiles);
        assert_eq!(runtime.saved.favorites, before.favorites);
        assert_eq!(runtime.saved.category, before.category);
        assert_eq!(runtime.catalog, raw);
        let mut members = runtime.ordered_ids();
        let mut baseline = original.clone();
        members.sort(); baseline.sort();
        assert_eq!(members, baseline);
        assert_eq!(runtime.organization_rows("Other").iter()
            .map(|(_, entry)| entry.id.clone()).collect::<Vec<_>>(), other);
        assert_eq!(rows(runtime), runtime.ordered_ids());

        let revision = open(&mut app);
        assert!(app.ott.as_ref().unwrap().local_order_view().rows.iter()
            .any(|row| row.id == "order.reset" && row.enabled));
        let runtime = app.ott.as_ref().unwrap();
        let context = &runtime.organization.order_menu.as_ref().unwrap().context;
        let mut document = Organization::read(&runtime.saved.settings).unwrap();
        document.apply(&Source::new(&context.profile).unwrap(), &runtime.catalog,
            Operation::ResetOrder { category: category(runtime, context).unwrap() }).unwrap();
        let mut reset = runtime.saved.clone();
        document.write_into(&mut reset.settings).unwrap();
        action(&mut app, OttAction::ResetOrder(revision)).unwrap();
        writer(&app);
        deliver(&mut app, Ok(reset));
        assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
    }
}

#[test]
fn saved_provider_menu_rejects_replaced_source_catalog_owner_and_overlay_before_writing() {
    let _serial = plx_base::testlock::serial();
    for case in 0..8 {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        let revision = open(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        match case {
            0 => runtime.generation += 1,
            1 => runtime.catalog_revision += 1,
            2 => runtime.navigation_epoch += 1,
            3 => runtime.reply = None,
            4 => runtime.cancel = Arc::new(AtomicBool::new(false)),
            5 => runtime.saved.profiles.profiles[0].source = "https://other.invalid/list.m3u".into(),
            6 => { runtime.saved.settings.insert(catalog_organization::KEY.into(), json!({"version":2})); }
            _ => {
                let original = runtime.saved.profiles.profiles[0].clone();
                runtime.saved.profiles.profiles[0].name = "Other source view".into();
                runtime.sync_saved_context();
                runtime.saved.profiles.profiles[0] = original;
                runtime.sync_saved_context();
            }
        }
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let raw = runtime.catalog.clone();
        assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err(), "case {case}");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(runtime.catalog, raw);
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
    }
}

#[test]
fn saved_provider_actions_require_ready_saved_catalog_and_exclude_favorites() {
    let _serial = plx_base::testlock::serial();
    for case in 0..9 {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        assert!(app.ott.as_ref().unwrap().local_order_key().is_some());
        let runtime = app.ott.as_mut().unwrap();
        match case {
            0 => runtime.loaded = false,
            1 => runtime.persistence_uncertain = true,
            2 => runtime.saving = true,
            3 => runtime.catalog_complete = false,
            4 => runtime.catalog_loading = true,
            5 => runtime.saved.profiles.profiles.clear(),
            6 => runtime.saved.favorites_selected = true,
            7 => runtime.cancel.store(true, Ordering::Release),
            _ => { runtime.catalog.entries[0].id = "another-profile:0000000000000001".into(); runtime.catalog_revision += 1; }
        }
        assert!(runtime.local_order_key().is_none(), "case {case}");
    }
}

#[test]
fn saved_provider_failed_write_remains_retryable_and_late_ack_cannot_navigate() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let original = app.ott.as_ref().unwrap().ordered_ids();
    let revision = open(&mut app);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    deliver(&mut app, Err("Fixture writer rejected"));
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), original);
    assert_eq!(app.ott.as_ref().unwrap().page, "actions");
    assert!(app.ott.as_ref().unwrap().local_order_view().rows.iter()
        .any(|row| row.id == "order.shuffle" && row.enabled));
    let saved = candidate(app.ott.as_ref().unwrap(), revision);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    app.ott.as_mut().unwrap().page = "profiles";
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    deliver(&mut app, Ok(saved));
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
}

#[test]
fn saved_provider_reorder_preserves_an_installed_playback_owner() {
    let _serial = plx_base::testlock::serial();
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            plx_media::player::SHARED.reset_session();
            plx_media::player::TX.reset();
            plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        }
    }
    let _reset = Reset;
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    local_integration::install(&mut app, None, "live");
    bridge::show_page(&mut app.pages, AppArg::Ott);
    for _ in 0..24 {
        app.loop_t += 16;
        bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: app.loop_t, dt_us: 16_000 }, Vec::new());
    }
    app.ott.as_mut().unwrap().reply = app.pages.top_page();
    let before = app.ott.as_ref().unwrap().playing.clone().unwrap();
    assert!(playing_matches(&before, &app));
    let ticket = plx_media::route::play_request_ticket();
    let generation = app.ott.as_ref().unwrap().play_generation;
    let revision = open(&mut app);
    let saved = candidate(app.ott.as_ref().unwrap(), revision);
    action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
    writer(&app);
    deliver(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    let after = runtime.playing.as_ref().unwrap();
    assert!(playing_matches(after, &app));
    assert!(after.identity == before.identity);
    assert_eq!(after.profile, before.profile);
    assert_eq!(after.entry, before.entry);
    assert_eq!(after.selection_id, before.selection_id);
    assert_eq!(runtime.play_generation, generation);
    assert!(plx_media::route::play_request_ticket() == ticket);
}

#[test]
fn saved_provider_menu_is_retired_by_an_actual_transient_source_request() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let revision = open(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":format!("{:032x}", 77), "expires_at":1020, "action":"local_media",
        "params":{"op":"sources", "runtime":runtime.remote.runtime()}
    }]}).to_string();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(incoming.len(), 1);
    let invocation = incoming.pop().unwrap();
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    assert!(runtime.transient.candidate.is_some());
    app.ott = Some(runtime);
    assert!(action(&mut app, OttAction::ShuffleOrder(revision)).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
}

#[test]
fn saved_provider_pending_ack_cannot_reopen_an_aba_source_owner() {
    let _serial = plx_base::testlock::serial();
    for persisted in [false, true] {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        let revision = open(&mut app);
        let saved = candidate(app.ott.as_ref().unwrap(), revision);
        let before = app.ott.as_ref().unwrap();
        let authority = before.organization.authority;
        let navigation = before.navigation_epoch;
        let generation = before.generation;
        let catalog_revision = before.catalog_revision;
        let profile_epoch = before.profile_edit_epoch;
        let cancel = before.cancel.clone();
        action(&mut app, OttAction::ShuffleOrder(revision)).unwrap();
        writer(&app);

        // The source returns to the same bytes while this write is pending.
        // Its observed authority must still retire the old menu after the ACK.
        let runtime = app.ott.as_mut().unwrap();
        let profile = runtime.saved.profiles.profiles[0].clone();
        runtime.saved.profiles.profiles[0].name = "Other source view".into();
        runtime.sync_saved_context();
        runtime.saved.profiles.profiles[0] = profile;
        runtime.sync_saved_context();
        assert_eq!(runtime.organization.authority, authority + 2);
        assert_eq!(runtime.navigation_epoch, navigation);
        assert_eq!(runtime.generation, generation);
        assert_eq!(runtime.catalog_revision, catalog_revision);
        assert_eq!(runtime.profile_edit_epoch, profile_epoch);
        assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
        assert!(runtime.saving);

        deliver(&mut app, if persisted { Ok(saved) } else { Err("Fixture writer rejected") });
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "actions", "a stale ACK must not navigate");
        assert!(!runtime.saving);
        assert!(runtime.organization.order_pending.is_none());
        assert!(runtime.local_order_view().rows.iter()
            .filter(|row| row.id == "order.shuffle" || row.id == "order.reset")
            .all(|row| !row.enabled));
    }
}

#[path = "ott_list_order_context_tests.rs"]
mod contextual;


fn classic_actions(app: &mut App) -> OttAction {
    let runtime = app.ott.as_mut().unwrap();
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    open(app);
    app.ott.as_ref().unwrap().local_order_view().rows.into_iter()
        .find(|row| row.id == "order.search" && row.enabled).expect("Classic Actions Search").action
}

#[test]
fn classic_actions_search_uses_the_captured_list_without_mutating_saved_or_media() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::Plex, ProviderKind::VPortal] {
        let (_directory, mut app) = saved_provider_fixture(kind);
        let search = classic_actions(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let generation = runtime.play_generation;
        let ticket = plx_media::route::play_request_ticket();
        let mut runtime = app.ott.take().unwrap();
        apply_ui_action(&mut runtime, &mut app, search, true, false, false);
        app.ott = Some(runtime);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert!(runtime.organization.order_menu.is_none());
        assert!(matches!(&runtime.edit_session.as_ref().expect("real search editor").purpose,
            search::EditorPurpose::Search { .. }));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.play_generation, generation);
        assert_eq!(plx_media::route::play_request_ticket(), ticket);
        assert!(!runtime.saving);
    }
}

#[test]
fn classic_actions_search_rejects_stale_nonce_source_navigation_and_mode_before_page_change() {
    let _serial = plx_base::testlock::serial();
    for change in 0..6 {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        let mut search = classic_actions(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        match change {
            0 => search = OttAction::EditSetting("order.0.search".into()),
            1 => runtime.saved.profiles.profiles[0].source = "https://fixture.invalid/replaced.m3u".into(),
            2 => runtime.navigation_epoch += 1,
            3 => { runtime.saved.settings.insert("interfaceMode".into(), json!(1)); }
            4 => runtime.saving = true,
            _ => runtime.saved = menu_visibility::candidate(&runtime.saved,
                plx_appkit::menu_visibility::Change::Set {
                    item: plx_appkit::menu_visibility::Item::Search, visible: false,
                }).unwrap(),
        }
        let before = serde_json::to_value(&runtime.saved).unwrap();
        assert!(action(&mut app, search).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "actions");
        assert!(runtime.organization.order_menu.is_some());
        assert!(runtime.edit_session.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn classic_actions_search_failed_editor_admission_keeps_its_menu_and_transient_search_stays_available() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let search = classic_actions(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.edit_serial = u64::MAX;
    let revision = runtime.organization.order_menu.as_ref().unwrap().revision;
    assert!(action(&mut app, search).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "actions");
    assert_eq!(runtime.organization.order_menu.as_ref().unwrap().revision, revision);
    assert!(runtime.edit_session.is_none());

    let (_directory, mut app) = fixture(false);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.truncate(1);
        transient::accepted_catalog_update(runtime);
        runtime.public_preferences.accept(None);
        runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    }
    // A one-row temporary list still has read-only Classic Search in Actions.
    let search = classic_actions(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.local_order_view().rows.iter()
        .filter(|row| row.id == "order.shuffle" || row.id == "order.reset")
        .all(|row| !row.enabled));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    action(&mut app, search).unwrap();
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(runtime.edit_session.is_some());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);

    let (_directory, mut app) = fixture(false);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries.truncate(1);
    transient::accepted_catalog_update(runtime);
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    assert!(runtime.local_order_key().is_none(), "Modern menu eligibility is unchanged");
    assert!(runtime.search_view().is_some());
}

#[test]
fn classic_actions_hidden_search_is_not_reintroduced_for_any_list_size() {
    let _serial = plx_base::testlock::serial();
    for count in [1, 3] {
        let (_directory, mut app) = fixture(false);
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.truncate(count);
        transient::accepted_catalog_update(runtime);
        runtime.public_preferences.accept(None);
        runtime.saved.settings.insert("interfaceMode".into(), json!(0));
        runtime.saved = menu_visibility::candidate(&runtime.saved,
            plx_appkit::menu_visibility::Change::Set {
                item: plx_appkit::menu_visibility::Item::Search, visible: false,
            }).unwrap();
        if count == 1 {
            assert!(runtime.local_order_key().is_none());
        } else {
            open(&mut app);
            assert!(!app.ott.as_ref().unwrap().local_order_view().rows.iter()
                .any(|row| row.id == "order.search"));
        }
    }
}

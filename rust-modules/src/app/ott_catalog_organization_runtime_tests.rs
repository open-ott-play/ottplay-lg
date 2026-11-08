//! Real OTT actions, shared form input and addressed Saved completion. The
//! revoked writer is intercepted; durable outcomes below are explicitly modeled.
//! No provider, decoder, protected read or successful protected write is needed.
use super::*;
use crate::app::{bridge, run};
use plx_machine::machine::{InputEvent, InputKind, Key, Source as InputSource, TextEdit, Tick};
use plx_screens::registry::AppFx;
use std::time::Duration;

#[path = "ott_local_organization_runtime_tests.rs"]
mod local_organization;

#[test]
fn alphabetical_manage_rejects_member_moves_from_saved_state_without_a_writer() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.reply = Some(InstanceId(412));
    let source = Source::new(&profile()).unwrap();
    let mut document = Organization::default();
    document.apply(&source, &runtime.catalog, Operation::SetAlphabetical { enabled: true }).unwrap();
    document.write_into(&mut runtime.saved.settings).unwrap();
    runtime.organization_open().unwrap();
    assert!(runtime.organization_view().unwrap().alphabetical);
    let mut session = runtime.organization.session.clone().unwrap();
    let category = session.projection.categories.iter().find(|row| row.label == "News").unwrap().id.clone();
    let item = runtime.catalog.entries.iter().find(|entry| entry.title == "Beta").unwrap().id.clone();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let catalog = runtime.catalog.clone();
    // Backend admission must read the authoritative document, not trust a UI flag.
    session.alphabetical = false;
    for direction in [Direction::Up, Direction::Down] {
        assert!(runtime.organization_candidate(&session, Operation::MoveMember {
            category: category.clone(), item: item.clone(), direction,
        }).is_err());
    }
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.catalog, catalog);
    assert!(!runtime.saving && runtime.organization.pending.is_none());
    assert!(runtime.rx.try_recv().is_err());
    assert!(runtime.organization_candidate(&session, Operation::MoveCategory {
        category: category.clone(), direction: Direction::Down,
    }).unwrap().is_some());
    assert!(runtime.organization_candidate(&session, Operation::RemoveMember {
        category: category.clone(), item: item.clone(),
    }).unwrap().is_some());

    document.apply(&source, &runtime.catalog, Operation::SetAlphabetical { enabled: false }).unwrap();
    document.write_into(&mut runtime.saved.settings).unwrap();
    runtime.organization_open().unwrap();
    assert!(!runtime.organization_view().unwrap().alphabetical);
    let session = runtime.organization.session.as_ref().unwrap();
    assert!(runtime.organization_candidate(session, Operation::MoveMember {
        category, item, direction: Direction::Up,
    }).unwrap().is_some());
}

#[test]
fn manage_alpha_snapshot_changes_only_after_an_accepted_reset() {
    let _serial = plx_base::testlock::serial();
    for persisted in [false, true] {
        let mut app = fixture();
        let mut runtime = app.ott.take().unwrap();
        let source = Source::new(&profile()).unwrap();
        let mut document = Organization::default();
        document.apply(&source, &runtime.catalog, Operation::SetAlphabetical { enabled: true }).unwrap();
        document.write_into(&mut runtime.saved.settings).unwrap();
        runtime.organization_open().unwrap();
        let session = runtime.organization.session.clone().unwrap();
        let revision = session.revision;
        let candidate = runtime.organization_candidate(&session, Operation::Reset).unwrap().unwrap();
        runtime.organization.pending = Some(Pending { session,
            overlay: candidate.settings.get(catalog_organization::KEY).cloned() });
        assert!(runtime.organization_view().unwrap().alphabetical);
        assert!(runtime.organization_view().unwrap().pending);
        // Model the writer acknowledgement; this fixture performs no protected write.
        if persisted { runtime.saved = candidate; }
        runtime.observe_organization_context();
        assert!(runtime.organization_view().unwrap().alphabetical, "pending view keeps the committed snapshot");
        runtime.finish_organization_save(&app, persisted);
        let view = runtime.organization_view().unwrap();
        assert_eq!(view.alphabetical, !persisted);
        assert!(!view.pending);
        assert_eq!(view.revision > revision, persisted);
    }
}

fn file_profile(kind: ProviderKind) -> ProviderProfile {
    let mut result = profile();
    result.id = "file-library".into();
    result.kind = kind;
    result.source = if kind == ProviderKind::Plex { "organization-machine" }
        else { "portal::[key:fixture]https://portal.fixture.invalid/api/" }.into();
    result
}

fn file_catalog(profile: &ProviderProfile) -> Catalog {
    if profile.kind == ProviderKind::VPortal {
        return plx_data::ott::parse_vportal_response(&json!({"type":"videoportal","items":[
            {"type":"category","id":"first-folder","title":"Folder","items":[
                {"type":"stream","id":"first","title":"First","request":{"id":"first"}}]},
            {"type":"category","id":"second-folder","title":"Folder","items":[
                {"type":"stream","id":"second","title":"Second","request":{"id":"second"}}]}
        ]}).to_string(), profile, "").unwrap().catalog;
    }
    let entries: Vec<_> = ["42", "43"].iter().map(|key| {
        let mut entry = raw_catalog().entries.remove(0);
        entry.id = format!("{}:{key}", profile.id);
        entry.title = format!("Film {key}");
        entry.category = "Folder".into();
        entry.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: (*key).into() };
        entry
    }).collect();
    let hierarchy = plx_data::ott::CategoryHierarchy {
        nodes: ["first-folder", "second-folder"].iter().map(|id| plx_data::ott::CategoryNode {
            id: (*id).into(), label: "Folder".into(), parent: None, anonymous_fingerprint: None,
            source_identity: None,
        }).collect(),
        memberships: entries.iter().zip(["first-folder", "second-folder"])
            .map(|(entry, id)| plx_data::ott::CategoryMembership {
                entry_id: entry.id.clone(), category_id: id.into(),
            }).collect(),
    };
    Catalog { entries, hierarchy: Some(hierarchy) }
}

#[test]
fn saved_file_provider_organization_keeps_raw_media_and_global_favorites() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::Plex, ProviderKind::VPortal] {
        let mut runtime = runtime_fixture();
        let profile = file_profile(kind);
        runtime.provider = kind;
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.category.clear();
        runtime.catalog = file_catalog(&profile);
        runtime.reply = Some(InstanceId(411));
        runtime.selected = Some(runtime.catalog.entries[0].id.clone());
        runtime.sync_saved_context();
        let catalog = runtime.catalog.clone();
        let favorites = runtime.saved.favorites.clone();
        let profiles = runtime.saved.profiles.clone();
        let selected = runtime.selected.clone();
        let epoch = runtime.playback_epoch;
        assert!(runtime.organization_available());
        runtime.organization_open().unwrap();
        let session = runtime.organization.session.as_ref().unwrap();
        assert_eq!(session.projection.categories.len(), 2, "same labels retain separate identities");
        let category = session.projection.categories[0].id.clone();
        let candidate = runtime.organization_candidate(session,
            Operation::Copy { category, label: "Copied".into() }).unwrap().unwrap();
        assert_eq!(candidate.profiles, profiles);
        assert_eq!(candidate.favorites, favorites);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.selected, selected);
        assert_eq!(runtime.playback_epoch, epoch);
        assert!(runtime.play_target.is_none() && !runtime.saving);
        let document = Organization::read(&candidate.settings).unwrap();
        let projection = document.project(&Source::new(&profile).unwrap(), &catalog).unwrap();
        assert_eq!(projection.categories.iter().find(|row| row.label == "Copied").unwrap().members.len(), 1);
        assert_eq!(projection.all, catalog.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
        runtime.saved = candidate;
        runtime.saved.favorites_selected = true;
        assert!(!runtime.organization_available(), "mixed Favorites remains its own owner");
        assert_eq!(runtime.saved.favorites, favorites);
    }
}

struct OrganizationRegistry;
impl Drop for OrganizationRegistry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}
fn classic_organization_fixture(durable: bool) -> (App, Runtime) {
    plx_plex::plex::reset_servers_for_test();
    plx_plex::plex::grant::reset_for_test();
    let mut app = fixture();
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = 31;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = bridge::Bridge::for_session_test(init);
    let origin = plx_plex::plex::Origin::parse("https://organization.fixture.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        "organization-machine", &origin, "synthetic-token", None,
        "synthetic-client", Default::default());
    assert!(plx_plex::plex::set_current(sid));
    let mut runtime = app.ott.take().unwrap();
    if durable { runtime.saved.profiles.profiles.push(file_profile(ProviderKind::Plex)); }
    assert!(classic_source::ensure_server_catalog(&mut runtime, &mut app, sid).unwrap());
    let profile = runtime.effective_profile().unwrap().clone();
    runtime.catalog = file_catalog(&profile);
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.reply = app.pages.top_page();
    runtime.observe_organization_context();
    (app, runtime)
}

#[test]
fn inactive_saved_classic_organization_remaps_only_its_source_after_commit() {
    let _serial = plx_base::testlock::serial();
    let _registry = OrganizationRegistry;
    let (app, mut runtime) = classic_organization_fixture(true);
    let profiles = runtime.saved.profiles.clone();
    let saved_category = runtime.saved.category.clone();
    let favorites = runtime.saved.favorites.clone();
    let catalog = runtime.catalog.clone();
    let selected = runtime.selected.clone();
    let epoch = runtime.playback_epoch;
    assert_eq!(runtime.provider, ProviderKind::Plex);
    assert_eq!(runtime.active().unwrap().kind, ProviderKind::M3u);
    assert_eq!(runtime.organization_profile().unwrap().kind, ProviderKind::Plex);
    let category = runtime.organization_projection().unwrap().categories[0].id.clone();
    classic_source::set_view(&mut runtime, &category);
    runtime.organization_open().unwrap();
    let session = runtime.organization.session.clone().unwrap();
    let candidate = runtime.organization_candidate(&session, Operation::Delete { category }).unwrap().unwrap();
    assert_eq!(candidate.profiles, profiles);
    assert_eq!(candidate.category, saved_category);
    assert_eq!(candidate.favorites, favorites);
    runtime.organization.pending = Some(Pending { session,
        overlay: candidate.settings.get(catalog_organization::KEY).cloned() });
    // Model the sole writer's confirmed image without performing storage IO.
    runtime.saved = candidate;
    runtime.observe_organization_context();
    runtime.finish_organization_save(&app, true);
    assert_eq!(runtime.view_selection().0, "", "a deleted viewed category returns to All");
    assert_eq!(runtime.saved.category, saved_category);
    assert_eq!(runtime.saved.profiles, profiles);
    assert_eq!(runtime.saved.favorites, favorites);
    assert_eq!(runtime.catalog, catalog);
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.playback_epoch, epoch);
    assert!(!runtime.saving && runtime.play_target.is_none());
    let authority = runtime.organization.authority;
    runtime.saved.profiles.profiles[1].name = "Changed".into();
    runtime.observe_organization_context();
    assert!(runtime.organization_profile().is_none());
    runtime.saved.profiles = profiles;
    runtime.observe_organization_context();
    assert!(runtime.organization_profile().is_some());
    assert_eq!(runtime.organization.authority, authority + 2, "inactive source A-B-A retires drafts");
}

#[test]
fn unsaved_classic_cannot_create_persisted_organization_or_duplicate_favorites() {
    let _serial = plx_base::testlock::serial();
    let _registry = OrganizationRegistry;
    let (_app, mut runtime) = classic_organization_fixture(false);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.organization_profile().is_none());
    assert!(runtime.organization_projection().is_none());
    assert!(!runtime.organization_available());
    assert!(runtime.organization_open().is_err());
    assert_eq!(runtime.organization_categories().iter().map(|row| &row.id).collect::<Vec<_>>(),
        runtime.catalog.category_choices().iter().map(|row| &row.id).collect::<Vec<_>>());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving && runtime.organization.pending.is_none());
}

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "organization-fixture".into(),
        name: "Home".into(),
        kind: ProviderKind::M3u,
        source: "https://playlist.fixture.invalid/live.m3u".into(),
        source_alias: "Home".into(),
        enabled: true,
    }
}
fn raw_catalog() -> Catalog {
    plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"alpha\" group-title=\"News\",Alpha\nhttps://media.fixture.invalid/a.m3u8\n\
         #EXTINF:-1 tvg-id=\"beta\" group-title=\"News\",Beta\nhttps://media.fixture.invalid/b.m3u8\n\
         #EXTINF:-1 tvg-id=\"gamma\" group-title=\"Cinema\",Gamma\nhttps://media.fixture.invalid/c.m3u8\n",
        &profile(),
    ).unwrap()
}
fn favorite(title: &str) -> Favorite {
    Favorite {
        title: title.into(),
        origin: plx_data::ott::favorites::Origin {
            provider: "m3u".into(),
            profile: "Home".into(),
            source_alias: "Home".into(),
            subprofile: None,
        },
        locator: plx_data::ott::favorites::Locator::Channel {
            channel_id: title.to_lowercase(),
        },
    }
}
fn runtime_fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    // Saved-context reconciliation runs before the first frame, including restarts.
    let worker = plx_base::task::OffFrame::for_test();
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture(&worker)
        .expect("organization fixture clock"));
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.write_permit.revoke();
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    runtime.saved.profiles.profiles = vec![profile()];
    runtime.saved.profiles.active_profile = Some(profile().id);
    runtime.provider = ProviderKind::M3u;
    runtime.catalog = raw_catalog();
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.saved.category = "News".into();
    runtime.saved.favorites.toggle(favorite("Alpha")).unwrap();
    runtime.page = "categories";
    // Keep unrelated producer/refresh paths inert through real runtime ticks.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.sync_saved_context();
    runtime
}
fn next_tick(app: &mut App) -> Tick {
    app.loop_t = app.loop_t.wrapping_add(16);
    Tick {
        ms: app.loop_t,
        dt_us: 16_000,
    }
}
fn settle(app: &mut App) {
    for _ in 0..24 {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
}
fn runtime_tick(app: &mut App) {
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(app, sample);
}
fn fixture() -> App {
    let mut app = run::lifecycle_regression_tests::app();
    app.ott = Some(runtime_fixture());
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(&mut app);
    runtime_tick(&mut app);
    settle(&mut app);
    assert!(app.ott.as_ref().unwrap().organization_available());
    app
}
fn ui_action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle(app);
}
fn scoped(app: &mut App, op: &str) {
    let key = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .key(op);
    ui_action(app, OttAction::EditSetting(key));
}
fn open(app: &mut App) {
    ui_action(app, OttAction::EditSetting("organization.open".into()));
    assert_eq!(app.ott.as_ref().unwrap().page, "organization");
    assert!(app.ott.as_ref().unwrap().organization_view().is_some());
}
fn select_category(app: &mut App, title: &str) -> String {
    let view = app.ott.as_ref().unwrap().organization_view().unwrap();
    let index = view
        .categories
        .iter()
        .position(|row| row.title == title)
        .unwrap();
    let id = view.categories[index].id.clone();
    scoped(app, &format!("category:{index}"));
    id
}
fn item_id(app: &App, title: &str) -> String {
    app.ott
        .as_ref()
        .unwrap()
        .catalog
        .entries
        .iter()
        .find(|row| row.title == title)
        .unwrap()
        .id
        .clone()
}
fn press(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(
        &mut app.pages,
        &mut app.bridge,
        tick,
        bridge::script_key(key, tick),
    );
    runtime_tick(app);
    settle(app);
}
fn type_text(app: &mut App, value: &str) {
    let tick = next_tick(app);
    let input = [TextEdit::Clear, TextEdit::Commit(value.into())]
        .into_iter()
        .map(|edit| InputEvent {
            at: tick,
            source: InputSource::Script,
            kind: InputKind::Text(edit),
        })
        .collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    runtime_tick(app);
    settle(app);
}
fn editor_visible(app: &App) -> bool {
    app.pages
        .nav
        .top_page()
        .unwrap()
        .inst
        .as_ref()
        .unwrap()
        .screen
        .as_any()
        .unwrap()
        .downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap()
        .handles_private_text()
}
fn candidate(app: &App, operation: Operation) -> Saved {
    let runtime = app.ott.as_ref().unwrap();
    runtime
        .organization_candidate(runtime.organization.session.as_ref().unwrap(), operation)
        .unwrap()
        .unwrap()
}
fn take_writer(app: &App) -> Job {
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(runtime.organization.pending.is_some() || runtime.pending_category_choice.is_some());
    let result = runtime
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("bounded revoked writer completion");
    assert!(
        matches!(&result, Job::Saved(Err(_), None, false, _)),
        "revoked permit must not perform a protected write"
    );
    assert!(runtime.rx.try_recv().is_err());
    result
}
fn deliver(app: &mut App, job: Job) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(job)
        .unwrap_or_else(|_| panic!("runtime receiver missing"));
    runtime_tick(app);
    settle(app);
}
fn durable(app: &mut App, saved: Saved) {
    deliver(
        app,
        Job::Saved(Ok(saved), None, false, ok(json!({"persisted":true}))),
    );
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().organization.pending.is_none());
}
fn text_save(app: &mut App, op: &str, text: &str, operation: Operation) -> (Saved, Job) {
    scoped(app, op);
    assert!(editor_visible(app));
    type_text(app, text);
    let saved = candidate(app, operation);
    let before = app
        .ott
        .as_ref()
        .unwrap()
        .saved
        .settings
        .get(catalog_organization::KEY)
        .cloned();
    press(app, Key::Down);
    press(app, Key::Ok);
    let writer = take_writer(app);
    assert!(
        editor_visible(app),
        "shared Save waits for the addressed durable result"
    );
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .saved
            .settings
            .get(catalog_organization::KEY),
        before.as_ref()
    );
    (saved, writer)
}
fn mutate(app: &mut App, op: &str, operation: Operation) {
    let saved = candidate(app, operation);
    scoped(app, op);
    drop(take_writer(app));
    durable(app, saved);
}
fn remote(app: &mut App, serial: u32, action: &str, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020
    }]})
    .to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, 100);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
    let result = commands::command(&mut runtime, app, &invocation).unwrap();
    app.ott = Some(runtime);
    result
}
fn remote_ids(app: &mut App, serial: u32) -> Vec<String> {
    let response = remote(app, serial, "channels", json!({}));
    assert_eq!(response["status"], "ok");
    response["data"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().into())
        .collect()
}

#[test]
fn shared_form_save_creates_empty_category_only_after_saved_and_choice_survives_restart() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let old_revision = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .revision;
    let (saved, _) = text_save(
        &mut app,
        "create",
        "Quiet",
        Operation::Create {
            label: "Quiet".into(),
        },
    );
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .organization_categories()
        .iter()
        .any(|row| row.title == "Quiet"));
    durable(&mut app, saved);
    assert!(!editor_visible(&app));
    assert!(
        app.ott
            .as_ref()
            .unwrap()
            .organization_view()
            .unwrap()
            .revision
            > old_revision
    );
    let id = app
        .ott
        .as_ref()
        .unwrap()
        .organization_categories()
        .into_iter()
        .find(|row| row.title == "Quiet")
        .unwrap()
        .id;
    let selected = app
        .ott
        .as_ref()
        .unwrap()
        .category_choice_candidate(Some(&id))
        .unwrap();
    ui_action(&mut app, OttAction::SelectCategory(id.clone()));
    drop(take_writer(&app));
    durable(&mut app, selected);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.category, id);
    assert!(runtime.ordered_ids().is_empty());
    assert_eq!(runtime.page, "channels");
    let mut restarted = runtime_fixture();
    restarted.saved = runtime.saved.clone();
    restarted.sync_saved_context();
    assert_eq!(restarted.effective_category(), id);
    assert!(restarted.ordered_ids().is_empty());
    assert!(restarted
        .organization_categories()
        .iter()
        .any(|row| row.id == id));
}

#[test]
fn member_order_remove_and_add_drive_runtime_and_remote_without_changing_raw_catalog() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    // This is a browse-only edit, not the initial catalogue autoplay path.
    let runtime = app.ott.as_mut().unwrap();
    runtime.continuity.stop(runtime.generation);
    let raw = app.ott.as_ref().unwrap().catalog.clone();
    let favorites = app.ott.as_ref().unwrap().saved.favorites.clone();
    open(&mut app);
    let category = select_category(&mut app, "News");
    let alpha = item_id(&app, "Alpha");
    let beta = item_id(&app, "Beta");
    let gamma = item_id(&app, "Gamma");
    scoped(&mut app, "item:1");
    mutate(
        &mut app,
        "member.up",
        Operation::MoveMember {
            category: category.clone(),
            item: beta.clone(),
            direction: Direction::Up,
        },
    );
    assert_eq!(
        app.ott.as_ref().unwrap().ordered_ids(),
        [beta.clone(), alpha.clone()]
    );
    assert_eq!(remote_ids(&mut app, 1), [beta.clone(), alpha.clone()]);
    mutate(
        &mut app,
        "member.remove",
        Operation::RemoveMember {
            category: category.clone(),
            item: beta,
        },
    );
    assert_eq!(remote_ids(&mut app, 2), [alpha.clone()]);
    scoped(&mut app, "member.add");
    let index = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .items
        .iter()
        .position(|row| row.id == gamma)
        .unwrap();
    mutate(
        &mut app,
        &format!("add:{index}"),
        Operation::AddMember {
            category,
            item: gamma.clone(),
        },
    );
    assert_eq!(
        app.ott.as_ref().unwrap().ordered_ids(),
        [alpha.clone(), gamma.clone()]
    );
    assert_eq!(remote_ids(&mut app, 3), [alpha, gamma]);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog, raw);
    assert_eq!(runtime.saved.favorites, favorites);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn rename_copy_delete_and_reset_preserve_newer_favorites_and_category_identity() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let category = select_category(&mut app, "News");
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .favorites
        .toggle(favorite("Gamma"))
        .unwrap();
    let favorites = app.ott.as_ref().unwrap().saved.favorites.clone();
    let raw = app.ott.as_ref().unwrap().catalog.clone();
    let (saved, _) = text_save(
        &mut app,
        "rename",
        "Headlines",
        Operation::Rename {
            category: category.clone(),
            label: "Headlines".into(),
        },
    );
    assert_eq!(saved.favorites, favorites);
    durable(&mut app, saved);
    assert_eq!(app.ott.as_ref().unwrap().saved.category, category);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .organization_view()
            .unwrap()
            .category
            .unwrap()
            .title,
        "Headlines"
    );
    let (saved, _) = text_save(
        &mut app,
        "copy",
        "Evening",
        Operation::Copy {
            category: category.clone(),
            label: "Evening".into(),
        },
    );
    durable(&mut app, saved);
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .organization_categories()
        .iter()
        .any(|row| row.title == "Evening"));
    scoped(&mut app, "delete");
    mutate(
        &mut app,
        "confirm",
        Operation::Delete {
            category: category.clone(),
        },
    );
    assert!(app.ott.as_ref().unwrap().saved.category.is_empty());
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .organization_categories()
        .iter()
        .any(|row| row.id == category));
    scoped(&mut app, "reset");
    mutate(&mut app, "confirm", Operation::Reset);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime
        .organization_categories()
        .iter()
        .any(|row| row.id == category && row.title == "News"));
    assert!(!runtime
        .organization_categories()
        .iter()
        .any(|row| row.title == "Evening"));
    assert_eq!(runtime.saved.favorites, favorites);
    assert_eq!(runtime.catalog, raw);
}

#[test]
fn failed_and_uncertain_save_do_not_publish_draft_and_back_remains_available() {
    let _serial = plx_base::testlock::serial();
    for uncertain in [false, true] {
        let mut app = fixture();
        open(&mut app);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let (_, failure) = text_save(
            &mut app,
            "create",
            "Uncommitted",
            Operation::Create {
                label: "Uncommitted".into(),
            },
        );
        deliver(
            &mut app,
            if uncertain {
                Job::Uncertain(None, None)
            } else {
                failure
            },
        );
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        assert!(runtime.organization.pending.is_none());
        assert_eq!(runtime.persistence_uncertain, uncertain);
        assert!(editor_visible(&app));
        press(&mut app, Key::Back);
        assert!(!editor_visible(&app));
    }
}

#[test]
fn cancelled_editor_rejects_old_submit_without_writer_or_overlay() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    scoped(&mut app, "create");
    let old = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    type_text(&mut app, "Discarded");
    press(&mut app, Key::Back);
    assert!(!editor_visible(&app));
    let mut runtime = app.ott.take().unwrap();
    assert!(handle_action(
        &mut runtime,
        &mut app,
        OttAction::SubmitSetting {
            session: old.id,
            key: old.key,
            value: "Discarded".into()
        }
    )
    .is_err());
    assert!(!runtime.saving);
    assert!(runtime.organization.pending.is_none());
    assert!(!runtime
        .saved
        .settings
        .contains_key(catalog_organization::KEY));
    assert!(runtime.rx.try_recv().is_err());
    app.ott = Some(runtime);
}

#[test]
fn stale_category_revision_and_replaced_catalog_cannot_mutate() {
    let _serial = plx_base::testlock::serial();
    for replacement in [false, true] {
        let mut app = fixture();
        open(&mut app);
        let old_key = app
            .ott
            .as_ref()
            .unwrap()
            .organization_view()
            .unwrap()
            .key("create");
        if replacement {
            app.ott.as_mut().unwrap().catalog_revision += 1;
        } else {
            select_category(&mut app, "News");
        }
        let mut runtime = app.ott.take().unwrap();
        assert!(handle_action(&mut runtime, &mut app, OttAction::EditSetting(old_key)).is_err());
        assert!(runtime.edit_session.is_none());
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
        app.ott = Some(runtime);
    }
}

#[test]
fn saved_source_and_overlay_aba_invalidate_old_editor_authority() {
    let _serial = plx_base::testlock::serial();
    for source_change in [false, true] {
        let mut app = fixture();
        open(&mut app);
        scoped(&mut app, "create");
        let old = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
        let change = candidate(
            &app,
            Operation::Create {
                label: "Other".into(),
            },
        );
        let mut runtime = app.ott.take().unwrap();
        let before = runtime.saved.clone();
        if source_change {
            runtime.saved.profiles.profiles[0].source =
                "https://other.fixture.invalid/list.m3u".into();
        } else {
            runtime.saved = change;
        }
        runtime.sync_saved_context();
        runtime.saved = before;
        runtime.sync_saved_context();
        assert!(handle_action(
            &mut runtime,
            &mut app,
            OttAction::SubmitSetting {
                session: old.id,
                key: old.key,
                value: "Stale".into()
            }
        )
        .is_err());
        assert!(!runtime.saving);
        assert!(!runtime
            .saved
            .settings
            .contains_key(catalog_organization::KEY));
        assert!(runtime.rx.try_recv().is_err());
        app.ott = Some(runtime);
    }
}

#[test]
fn pending_back_and_late_saved_do_not_take_navigation_back() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let (saved, _) = text_save(
        &mut app,
        "create",
        "Durable later",
        Operation::Create {
            label: "Durable later".into(),
        },
    );
    press(&mut app, Key::Back);
    assert!(!editor_visible(&app));
    scoped(&mut app, "back");
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    durable(&mut app, saved);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "categories");
    assert!(runtime.organization.session.is_none());
    assert!(runtime
        .organization_categories()
        .iter()
        .any(|row| row.title == "Durable later"));
    assert!(!editor_visible(&app));
}

#[test]
fn wrong_screen_owner_rejects_scoped_actions_before_editor_or_writer() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let key = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .key("create");
    let mut runtime = app.ott.take().unwrap();
    let owner = runtime.reply.unwrap();
    runtime.reply = Some(InstanceId(owner.0.wrapping_add(1)));
    assert!(handle_action(&mut runtime, &mut app, OttAction::EditSetting(key)).is_err());
    assert!(runtime.edit_session.is_none());
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    app.ott = Some(runtime);
}

#[test]
fn favorites_selection_changed_after_open_is_preserved_and_cannot_be_organized() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let key = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .key("create");
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.favorites.toggle(favorite("Gamma")).unwrap();
    runtime.saved.favorites_selected = true;
    let favorites = runtime.saved.favorites.clone();
    let mut runtime = app.ott.take().unwrap();
    assert!(handle_action(&mut runtime, &mut app, OttAction::EditSetting(key)).is_err());
    assert!(runtime.saved.favorites_selected);
    assert_eq!(runtime.saved.favorites, favorites);
    assert!(!runtime.organization_available());
    assert!(!runtime.saving);
    app.ott = Some(runtime);
    let ids = remote_ids(&mut app, 11);
    assert_eq!(
        ids,
        favorites
            .entries()
            .iter()
            .map(|f| format!("favorite:{}", f.identity()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ordinary_provider_kinds_allow_organization_but_kiosk_and_disabled_do_not() {
    let _serial = plx_base::testlock::serial();
    for kind in [
        ProviderKind::M3u,
        ProviderKind::Xtream,
        ProviderKind::Stalker,
    ] {
        let mut app = fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            runtime.provider = kind;
            let profile = &mut runtime.saved.profiles.profiles[0];
            profile.kind = kind;
            match kind {
                ProviderKind::Xtream => profile.source = r#"{"server":"https://fixture.invalid","username":"fixture","password":"fixture"}"#.into(),
                ProviderKind::Stalker => profile.source = r#"{"portal":"https://fixture.invalid","mac":"00:1A:79:00:00:01"}"#.into(),
                _ => {},
            }
            for entry in &mut runtime.catalog.entries {
                entry.media = match kind {
                    ProviderKind::Xtream => MediaLocator::Xtream {
                        stream_id: "1".into(),
                        stream_kind: "live".into(),
                        extension: "m3u8".into(),
                        series_id: None,
                        has_direct_source: false,
                    },
                    ProviderKind::Stalker => MediaLocator::Stalker {
                        cmd: "ffmpeg http://fixture.invalid/1".into(),
                        stream_kind: "itv".into(),
                        series: String::new(),
                    },
                    _ => entry.media.clone(),
                };
            }
            runtime.sync_saved_context();
        }
        open(&mut app);
        assert!(app
            .ott
            .as_ref()
            .unwrap()
            .organization_view()
            .unwrap()
            .categories
            .iter()
            .any(|row| row.title == "News"));
        let runtime = app.ott.as_mut().unwrap();
        let pin =
            crate::app::ott_kiosk_stored::Stored::armed(favorite("Alpha").origin, false).unwrap();
        runtime.saved.settings.insert(
            crate::app::ott_kiosk_stored::KEY.into(),
            pin.encode().unwrap(),
        );
        assert!(!runtime.organization_available());
        runtime
            .saved
            .settings
            .remove(crate::app::ott_kiosk_stored::KEY);
        runtime.saved.profiles.profiles[0].enabled = false;
        assert!(!runtime.organization_available());
    }
}

#[test]
fn edited_all_uses_raw_add_picker_and_matches_remote_catalog_receipt() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott.as_mut().unwrap().saved.category.clear();
    open(&mut app);
    let all = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .categories
        .iter()
        .position(|row| row.id == catalog_organization::ALL_ID)
        .unwrap();
    scoped(&mut app, &format!("category:{all}"));
    let alpha = item_id(&app, "Alpha");
    let beta = item_id(&app, "Beta");
    let gamma = item_id(&app, "Gamma");
    scoped(&mut app, "item:0");
    mutate(
        &mut app,
        "member.remove",
        Operation::RemoveMember {
            category: catalog_organization::ALL_ID.into(),
            item: alpha.clone(),
        },
    );
    assert_eq!(
        app.ott.as_ref().unwrap().ordered_ids(),
        [beta.clone(), gamma.clone()]
    );
    let runtime_id = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let categories = remote(
        &mut app,
        21,
        "categories",
        json!({
            "op":"list", "runtime":runtime_id, "offset":0,"limit":100,
        }),
    );
    assert_eq!(categories["status"], "ok");
    let channels = remote(
        &mut app,
        22,
        "channels",
        json!({
            "runtime":runtime_id,"catalog":categories["data"]["catalog"],
            "category":"all","offset":0,"limit":100,
        }),
    );
    assert_eq!(channels["status"], "ok");
    let ids: Vec<_> = channels["data"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, [beta, gamma]);
    scoped(&mut app, "member.add");
    let index = app
        .ott
        .as_ref()
        .unwrap()
        .organization_view()
        .unwrap()
        .items
        .iter()
        .position(|row| row.id == alpha)
        .expect("removed All member remains in the raw Add picker");
    mutate(
        &mut app,
        &format!("add:{index}"),
        Operation::AddMember {
            category: catalog_organization::ALL_ID.into(),
            item: alpha.clone(),
        },
    );
    assert!(app.ott.as_ref().unwrap().ordered_ids().contains(&alpha));
    assert_eq!(app.ott.as_ref().unwrap().catalog, raw_catalog());
}

#[test]
fn wrong_session_or_key_cannot_save_or_acknowledge_the_live_category_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    scoped(&mut app, "create");
    let editor = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    for (session, key) in [
        (0, editor.key.clone()),
        (editor.id, "organization.invalid.rename".into()),
    ] {
        let mut runtime = app.ott.take().unwrap();
        assert!(handle_action(
            &mut runtime,
            &mut app,
            OttAction::SubmitSetting {
                session,
                key,
                value: "Wrong target".into(),
            }
        )
        .is_err());
        assert!(runtime.edit_session.as_ref() == Some(&editor));
        assert!(runtime.pending_editor_save.is_none());
        assert!(runtime.organization.pending.is_none());
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
        app.ott = Some(runtime);
        settle(&mut app);
        assert!(editor_visible(&app));
    }
}

#[test]
fn departed_screen_keeps_its_route_when_old_category_save_is_acknowledged() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    open(&mut app);
    let old_owner = app.pages.top_page().unwrap();
    let (saved, _) = text_save(
        &mut app,
        "create",
        "Saved after exit",
        Operation::Create {
            label: "Saved after exit".into(),
        },
    );
    bridge::show_page(&mut app.pages, AppArg::Home);
    settle(&mut app);
    runtime_tick(&mut app);
    settle(&mut app);
    let owner = app.pages.top_page().unwrap();
    assert_ne!(owner, old_owner);
    assert!(matches!(app.route(), AppArg::Home));
    durable(&mut app, saved);
    assert_eq!(app.pages.top_page(), Some(owner));
    assert!(matches!(app.route(), AppArg::Home));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.organization.session.is_none());
    assert!(runtime.edit_session.is_none());
    assert!(runtime
        .organization_categories()
        .iter()
        .any(|row| row.title == "Saved after exit"));
}

#[test]
fn scoped_back_is_cleanup_even_after_settings_access_becomes_locked() {
    let _serial = plx_base::testlock::serial();
    for pending in [false, true] {
        let mut app = fixture();
        open(&mut app);
        let failure = if pending {
            let (_, failure) = text_save(
                &mut app,
                "create",
                "Still pending",
                Operation::Create {
                    label: "Still pending".into(),
                },
            );
            press(&mut app, Key::Back);
            Some(failure)
        } else {
            None
        };
        {
            let runtime = app.ott.as_mut().unwrap();
            // Model access becoming unavailable after admission, without
            // mutating the private parental Gate or executing PIN derivation.
            runtime.saved.settings.insert("parentPin".into(), json!(
                "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"
            ));
            runtime.saved.settings.insert("psOptions".into(), json!(1));
            assert!(parental::Policy::read(&runtime.saved.settings).is_ok());
            runtime.retire_parental_source();
            assert!(runtime
                .require_parental(parental::Target::Settings)
                .is_err());
        }
        scoped(&mut app, "back");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "categories");
        assert!(runtime.organization.session.is_none());
        assert!(
            runtime.edit_session.is_none(),
            "cleanup must not launch a PIN editor"
        );
        assert!(!editor_visible(&app));
        assert_eq!(
            runtime.saving, pending,
            "Back cannot cancel an admitted writer"
        );
        if let Some(failure) = failure {
            deliver(&mut app, failure);
            assert_eq!(app.ott.as_ref().unwrap().page, "categories");
        }
    }
}

#[test]
fn stale_or_wrong_owner_scoped_back_cannot_close_current_organization() {
    let _serial = plx_base::testlock::serial();
    for wrong_owner in [false, true] {
        let mut app = fixture();
        open(&mut app);
        let old_key = app
            .ott
            .as_ref()
            .unwrap()
            .organization_view()
            .unwrap()
            .key("back");
        if !wrong_owner {
            select_category(&mut app, "News");
        }
        let mut runtime = app.ott.take().unwrap();
        if wrong_owner {
            runtime.reply = Some(InstanceId(runtime.reply.unwrap().0.wrapping_add(1)));
        }
        let revision = runtime.organization.session.as_ref().unwrap().revision;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        assert!(handle_action(&mut runtime, &mut app, OttAction::EditSetting(old_key)).is_err());
        assert_eq!(runtime.page, "organization");
        assert_eq!(
            runtime.organization.session.as_ref().unwrap().revision,
            revision
        );
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.edit_session.is_none());
        assert!(!runtime.saving);
        app.ott = Some(runtime);
    }
}

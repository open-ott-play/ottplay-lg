//! Plex setup crosses the ordinary OTT action, save and authentication boundaries.
//! Registry clients are synthetic and data IO is disabled. Save tests admit the
//! real writer with a revoked permit (no private IO), consume its refusal, then
//! inject a documented completion-model result rather than claim persistence.
use super::*;
use crate::app::ott_plex_setup::{self, PlexSetupPlan, PlexSetupServer};

struct Registry;
impl Registry {
    fn empty() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn register(machine: &str) -> PlexSetupServer {
    let origin = plx_plex::plex::Origin::parse("https://plex-setup.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        machine,
        &origin,
        "synthetic-token",
        None,
        "synthetic-client",
        Default::default(),
    );
    let client = plx_plex::plex::client_for(sid).unwrap();
    client.disable_data_io();
    assert!(plx_plex::plex::grant::credential_allowed_for(
        machine,
        client.origin()
    ));
    PlexSetupServer {
        machine_id: machine.into(),
        name: plx_plex::plex::server_facts(sid)
            .map(|facts| facts.name.clone())
            .unwrap_or_default(),
        id_hint: sid.raw(),
    }
}

fn plex_profile(id: &str, machine: &str, enabled: bool) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: format!("Profile {id}"),
        kind: ProviderKind::Plex,
        source: machine.into(),
        source_alias: "Preserved alias".into(),
        enabled,
    }
}

fn mounted() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    let mut active = plex_profile("tv", "https://media.invalid/list.m3u", true);
    active.kind = ProviderKind::M3u;
    runtime.saved.profiles.profiles.push(active);
    runtime.saved.profiles.active_profile = Some("tv".into());
    runtime
        .saved
        .settings
        .insert("selectedProvider".into(), json!("m3u"));
    runtime.selected = Some("currently-playing-tv".into());
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
}

fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(
        MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })),
    );
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn refused_write(app: &mut App) -> (Option<Invocation>, bool, Value) {
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.saving, "real save must have been admitted");
    match runtime
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("revoked writer reply")
    {
        Job::Saved(Err("Could not prepare protected OTT settings for saving"), request, reload, response) => {
            assert!(!reload, "Plex recovery must not request a catalogue reload");
            assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
            (request, reload, response)
        }
        _ => panic!("expected a pre-IO refusal"),
    }
}

fn expected_saved(app: &App, servers: &[PlexSetupServer]) -> (Saved, String, usize) {
    let runtime = app.ott.as_ref().unwrap();
    let PlexSetupPlan::Profiles {
        updated,
        first_profile_id,
        omitted_servers,
        ..
    } = ott_plex_setup::plan(&runtime.saved.profiles, servers).unwrap()
    else {
        panic!("synthetic eligible servers must be represented")
    };
    let mut saved = runtime.saved.clone();
    saved.profiles = updated;
    (saved, first_profile_id, omitted_servers)
}

fn complete(
    app: &mut App,
    result: Result<Saved, &'static str>,
    pending: (Option<Invocation>, bool, Value),
) {
    let (request, reload, response) = pending;
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(result, request, reload, response))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn plex_no_eligible_server_runs_account_sign_in_instead_of_just_a_login_route() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    for disabled in [false, true] {
        let mut app = mounted();
        app.ott
            .as_mut()
            .unwrap()
            .saved
            .profiles
            .profiles
            .push(plex_profile("stale", "absent-machine", !disabled));
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let epoch = app.bridge.auth_read().0.flow_epoch;
        ui(&mut app, OttAction::SelectProvider("plex".into()));
        assert!(
            matches!(app.route(), AppArg::Ott),
            "sign-in is still a queued loop request"
        );
        run::loop_requests(&mut app);
        settle_pages(&mut app);
        auth_return::poll(&mut app);
        settle_pages(&mut app);
        assert!(matches!(app.route(), AppArg::Login));
        assert!(
            app.bridge.auth_read().0.flow_epoch > epoch,
            "the addressed OTT handler must execute the Session sign-in command"
        );
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
}

#[test]
fn plex_usable_profile_browses_without_reenabling_others_and_always_offers_connect() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    register("machine-a");
    let mut app = mounted();
    app.ott.as_mut().unwrap().saved.profiles.profiles.extend([
        plex_profile("available", "machine-a", true),
        plex_profile("deliberately-disabled", "machine-a", false),
    ]);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    ui(&mut app, OttAction::SelectProvider("plex".into()));
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.view();
    assert_eq!(view.page_id, "profiles:plex");
    assert_eq!(view.selected_row.as_deref(), Some("available"));
    let connect = view
        .rows
        .iter()
        .find(|row| row.id == "action/plex.connect")
        .unwrap();
    assert!(connect.enabled);
    assert!(matches!(&connect.action, OttAction::EditSetting(key) if key == "plex.connect"));
    assert!(!runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.selected.as_deref(), Some("currently-playing-tv"));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(
        app.bridge.take_reqs().is_empty(),
        "ordinary browse must not start authentication"
    );
}

#[test]
fn plex_recovery_saved_keeps_active_tv_and_focuses_represented_profile_without_autoplay() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let servers = vec![register("machine-a")];
    let mut app = mounted();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .profiles
        .profiles
        .push(plex_profile("kept-id", "machine-a", false));
    let (saved, first, _) = expected_saved(&app, &servers);
    let old_profiles = app.ott.as_ref().unwrap().saved.profiles.clone();
    ui(&mut app, OttAction::SelectProvider("plex".into()));
    let pending = refused_write(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles, old_profiles);
    complete(&mut app, Ok(saved), pending);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        (runtime.page, runtime.provider),
        ("profiles", ProviderKind::Plex)
    );
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("tv"));
    assert_eq!(runtime.saved.settings["selectedProvider"], json!("m3u"));
    assert_eq!(runtime.view().selected_row.as_deref(), Some(first.as_str()));
    assert_eq!(runtime.selected.as_deref(), Some("currently-playing-tv"));
    assert!(
        runtime.catalog_next.is_none()
            && runtime.play_target.is_none()
            && runtime.resolver_next.is_none()
    );
    let before = serde_json::to_value(&runtime.saved).unwrap();
    ui(&mut app, OttAction::EditSetting("plex.connect".into()));
    let runtime = app.ott.as_ref().unwrap();
    assert!(
        !runtime.saving,
        "an identical explicit connection must not write again"
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn plex_recovery_completion_cannot_reopen_profiles_after_newer_navigation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let servers = vec![register("machine-a")];
    for action in [OttAction::Back, OttAction::SelectProvider("m3u".into())] {
        let mut app = mounted();
        let (saved, _, _) = expected_saved(&app, &servers);
        ui(&mut app, OttAction::SelectProvider("plex".into()));
        let pending = refused_write(&mut app);
        ui(&mut app, action);
        let browse = (
            app.ott.as_ref().unwrap().page,
            app.ott.as_ref().unwrap().provider,
        );
        complete(&mut app, Ok(saved), pending);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!((runtime.page, runtime.provider), browse);
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("tv"));
        assert!(
            runtime.catalog_next.is_none()
                && runtime.play_target.is_none()
                && runtime.resolver_next.is_none()
        );
    }
}

#[test]
fn plex_recovery_refusal_retains_profiles_and_offers_retry_without_invalid_focus() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    register("machine-a");
    let mut app = mounted();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    ui(&mut app, OttAction::SelectProvider("plex".into()));
    let pending = refused_write(&mut app);
    complete(&mut app, Err("Synthetic save refused"), pending);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    let view = runtime.view();
    assert_eq!(view.selected_row.as_deref(), Some("action/plex.connect"));
    assert!(view
        .rows
        .iter()
        .any(|row| row.id == "action/plex.connect" && row.enabled));
    assert!(view.status.contains("Synthetic save refused"));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn plex_capacity_notice_survives_saved_alongside_errors_and_clears_on_provider_switch() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let servers: Vec<_> = (0..16).map(|n| register(&format!("machine-{n}"))).collect();
    let mut app = mounted();
    let (saved, _, omitted) = expected_saved(&app, &servers);
    assert!(omitted > 0);
    ui(&mut app, OttAction::SelectProvider("plex".into()));
    let pending = refused_write(&mut app);
    complete(&mut app, Ok(saved), pending);
    let notice = format!("{omitted} additional Plex servers exceed the profile limit");
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.view().status.contains(&notice));
    runtime.status = "Synthetic later error".into();
    let status = runtime.view().status;
    assert!(status.contains(&notice) && status.contains("Synthetic later error"));
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    assert_eq!(app.ott.as_ref().unwrap().plex_omitted_servers, 0);
    assert!(!app.ott.as_ref().unwrap().view().status.contains(&notice));
}

#[path = "plex_origin_return.rs"]
mod origin_return;

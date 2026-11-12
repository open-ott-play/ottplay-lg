//! Actual deferred-release ingress; no decoder/network worker is launched.
use super::*;
use plx_media::{player, route};

struct DeferredOwner(Box<App>);
impl Drop for DeferredOwner {
    fn drop(&mut self) {
        player::SHARED.duration_ns.store(0, Ordering::Release);
        player::stop_bufferfeed(&mut self.0.player.session, &mut self.0.adapters.player);
        player::TX.reset();
        route::reset_player_control_for_test(&self.0.player.session);
    }
}
fn info() -> plx_media::ff::inspect::MediaInfo {
    plx_media::ff::inspect::MediaInfo { tracks:Default::default(), resolved_source:None,
        hls_playback:None, video_codec:"h264".into(), audio_codec:"aac".into(),
        width:1280, height:720, fps:24.0, duration_ns:60_000_000_000,
        audio_channels:2, audio_sample_rate:48_000, audio_bit_depth:16, dovi:Default::default() }
}
fn playing(profile: ProviderProfile, entry: CatalogEntry, app: &App) -> Playing {
    Playing { channel_capture:None, kiosk_selection:None, archive:None,
        transient_owner:None, local_selection:None,
        identity:route::playback_identity(&app.player.session), profile,
        selection_id:entry.id.clone(), entry,
        source:route::external_source(&app.player.session), advance_on_failure:false,
        requires_server_lease:false }
}
fn external(app: &mut App) {
    player::TX.reset();
    route::reset_player_control_for_test(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    classic_source::release(runtime);
    let profile = ProviderProfile { id:"native-external".into(), name:"Fixture".into(),
        kind:ProviderKind::M3u, source:"https://owned.invalid/list.m3u".into(),
        source_alias:"Fixture".into(), enabled:true };
    let mut entry = runtime.catalog.entries[0].clone();
    entry.id = "owned".into();
    entry.media = MediaLocator::Http { url:"https://owned.invalid/video.mp4".into() };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = profile.kind;
    runtime.catalog.entries = vec![entry.clone()];
    route::request_external_source(&mut app.player.session, &info(),
        plx_media::ff::ExternalSource::Http("https://owned.invalid/video.mp4".into()),
        "Fixture", "Documentary", plx_platform::devcaps::assumed_shared()).unwrap();
    let owner = playing(profile, entry, app);
    app.ott.as_mut().unwrap().playing = Some(owner);
}
fn defer(app: &mut App) {
    app.adapters.player.begin_release_for_test(std::time::Duration::ZERO);
    assert_eq!(player::start_bufferfeed_tracked(&mut app.player.session, &mut app.adapters.player),
        player::BufferfeedStartOutcome::Deferred);
    assert!(app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_external_deferred_start_revalidates_current_source_before_native_release() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, _) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(app));
    defer(app);
    crate::app::run::publish_playback_frame(app, false);
    assert!(app.adapters.player.release_has_queued_start(), "ordinary OTT source keeps its admitted deferred start");
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_external_deferred_start_refuses_metadata_source_and_owner_changes_without_load() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for change in ["metadata", "source", "cancel", "duplicate", "receipt", "quit"] {
        let (app, _, _) = foreground_home();
        let mut rig = DeferredOwner(Box::new(app));
        let app = &mut *rig.0;
        external(app);
        defer(app);
        let runtime = app.ott.as_mut().unwrap();
        match change {
            "metadata" => runtime.catalog.entries[0].protection = Some(true),
            "source" => runtime.saved.profiles.profiles[0].source = "https://changed.invalid/list.m3u".into(),
            "cancel" => runtime.playing = None,
            "quit" => app.running = false,
            "duplicate" => runtime.catalog.entries.push(runtime.catalog.entries[0].clone()),
            "receipt" => runtime.playing.as_mut().unwrap().source = Some(
                plx_media::ff::ExternalSource::Http("https://changed.invalid/video.mp4".into())),
            _ => unreachable!(),
        }
        crate::app::run::publish_playback_frame(app, false);
        assert!(!app.adapters.player.release_has_queued_start(), "{change} must cancel the queued native Load");
        assert!(!app.adapters.player.is_live());
    }
}

#[test]
fn owned_external_deferred_start_observes_policy_and_pin_expiry_after_original_admission() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, mut now) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    app.ott.as_mut().unwrap().catalog.entries[0].protection = Some(true);
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().entry.protection = Some(true);
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.playing.as_ref().unwrap().profile.clone();
    parental_runtime::request_foreign_pin(&mut runtime, app, &profile);
    app.ott = Some(runtime);
    real_settings_owner(app, &mut now);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    parental_action(app, OttAction::SubmitSetting { session:editor.id,
        key:editor.key, value:"1234".into() }).unwrap();
    let job = app.ott.as_ref().unwrap().rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    let Job::Parental(completion) = job else { panic!("expected addressed PIN verification"); };
    let mut runtime = app.ott.take().unwrap();
    parental_runtime::completed(&mut runtime, app, completion);
    app.ott = Some(runtime);
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(app));
    defer(app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.clock_sample = Some(runtime.clock_sample.unwrap()
        .advance_for_test(std::time::Duration::from_secs(3601)));
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_plex_final_start_uses_its_saved_profile_without_a_classic_fallback() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid, _) = foreground_home();
    let runtime = app.ott.as_mut().unwrap();
    classic_source::release(runtime);
    let profile = ProviderProfile { id:"owned-plex".into(), name:"Fixture".into(),
        kind:ProviderKind::Plex, source:"foreign-parental".into(),
        source_alias:"Fixture".into(), enabled:true };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.settings.insert("nativePlexSources".into(), json!([{
        "machine_id":"foreign-parental", "name":"Fixture", "address":"foreign-parental.invalid",
        "port":443, "origin":"https://foreign-parental.invalid:443", "token":"synthetic-token"
    }]));
    runtime.provider = profile.kind;
    let entry = runtime.catalog.entries[0].clone();
    route::install_plex_playback_for_test(&mut app.player.session, sid, "item");
    route::install_plex_request_for_test(&mut app.player.session, sid, "item");
    let owner = playing(profile, entry, &app);
    app.ott.as_mut().unwrap().playing = Some(owner);
    assert!(classic_source::profile(app.ott.as_ref().unwrap()).is_none());
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    assert!(classic_source::profile(app.ott.as_ref().unwrap()).is_none(), "owned admission cannot stage a foreign catalogue");
    app.ott.as_mut().unwrap().catalog.entries[0].protection = Some(true);
    assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
}

#[test]
fn owned_mixed_favorite_final_start_keeps_the_bound_target_and_rejects_retired_favorite() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, _) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    let runtime = app.ott.as_mut().unwrap();
    let target = runtime.saved.profiles.profiles[0].clone();
    let mut active = target.clone();
    active.id = "other-source".into(); active.name = "Other".into();
    active.source = "https://other.invalid/list.m3u".into(); active.source_alias = "Other".into();
    runtime.saved.profiles.profiles.push(active.clone());
    runtime.saved.profiles.active_profile = Some(active.id);
    let entry = runtime.playing.as_ref().unwrap().entry.clone();
    let favorite = Favorite { title:entry.title.clone(),
        origin:plx_data::ott::favorites::Origin { provider:"m3u".into(), profile:target.name.clone(),
            subprofile:None, source_alias:target.source_alias.clone() },
        locator:plx_data::ott::favorites::Locator::ChannelName {
            name:entry.title, group:Some(entry.category),
        },
    };
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    runtime.playing.as_mut().unwrap().selection_id = format!("favorite:{}", favorite.identity());
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(app),
        "the actual Favorite source stays authoritative while a different provider is active");
    defer(app);
    app.ott.as_mut().unwrap().saved.favorites.remove(&favorite.identity());
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_external_deferred_start_rechecks_a_newly_enabled_channel_policy() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, _) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("psChannels".into(), json!(0));
    runtime.catalog.entries[0].protection = Some(true);
    runtime.playing.as_mut().unwrap().entry.protection = Some(true);
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(app));
    defer(app);
    app.ott.as_mut().unwrap().saved.settings.insert("psChannels".into(), json!(1));
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_deferred_denial_releases_its_lease_once_without_canceling_a_newer_selection() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, _) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    let (lease, released, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let runtime = app.ott.as_mut().unwrap();
    runtime.active_lease = Some(HeldLease::Playback(lease));
    runtime.playing.as_mut().unwrap().requires_server_lease = true;
    defer(app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries[0].protection = Some(true);
    runtime.play_target = Some(PlaybackRequest::ordinary("newer-selection", 0));
    let generation = runtime.play_generation;
    let ticket = route::play_request_ticket();
    crate::app::run::finish_landed_play_for_test(app);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
    released.recv_timeout(std::time::Duration::from_secs(1)).expect("old playback lease begins release");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.active_lease.as_ref().is_some_and(|lease| !lease.is_playback()));
    assert!(runtime.playing.is_none());
    assert_eq!(runtime.play_target.as_ref().map(|target| target.id.as_str()), Some("newer-selection"));
    assert_eq!(runtime.play_generation, generation);
    assert!(route::play_request_is_current(ticket));
    crate::app::run::publish_playback_frame(app, false);
    assert!(released.try_recv().is_err(), "the retired owner cannot release or cancel again");
}

#[test]
fn owned_deferred_start_refuses_a_required_lease_that_has_already_retired() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (app, _, _) = foreground_home();
    let mut rig = DeferredOwner(Box::new(app));
    let app = &mut *rig.0;
    external(app);
    defer(app);
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().requires_server_lease = true;
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn owned_local_deferred_start_keeps_its_real_root_grant_and_denies_revocation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    use crate::app::ott_runtime::tests::local_integration as local_fixture;
    let (_directory, app, root) = local_fixture::setup();
    let mut rig = DeferredOwner(app);
    let app = &mut *rig.0;
    app.running = true;
    std::fs::write(std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4"), b"fixture").unwrap();
    local_fixture::install(app, Some(&root), "first");
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(app), "the admitted Local root is not a legacy Plex fallback");
    defer(app);
    crate::app::run::publish_playback_frame(app, false);
    assert!(app.adapters.player.release_has_queued_start());
    app.ott.as_mut().unwrap().local_inventory.install(Vec::new(), 8);
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn owned_plex_final_start_rechecks_configured_transient_profile_and_origin_even_with_pin_disabled() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    use crate::app::ott_plex_queue as queue;
    for change in ["profile", "origin", "token"] {
        let (app, sid, _) = foreground_home();
        let mut rig = DeferredOwner(Box::new(app));
        let app = &mut *rig.0;
        external(app);
        let profile = ProviderProfile { id:"configured-queue".into(), name:"Fixture".into(),
            kind:ProviderKind::Plex, source:"foreign-parental".into(),
            source_alias:"Fixture".into(), enabled:true };
        let authority = Arc::new(crate::app::ott_plex_prepare::CapturedAuthority::capture(
            plx_plex::plex::client_for(sid).unwrap()).unwrap());
        let prepared = queue::prepare(&[queue::DecimalId::parse("1").unwrap()],
            &[queue::PlexRecord { id:"1".into(), title:"Fixture".into(),
                kind:queue::RecordKind::Movie, playable:true }]).unwrap();
        let owner = OwnerToken(77);
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.settings.insert("parentPin".into(), json!("*"));
        runtime.saved.settings.insert("nativePlexSources".into(), json!([{
            "machine_id":"foreign-parental", "name":"Fixture", "address":"foreign-parental.invalid",
            "port":443, "origin":"https://foreign-parental.invalid:443", "token":"synthetic-token"
        }]));
        runtime.provider = profile.kind;
        runtime.transient.accepted = Some(transient::TransientMedia::Plex(transient::plex_remote::PlexSession {
            queue:queue::PlexQueue::new(queue::OwnerToken(owner.0), prepared), authority,
            metadata:Arc::new(Vec::new()), occurrences:vec![0], profile:profile.clone(),
            category:String::new(), favorites_selected:false, borrowed:None, receipt:None,
        }));
        let entry = &mut runtime.catalog.entries[0];
        entry.id = "plex-occurrence:77:0".into();
        entry.media = MediaLocator::Plex { server_id:profile.source.clone(), rating_key:"1".into() };
        let entry = entry.clone();
        let playing = runtime.playing.as_mut().unwrap();
        playing.profile = profile;
        playing.entry = entry.clone();
        playing.selection_id = entry.id;
        playing.transient_owner = Some(owner);
        assert!(crate::app::ott_runtime::foreign_native_start_allowed(app), "unmodified configured Plex session is valid");
        defer(app);
        let runtime = app.ott.as_mut().unwrap();
        match change {
            "profile" => runtime.saved.profiles.profiles[0].enabled = false,
            "origin" => runtime.saved.settings.get_mut("nativePlexSources").unwrap()[0]["origin"] = json!("https://changed.invalid:443"),
            "token" => runtime.saved.settings.get_mut("nativePlexSources").unwrap()[0]["token"] = json!("changed-token"),
            _ => unreachable!(),
        }
        crate::app::run::publish_playback_frame(app, false);
        assert!(!app.adapters.player.release_has_queued_start(), "{change} cannot borrow retained queue credentials");
        assert!(!app.adapters.player.is_live());
    }
}

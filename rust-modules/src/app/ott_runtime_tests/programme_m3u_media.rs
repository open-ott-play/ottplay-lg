//! Private media-link editor with real screen submission and injected Saved outcomes.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};
use crate::app::ott_m3u_attachment::Registry;
const LINK: &str = "portal::[key:fixture-private]https://portal.fixture.invalid/api/";
const NEXT: &str = "portal::[key:next-private]https://next.fixture.invalid/api/";

fn parent(id: &str) -> ProviderProfile {
    ProviderProfile { id: id.into(), name: format!("TV {id}"), kind: ProviderKind::M3u,
        source: format!("https://{id}.fixture.invalid/list.m3u"), source_alias: String::new(), enabled: true }
}
fn mounted(mode: u8) -> App {
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = false;
    runtime.entropy = Some([5; 16]);
    runtime.saved.profiles.profiles.extend([parent("tv-a"), parent("tv-b")]);
    runtime.saved.profiles.active_profile = Some("tv-a".into());
    runtime.saved.settings.insert("interfaceMode".into(), json!(mode));
    runtime.provider = ProviderKind::M3u;
    runtime.page = "profiles";
    runtime.profile_focus = Some("tv-a".into());
    runtime.observe_m3u_attachments();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    app.bridge.apply_ui_mode(if mode == 0 { plx_machine::machine::UiMode::Classic }
        else { plx_machine::machine::UiMode::Modern });
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    action(&mut app, OttAction::ShowProfiles);
    app
}
fn checked(app: &mut App, act: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, act);
    app.ott = Some(runtime);
    settle_pages(app);
    result
}
fn action(app: &mut App, act: OttAction) {
    let reply = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Ott(OttOut::Action { reply, action: act })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}
fn offer(app: &App, id: &str) -> OttAction {
    app.ott.as_ref().unwrap().view().rows.into_iter().find_map(|row| match &row.action {
        OttAction::EditM3uMediaLink { profile, .. } if profile == id => Some(row.action), _ => None,
    }).expect("selector must expose the captured M3U parent")
}
fn open(app: &mut App) -> EditorSession {
    if app.ott.as_ref().unwrap().page != "m3u-media-links" {
        action(app, OttAction::ShowM3uMediaLinks);
    }
    let act = offer(app, "tv-b");
    action(app, act);
    app.ott.as_ref().unwrap().edit_session.clone().unwrap()
}
fn stage(app: &mut App, session: &EditorSession, value: &str) -> OttAction {
    let tick = next_tick(app);
    let edits = [TextEdit::Clear, TextEdit::Commit(value.into())].into_iter()
        .map(|edit| InputEvent { at: tick, source: Source::Script, kind: InputKind::Text(edit) }).collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, edits);
    settle_pages(app);
    for key in [Key::Down, Key::Ok] {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
        settle_pages(app);
    }
    submitted(app, session, value)
}
fn submitted(app: &mut App, session: &EditorSession, value: &str) -> OttAction {
    let actions: Vec<_> = app.bridge.take_ott_reqs().into_iter().filter_map(|out| match out {
        OttOut::Action { action: a @ OttAction::SubmitSetting { .. }, .. } => Some(a), _ => None,
    }).collect();
    assert_eq!(actions, vec![OttAction::SubmitSetting { session: session.id,
        key: session.key.clone(), value: value.into() }]);
    actions.into_iter().next().unwrap()
}
fn pending(app: &mut App, session: &EditorSession, value: &str) -> Saved {
    let act = stage(app, session, value);
    arm(app, session, value, act)
}
fn arm(app: &mut App, session: &EditorSession, value: &str, act: OttAction) -> Saved {
    let runtime = app.ott.as_mut().unwrap();
    let candidate = runtime.m3u_media_candidate(value).unwrap();
    runtime.saving = true;
    runtime.pending_editor_save = Some(session.clone());
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.save_ui_owner = Some(session.owner);
    // Admission is modeled; this real duplicate must not release Pending or start IO.
    checked(app, act).unwrap();
    candidate
}
fn deliver(app: &mut App, image: Result<Saved, &'static str>) {
    app.ott.as_ref().unwrap().tx.send(Job::Saved(image, None, false, json!({}))).ok().unwrap();
    runtime_tick(app);
    settle_pages(app);
}
fn publish_image(app: &mut App, image: Saved) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved = image;
    runtime.observe_m3u_attachments();
}

#[test]
fn media_link_selector_and_cancel_preserve_parent_selection_and_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [0, 1] {
        let mut app = mounted(mode);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let identity = route::playback_identity(&app.player.session);
        assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|r|
            r.enabled && r.action == OttAction::ShowM3uMediaLinks));
        let session = open(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        let view = runtime.view();
        assert_eq!(view.page_id, "m3u-media-links");
        assert_eq!(view.rows.iter().filter(|r| matches!(r.action, OttAction::EditM3uMediaLink { .. })).count(), 2);
        assert!(view.rows.iter().all(|r| !r.description.contains("portal::") && !r.title.contains("portal::")));
        assert_eq!(session.key, "profile.media");
        action(&mut app, OttAction::CancelSetting { session: session.id });
        assert!(app.ott.as_ref().unwrap().m3u_media_editor.is_none());
        action(&mut app, OttAction::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
        assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
    }
}

#[test]
fn media_link_failed_save_retains_draft_and_retry_commits_only_captured_parent() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let session = open(&mut app);
    let first = pending(&mut app, &session, LINK);
    deliver(&mut app, Err("Fixture refused"));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.as_ref() == Some(&session));
    assert!(runtime.m3u_media_editor.is_some());
    assert!(!runtime.saving && runtime.pending_editor_save.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    // Failure leaves Save focused. Retry without any text event proves the screen retained LINK.
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Ok, tick));
    settle_pages(&mut app);
    let retry_action = submitted(&mut app, &session, LINK);
    let retry = arm(&mut app, &session, LINK, retry_action);
    assert!(Registry::read(&first.settings, &first.profiles).unwrap().for_parent("tv-b").is_some());
    deliver(&mut app, Ok(retry));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.is_none() && runtime.m3u_media_editor.is_none());
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("tv-a"));
    assert_eq!(runtime.saved.profiles.profiles.iter().find(|p| p.id == "tv-a"), Some(&parent("tv-a")));
    let registry = Registry::read(&runtime.saved.settings, &runtime.saved.profiles).unwrap();
    assert!(registry.for_parent("tv-a").is_none());
    assert_eq!(registry.for_parent("tv-b").unwrap().project(&parent("tv-b")).unwrap().source, LINK);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
}

#[test]
fn media_link_stale_rows_and_attachment_aba_cannot_replace_another_editor() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mutation in ["parent", "attachment-aba", "navigation", "target"] {
        let mut app = mounted(1);
        let session = open(&mut app);
        let mut stale = offer(&app, "tv-b");
        let original = app.ott.as_ref().unwrap().saved.clone();
        match mutation {
            "parent" => {
                let mut changed = original.clone();
                changed.profiles.profiles.iter_mut().find(|p| p.id == "tv-b").unwrap().source.push('x');
                publish_image(&mut app, changed);
            }
            "attachment-aba" => {
                let changed = app.ott.as_ref().unwrap().attachment_candidate(&parent("tv-b"), LINK, Some("fixture-id")).unwrap();
                publish_image(&mut app, changed);
                publish_image(&mut app, original);
            }
            "navigation" => app.ott.as_mut().unwrap().accepted_navigation(false),
            "target" => if let OttAction::EditM3uMediaLink { profile, .. } = &mut stale { *profile = "tv-a".into(); },
            _ => unreachable!(),
        }
        assert!(checked(&mut app, stale).is_err());
        assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(&session));
        if matches!(mutation, "parent" | "attachment-aba") {
            let act = stage(&mut app, &session, NEXT);
            assert!(checked(&mut app, act).is_err());
            assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(&session));
            assert!(!app.ott.as_ref().unwrap().saving);
        }
    }
}

#[test]
fn media_link_clear_rules_identity_and_utf8_bound_use_existing_candidate() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let attached = app.ott.as_ref().unwrap().attachment_candidate(&parent("tv-b"), LINK, Some("stable-id")).unwrap();
    publish_image(&mut app, attached);
    open(&mut app);
    let changed = app.ott.as_mut().unwrap().m3u_media_candidate(NEXT).unwrap();
    assert_eq!(Registry::read(&changed.settings, &changed.profiles).unwrap().for_parent("tv-b").unwrap().id(), "stable-id");
    let cleared = app.ott.as_mut().unwrap().m3u_media_candidate("").unwrap();
    assert!(Registry::read(&cleared.settings, &cleared.profiles).unwrap().for_parent("tv-b").is_none());
    // The endpoint parser retains its separate 8 KiB bound; use accepted outer whitespace
    // to isolate this editor's raw UTF-8 byte ceiling without inventing a longer URL.
    let exact = format!("{}{LINK}", " ".repeat(16384 - LINK.len()));
    assert_eq!(exact.len(), 16384);
    assert!(app.ott.as_mut().unwrap().m3u_media_candidate(&exact).is_ok());
    assert!(app.ott.as_mut().unwrap().m3u_media_candidate(&(exact + " ")).is_err());
    let padding = 16384 - LINK.len();
    let utf8 = format!("{}{}{LINK}", "\u{00a0}".repeat(padding / 2), " ".repeat(padding % 2));
    assert_eq!(utf8.len(), 16384);
    assert!(app.ott.as_mut().unwrap().m3u_media_candidate(&utf8).is_ok());
    assert!(app.ott.as_mut().unwrap().m3u_media_candidate(&(utf8 + "\u{00a0}")).is_err());
    let current = app.ott.as_ref().unwrap().edit_session.as_ref().unwrap().id;
    action(&mut app, OttAction::CancelSetting { session: current });
    let runtime = app.ott.as_mut().unwrap();
    let old = parent("tv-b");
    let mut portal = old.clone(); portal.source = LINK.into();
    let mut image = runtime.profile_candidate(portal, Some(&old)).unwrap();
    image.profiles.active_profile = Some("tv-a".into());
    publish_image(&mut app, image);
    let session = open(&mut app);
    let act = stage(&mut app, &session, "");
    assert!(checked(&mut app, act).is_err());
    assert!(app.ott.as_ref().unwrap().m3u_media_editor.is_some());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn media_link_retired_ack_does_not_reopen_or_replace_a_new_editor() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let first = open(&mut app);
    let candidate = pending(&mut app, &first, LINK);
    action(&mut app, OttAction::CancelSetting { session: first.id });
    // A different ordinary editor can exist while the original writer is still owned.
    action(&mut app, OttAction::EditSetting("profile.name".into()));
    let next = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    assert_ne!(next.id, first.id);
    deliver(&mut app, Ok(candidate));
    assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(&next));
    assert!(app.ott.as_ref().unwrap().m3u_media_editor.is_none());
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.active_profile.as_deref(), Some("tv-a"));
}

#[test]
fn media_link_own_persisted_receipt_only_rebases_a_retained_editor() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted(1);
    let session = open(&mut app);
    let candidate = app.ott.as_mut().unwrap().m3u_media_candidate(LINK).unwrap();
    publish_image(&mut app, candidate);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.m3u_media_candidate(NEXT).is_err());
    let mut other = session.clone(); other.id += 1;
    runtime.complete_profile_editor(&other, true);
    assert!(runtime.m3u_media_candidate(NEXT).is_err());
    runtime.complete_profile_editor(&session, false);
    assert!(runtime.m3u_media_candidate(NEXT).is_err());
    // This is the existing committed-but-application-error hook, not a second writer.
    runtime.complete_profile_editor(&session, true);
    assert!(runtime.m3u_media_candidate(NEXT).is_ok());
}

#[test]
fn media_link_policy_and_uncertain_state_refuse_before_private_editor_replacement() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for guard in ["soft", "strict", "parental", "uncertain"] {
        let mut app = mounted(1);
        let session = open(&mut app);
        let offered = offer(&app, "tv-b");
        let runtime = app.ott.as_mut().unwrap();
        if guard == "uncertain" {
            runtime.persistence_uncertain = true;
        } else if guard == "parental" {
            runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
            runtime.saved.settings.insert("psOptions".into(), json!(1));
        } else {
            let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
            runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":guard=="strict","origin":origin,
                "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
            assert_eq!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Locked { strict: guard=="strict" }));
        }
        assert!(checked(&mut app, offered).is_err());
        let runtime = app.ott.as_ref().unwrap();
        if guard == "parental" {
            assert_eq!(runtime.edit_session.as_ref().unwrap().key, "parental.unlock");
        } else {
            assert!(runtime.edit_session.as_ref() == Some(&session));
        }
        assert!(!runtime.saving);
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("tv-a"));
    }
}

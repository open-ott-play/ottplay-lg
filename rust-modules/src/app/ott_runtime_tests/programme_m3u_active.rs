//! Classic M3U selection separates editing the active source from explicit Load.
use super::*;

#[test]
fn classic_active_m3u_edits_with_explicit_load_and_modern_keeps_primary_load() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [0, 1] {
        let mut app = mounted_source(mode, ProviderKind::M3u);
        for active in [Some("a"), Some("b"), None] {
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.profiles.active_profile = active.map(str::to_owned);
            let view = runtime.raw_view();
            for (slot, id) in ["a", "b"].into_iter().enumerate() {
                let primary = view.rows.iter().find(|row| row.id == id).unwrap();
                assert_eq!(primary.title, if mode == 0 { format!("{}: Saved {id}", slot + 1) } else { format!("Saved {id}") });
                if mode == 0 && active == Some(id) {
                    assert!(matches!(&primary.action, OttAction::EditProfile { target, context }
                        if target == id && context == &runtime.profile_edit_context(&source_profile(ProviderKind::M3u, id))));
                    let load = view.rows.iter().find(|row| row.id.starts_with("action/profile.load/")
                        && row.action == OttAction::SelectProfile(id.into())).unwrap();
                    assert_eq!(load.title, "Load playlist");
                    assert_eq!(load.content_title.as_deref(), Some(format!("Saved {id}").as_str()));
                    assert!(!view.rows.iter().any(|row| row.id.starts_with("action/profile.edit/")
                        && matches!(&row.action, OttAction::EditProfile { target, .. } if target == id)));
                } else {
                    assert_eq!(primary.action, OttAction::SelectProfile(id.into()));
                    assert!(view.rows.iter().any(|row| row.id.starts_with("action/profile.edit/")
                        && matches!(&row.action, OttAction::EditProfile { target, .. } if target == id)));
                }
            }
            assert_eq!(view.rows.iter().filter_map(|row| match &row.action {
                OttAction::SelectProfile(id) => Some(id.as_str()), _ => None,
            }).collect::<Vec<_>>(), vec!["a", "b"], "every configured profile retains exactly one explicit Load");
        }
    }
}

#[test]
fn classic_active_m3u_ok_opens_save_only_editor_and_cancel_preserves_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = mounted_source(0, ProviderKind::M3u);
    let runtime = app.ott.as_ref().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let generation = runtime.generation;
    let identity = route::playback_identity(&app.player.session);
    assert!(matches!(&runtime.raw_view().rows.iter().find(|row| row.id == "a").unwrap().action,
        OttAction::EditProfile { target, .. } if target == "a"));
    press(&mut app, Key::Ok);
    assert!(private_editor_visible(&app));
    let runtime = app.ott.as_mut().unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    assert!(runtime.profile_edit_save_only.as_ref() == Some(&editor));
    let original = runtime.edit_profile_original.clone().unwrap();
    assert_eq!(original, source_profile(ProviderKind::M3u, "a"));
    let mut changed = original.clone();
    changed.source = "https://changed.invalid/list.m3u".into();
    let (candidate, reload) = runtime.profile_editor_candidate(&editor, changed, Some(&original)).unwrap();
    assert!(!reload);
    assert_eq!(candidate.profiles.active_profile.as_deref(), Some("a"));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert_eq!(runtime.generation, generation);
    press(&mut app, Key::Back);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.is_none());
    assert_eq!(runtime.page, "profiles");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

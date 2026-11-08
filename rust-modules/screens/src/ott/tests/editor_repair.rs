//! Repairing an existing oversized value must not require deleting the whole
//! field. These use the real editor and provider validator, without storage IO.
use super::*;
use plx_data::ott::{ProviderKind, ProviderProfile};

fn open(screen: &mut OttScreen, key: &str, value: &str, limit: usize) {
    deliver(screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
        session: 37, kind: OttEditorKind::Setting, text_policy: OttTextPolicy::Literal,
        key: key.into(), title: "Fixture field".into(), value: value.into(),
        secret: true, limit,
    }))));
}

fn input(screen: &mut OttScreen, edit: TextEdit) -> Vec<Stamped<TestHost>> {
    deliver_recorded(screen, ScreenEvent::Input(plx_machine::machine::InputEvent {
        at: Default::default(), source: plx_machine::machine::Source::Sdl,
        kind: InputKind::Text(edit),
    }))
}

fn state(screen: &OttScreen) -> (&str, usize) {
    let draft = &screen.editor.as_ref().unwrap().draft;
    (draft.text(), draft.caret())
}

fn has_submission(effects: &[Stamped<TestHost>]) -> bool {
    effects.iter().any(|effect| matches!(&effect.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { .. }, .. }))))
}

#[test]
fn valid_stalker_mac_above_editor_cap_can_be_shortened_without_clearing() {
    let _serial = plx_base::testlock::serial();
    let mac = format!("00:00:00:00:00:01{}", " ".repeat(8192));
    let source = serde_json::json!({"portal":"https://fixture.invalid/portal", "mac":mac.clone()}).to_string();
    let profile = ProviderProfile {
        id: "stalker-fixture".into(), name: "Fixture".into(), kind: ProviderKind::Stalker,
        source, source_alias: String::new(), enabled: true,
    };
    // This is an admitted provider value, not an impossible oversized fixture.
    assert!(profile.validate().is_ok());
    assert!(profile.source.len() < 16384);
    assert!(mac.len() > 8192 && mac.len() < 16384);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    open(&mut screen, "profile.setup.mac", &mac, 16384);
    assert!(!has_submission(&input(&mut screen, TextEdit::Left)));
    assert_eq!(state(&screen).1, mac.len() - 1);
    input(&mut screen, TextEdit::Right);
    assert_eq!(state(&screen).1, mac.len());
    assert!(!has_submission(&input(&mut screen, TextEdit::Backspace)));
    assert_eq!(state(&screen), (&mac[..mac.len() - 1], mac.len() - 1));
    assert!(state(&screen).0.len() > 8192);
    input(&mut screen, TextEdit::Commit("x".into()));
    assert_eq!(state(&screen), (&mac[..mac.len() - 1], mac.len() - 1),
        "repair does not raise the insertion cap");
    let expected = state(&screen).0.to_owned();
    let save = screen.keys["edit:save"];
    let sent = effects(|fx| screen.activate(save, false, fx));
    assert_eq!(sent.iter().filter(|effect| matches!(&effect.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting {
            session: 37, key, value }, .. }))
        if key == "profile.setup.mac" && value == &expected)).count(), 1);
    assert!(screen.editor.as_ref().unwrap().pending);
    let pending = state(&screen).0.to_owned();
    for edit in [TextEdit::Left, TextEdit::Right, TextEdit::Backspace, TextEdit::Clear] {
        assert!(!has_submission(&input(&mut screen, edit)));
        assert_eq!(state(&screen), (pending.as_str(), pending.len()),
            "pending save must still reject every edit");
    }
}

#[test]
fn over_declared_limit_unicode_draft_keeps_atomic_insertion_and_scalar_deletion() {
    let _serial = plx_base::testlock::serial();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    open(&mut screen, "profile.name", "映画🙂xy", 3);
    input(&mut screen, TextEdit::Left);
    assert_eq!(state(&screen).1, "映画🙂x".len());
    input(&mut screen, TextEdit::Backspace);
    assert_eq!(state(&screen), ("映画🙂y", "映画🙂".len()));
    input(&mut screen, TextEdit::Commit("追加".into()));
    assert_eq!(state(&screen), ("映画🙂y", "映画🙂".len()));
    input(&mut screen, TextEdit::Backspace);
    assert_eq!(state(&screen), ("映画y", "映画".len()));
    input(&mut screen, TextEdit::Clear);
    input(&mut screen, TextEdit::Commit("映画🙂".into()));
    assert_eq!(state(&screen), ("映画🙂", "映画🙂".len()));
    input(&mut screen, TextEdit::Commit("x".into()));
    assert_eq!(state(&screen), ("映画🙂", "映画🙂".len()),
        "the declared limit still counts Unicode scalars, not bytes");
}

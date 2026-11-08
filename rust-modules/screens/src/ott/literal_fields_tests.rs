// Normalized commits and the existing shared dispatcher only. There is no
// selection range or composition-start/end event in this input protocol.
use super::*;
use plx_machine::machine::{InputKind, Source, TextEdit};

fn open_field(kind: OttEditorKind, field: &str, policy: OttTextPolicy, secret: bool,
    limit: usize) -> (Dispatcher<TestHost>, TestRig, InstanceId) {
    let (mut d, mut rig) = boot(false);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            session: 910, kind, key: field.into(), text_policy: policy,
            title: "Fixture field".into(), value: String::new(), secret, limit,
        }))))));
    frame(&mut d, &mut rig, 256, vec![]);
    assert_focus(&d, "edit:value");
    rig.actions.clear();
    press_navigation_ok(&mut d, &mut rig, 272, false);
    assert!(d.input.keyboard);
    assert_eq!(d.input.keyboard_owner, Some(instance));
    (d, rig, instance)
}

fn edits(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32,
    edits: impl IntoIterator<Item = TextEdit>) {
    frame(d, rig, ms, edits.into_iter().map(|edit| InputEvent {
        at: tick(ms), source: Source::Sdl, kind: InputKind::Text(edit),
    }).collect());
}

fn state(d: &Dispatcher<TestHost>) -> (bool, bool, String, usize) {
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    let editor = screen.editor.as_ref().expect("fixture editor remains mounted");
    (screen.editing, editor.pending, editor.draft.text().into(), editor.draft.caret())
}

fn text_is(d: &Dispatcher<TestHost>, expected: &str) {
    let (_, _, text, caret) = state(d);
    // Do not include even synthetic credential contents in failure output.
    assert!(text == expected, "the complete synthetic draft must match");
    assert!(text.is_char_boundary(caret));
}

#[test]
fn literal_field_policy_preserves_fragmented_machine_values_independent_of_key() {
    let _serial = plx_base::testlock::serial();
    for (field, secret, chunks) in [
        ("profile.source", false, &["https://fixture.invalid/", "list.m3u"][..]),
        ("offline.bind:0", false, &["/media/", "映画/clip.mkv"][..]),
        ("setup.username", true, &["fixture-", "account"][..]),
        ("setup.password", true, &["synthetic-", "only"][..]),
        ("torrent.key", true, &["fixture-", "token"][..]),
        ("setup.mac", true, &["00:00:00:", "00:00:01"][..]),
        ("torrent.id", false, &["fixture-", "opaque-id"][..]),
        ("stremio.language:fixture", false, &["pt", "-BR"][..]),
        ("future.machine-field", false, &["fixture-", "opaque-value"][..]),
    ] {
        let (mut d, mut rig, _) = open_field(OttEditorKind::Setting, field,
            OttTextPolicy::Literal, secret, 256);
        edits(&mut d, &mut rig, 384,
            chunks.iter().map(|chunk| TextEdit::Commit((*chunk).into())));
        text_is(&d, &chunks.concat());
        assert!(state(&d).0);
        assert!(rig.actions.is_empty(), "typing does not submit the field");
    }
}

#[test]
fn literal_field_caret_and_unicode_limit_are_atomic() {
    let _serial = plx_base::testlock::serial();
    let (mut d, mut rig, _) = open_field(OttEditorKind::Setting, "profile.source",
        OttTextPolicy::Literal, false, 6);
    edits(&mut d, &mut rig, 384, [TextEdit::Commit("映".into()),
        TextEdit::Commit("画/一二".into()), TextEdit::Left, TextEdit::Backspace,
        TextEdit::Commit("語".into()), TextEdit::Right, TextEdit::Commit("🙂".into())]);
    text_is(&d, "映画/語二🙂");
    assert_eq!(state(&d).3, "映画/語二🙂".len());
    let before = state(&d);
    edits(&mut d, &mut rig, 400, [TextEdit::Commit("追加".into())]);
    assert!(state(&d) == before, "reject the entire over-limit commit, including its caret move");
    edits(&mut d, &mut rig, 416, [TextEdit::Clear, TextEdit::Backspace,
        TextEdit::Left, TextEdit::Right, TextEdit::Commit("日本".into()),
        TextEdit::Commit("語".into())]);
    text_is(&d, "日本語");
}

#[test]
fn predictive_names_and_search_keep_latin_and_cjk_word_replacement() {
    let _serial = plx_base::testlock::serial();
    for (kind, field) in [(OttEditorKind::Setting, "profile.name"),
        (OttEditorKind::Setting, "stremio.search:fixture"),
        (OttEditorKind::Search, "query")] {
        let (mut d, mut rig, _) = open_field(kind, field, OttTextPolicy::Predictive, false, 256);
        edits(&mut d, &mut rig, 384,
            ["s", "u", "m", "summer ", "holiday "].into_iter().map(|s| TextEdit::Commit(s.into())));
        text_is(&d, "summer holiday ");
        edits(&mut d, &mut rig, 400, [TextEdit::Clear]);
        edits(&mut d, &mut rig, 416,
            ["東", "東京 ", "映", "映画 "].into_iter().map(|s| TextEdit::Commit(s.into())));
        text_is(&d, "東京 映画 ");
    }
}

#[test]
fn literal_fields_keyboard_back_then_cancel_keeps_the_list_owner() {
    let _serial = plx_base::testlock::serial();
    for (field, secret) in [("profile.source", false), ("setup.password", true),
        ("torrent.key", true)] {
        let (mut d, mut rig, instance) = open_field(OttEditorKind::Setting, field,
            OttTextPolicy::Literal, secret, 256);
        edits(&mut d, &mut rig, 384,
            [TextEdit::Commit("fixture-".into()), TextEdit::Commit("only".into())]);
        frame(&mut d, &mut rig, 400,
            vec![key(Key::Back, tick(400)), key_up(Key::Back, tick(400))]);
        frame(&mut d, &mut rig, 416, vec![]);
        assert!(!d.input.keyboard);
        assert_eq!(d.input.keyboard_owner, None);
        assert_focus(&d, "edit:value");
        text_is(&d, "fixture-only");
        assert!(rig.actions.is_empty());
        frame(&mut d, &mut rig, 432,
            vec![key(Key::Back, tick(432)), key_up(Key::Back, tick(432))]);
        assert_eq!(d.nav.top_page().unwrap().inst.as_ref().unwrap().id, instance);
        assert_focus(&d, "item:video-one");
        assert_eq!(rig.actions, vec![OttAction::CancelSetting { session: 910 }]);
    }
}

#[test]
fn pending_literal_secret_ignores_more_commits_and_failed_ack_preserves_draft() {
    let _serial = plx_base::testlock::serial();
    let (mut d, mut rig, instance) = open_field(OttEditorKind::Setting, "torrent.key",
        OttTextPolicy::Literal, true, 256);
    edits(&mut d, &mut rig, 384,
        [TextEdit::Commit("fixture-".into()), TextEdit::Commit("only".into())]);
    frame(&mut d, &mut rig, 400,
        vec![key(Key::Ok, tick(400)), key_up(Key::Ok, tick(400))]);
    frame(&mut d, &mut rig, 416, vec![]);
    frame(&mut d, &mut rig, 496, vec![key(Key::Down, tick(496))]);
    assert_focus(&d, "edit:save");
    press_navigation_ok(&mut d, &mut rig, 512, false);
    assert!(state(&d).1);
    assert!(matches!(rig.actions.as_slice(), [OttAction::SubmitSetting {
        session: 910, key, value,
    }] if key == "torrent.key" && value == "fixture-only"),
        "Save must submit exactly one complete synthetic draft");
    let pending = state(&d);
    edits(&mut d, &mut rig, 624, [TextEdit::Commit("ignored".into())]);
    assert!(state(&d) == pending, "pending input cannot mutate the submitted draft");
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult {
            session: 910, error: Some("Fixture save failure".into()),
        }))));
    frame(&mut d, &mut rig, 640, vec![]);
    assert!(!state(&d).1);
    text_is(&d, "fixture-only");
    assert_eq!(rig.actions.len(), 1, "a failed acknowledgement must not retry by itself");
}

#[test]
fn literal_policy_changes_canon_but_private_draft_contents_do_not() {
    let _serial = plx_base::testlock::serial();
    let mut hashes = Vec::new();
    for (policy, value) in [(OttTextPolicy::Literal, "fixture-one"),
        (OttTextPolicy::Literal, "fixture-two"),
        (OttTextPolicy::Predictive, "fixture-one")] {
        let (mut d, mut rig, _) = open_field(OttEditorKind::Setting, "fixture.opaque",
            policy, true, 256);
        edits(&mut d, &mut rig, 384, [TextEdit::Commit(value.into())]);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
            .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        let mut canonical = Canon::new();
        screen.write(&mut canonical);
        hashes.push(canonical.finish());
    }
    assert_eq!(hashes[0], hashes[1], "private draft values must remain outside Canon");
    assert_ne!(hashes[0], hashes[2], "typed commit behavior is part of logical state");
}

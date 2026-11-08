use super::*;
use plx_platform::i18n::{language_on_this_thread_for_test, Preference};

fn open(screen: &mut OttScreen, kind: OttEditorKind, key: &str, title: &str) {
    deliver(screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
        session: 73, kind, text_policy: OttTextPolicy::Literal, key: key.into(), title: title.into(),
        value: "private draft".into(), secret: true, limit: 256,
    }))));
}

fn painted(screen: &mut OttScreen) -> String {
    let measure = plx_ui::fixture::FixtureMeasure;
    let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
    let runs = plx_ui::draw_census::capture_runs(|| {
        plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
    });
    runs.into_iter().filter(|run| run.ink[3] > 0.0).map(|run| run.text).collect::<Vec<_>>().join(" ")
}

#[test]
fn locale_refresh_relabels_setting_titles_without_replacing_editor_or_draft() {
    let _serial = plx_base::testlock::serial();
    for mode in [plx_machine::machine::UiMode::Modern, plx_machine::machine::UiMode::Classic] {
        for (key, raw) in [("profile.name", "Profile name"), ("uiFontScale", "Interface text scale (20–200%)"),
            ("profile.media", "Media library link"), ("organization.4.rename", "Rename category")] {
            let _english = language_on_this_thread_for_test(Preference::En);
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.install(snapshot(1, &["first"]));
            open(&mut screen, OttEditorKind::Setting, key, raw);
            effects(|fx| screen.edit(&TextEdit::Commit(" changed".into()), fx));
            screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
            let spec = screen.editor.as_ref().unwrap().spec.clone();
            let draft = screen.editor.as_ref().unwrap().draft.text().to_owned();
            let selected = screen.form.selected_key();
            let hash = screen.hash();
            let _spanish = language_on_this_thread_for_test(Preference::Es);
            <OttScreen as Screen<TestHost>>::locale_changed(&mut screen, 1);
            let expected = ui_text::setting_text(key, raw);
            assert_ne!(expected, raw);
            assert_eq!(screen.editor.as_ref().unwrap().title, expected);
            assert!(Arc::ptr_eq(&spec, &screen.editor.as_ref().unwrap().spec));
            assert_eq!(screen.editor.as_ref().unwrap().spec.title, raw);
            assert_eq!(screen.editor.as_ref().unwrap().spec.session, 73);
            assert_eq!(screen.editor.as_ref().unwrap().draft.text(), draft);
            assert_eq!(screen.form.selected_key(), selected);
            assert_eq!(screen.hash(), hash);
            if mode == plx_machine::machine::UiMode::Modern {
                assert_eq!(screen.view(&plx_ui::fixture::FixtureMeasure).header.title, expected);
            }
            let visible = painted(&mut screen);
            assert!(visible.contains(expected), "{mode:?}: {visible}");
            assert!(!visible.contains("private draft"));
        }
    }
}

#[test]
fn locale_refresh_reformats_captured_search_scope_and_keeps_its_session() {
    for (page, scope) in [("channels:one", "My list · Drama"), ("channels:one", ""), ("favorites:one", "")] {
        let _english = language_on_this_thread_for_test(Preference::En);
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let mut view = (*snapshot(1, &["first"])).clone();
        view.page_id = page.into();
        view.search = Some(OttSearch { query: String::new(), scope: "A newer unrelated display scope".into() });
        screen.install(Arc::new(view));
        open(&mut screen, OttEditorKind::Search, "", scope);
        let spec = screen.editor.as_ref().unwrap().spec.clone();
        let before = screen.editor.as_ref().unwrap().title.clone();
        let save = screen.keys["edit:save"];
        effects(|fx| screen.activate(save, false, fx));
        assert!(screen.editor.as_ref().unwrap().pending);
        let hash = screen.hash();
        let _spanish = language_on_this_thread_for_test(Preference::Es);
        <OttScreen as Screen<TestHost>>::locale_changed(&mut screen, 1);
        let expected = OttSearch { query: String::new(), scope: scope.into() }.title(favorites_page(page));
        assert_ne!(before, expected);
        assert_eq!(screen.editor.as_ref().unwrap().title, expected);
        assert!(Arc::ptr_eq(&spec, &screen.editor.as_ref().unwrap().spec));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "private draft");
        assert!(screen.editor.as_ref().unwrap().pending);
        assert_eq!(screen.hash(), hash);
    }
}

#[test]
fn locale_refresh_translates_failed_editor_copy_without_rewriting_raw_error() {
    let _english = language_on_this_thread_for_test(Preference::En);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    open(&mut screen, OttEditorKind::Setting, "volumeStep", "Volume step (3–10%)");
    let save = screen.keys["edit:save"];
    effects(|fx| screen.activate(save, false, fx));
    let raw = "Enter an integer from 3 to 10";
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 73, error: Some(raw.into()) }));
    let spec = screen.editor.as_ref().unwrap().spec.clone();
    let hash = screen.hash();
    let _spanish = language_on_this_thread_for_test(Preference::Es);
    <OttScreen as Screen<TestHost>>::locale_changed(&mut screen, 1);
    assert_eq!(screen.editor_copy(), msg::ott_volume_step_invalid());
    assert_ne!(screen.editor_copy(), raw);
    let editor = screen.editor.as_ref().unwrap();
    assert_eq!(editor.error, raw);
    assert_eq!(editor.draft.text(), "private draft");
    assert!(!editor.pending);
    assert!(Arc::ptr_eq(&spec, &editor.spec));
    assert_eq!(screen.hash(), hash);
    assert_eq!(ui_text::setting_text("unrecognized", "Unrecognized raw fallback"), "Unrecognized raw fallback");
}

#[test]
fn locale_refresh_preserves_exact_source_retirement_after_translation() {
    let _english = language_on_this_thread_for_test(Preference::En);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first", "second"]));
    open(&mut screen, OttEditorKind::Setting, "profile.name", "Profile name");
    let spec = screen.editor.as_ref().unwrap().spec.clone();
    let hash = screen.hash();
    let _spanish = language_on_this_thread_for_test(Preference::Es);
    <OttScreen as Screen<TestHost>>::locale_changed(&mut screen, 1);
    assert_eq!(screen.hash(), hash);
    assert!(Arc::ptr_eq(&spec, &screen.editor.as_ref().unwrap().spec));
    assert_eq!(screen.editor.as_ref().unwrap().title,
        ui_text::setting_text("profile.name", "Profile name"));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorRetired { session: 72 }));
    assert!(Arc::ptr_eq(&spec, &screen.editor.as_ref().unwrap().spec));
    let out = deliver_recorded(&mut screen,
        ScreenEvent::App(AppMsg::OttEditorRetired { session: 73 }));
    assert!(screen.editor.is_none());
    assert!(out.iter().all(|change| !matches!(&change.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { .. }, .. })))));
    assert_eq!(screen.snapshot.rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["first", "second"]);
}

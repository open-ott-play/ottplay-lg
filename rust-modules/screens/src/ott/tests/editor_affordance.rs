use super::*;

#[test]
fn editor_fields_explain_input_and_hidden_values_in_classic_and_modern() {
    let _serial = plx_base::testlock::serial();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
            for (value, secret) in [("", false), ("", true), ("fixture-account", true), ("https://fixture.invalid/list", false)] {
                let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
                deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                    session: 31, kind: OttEditorKind::Setting, key: "profile.source".into(),
                    text_policy: OttTextPolicy::Literal, title: "Source address".into(),
                    value: value.into(), secret, limit: 256,
                }))));
                screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
                let field = screen.form.table.sections.iter().flat_map(|section| &section.rows)
                    .find(|row| row.is_text_entry()).expect("the editable value is a visibly framed field");
                let hint = if secret && !value.is_empty() { msg::ott_input_hidden_hint() }
                    else { msg::ott_input_keyboard_hint() };
                assert_eq!(field.detail, hint);
                if value.is_empty() { assert_eq!(field.label, msg::ott_value_empty()); }
                else if secret { assert!(!field.label.contains(value)); }
                else { assert_eq!(field.label, value); }
                let measure = plx_ui::fixture::FixtureMeasure;
                let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
                    focus: Default::default(), press: Default::default(),
                    owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
                let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
                let runs = plx_ui::draw_census::capture_runs(||
                    plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame)));
                assert!(runs.iter().any(|run| run.text == hint && run.ink[3] > 0.0),
                    "the action hint must actually be painted in both presentations");
                if secret && !value.is_empty() {
                    assert!(runs.iter().all(|run| !run.text.contains(value)), "a hint must never reveal protected input");
                }
                let save = screen.keys["edit:save"];
                let emitted = effects(|fx| screen.activate(save, false, fx));
                assert!(emitted.iter().any(|event| matches!(&event.fx,
                    Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { value: submitted, .. }, .. }))
                    if submitted == value)), "presentation must not substitute placeholder or hidden text for the actual draft");
            }
        }
    }
}

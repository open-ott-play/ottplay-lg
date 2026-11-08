//! The real scalar editor keeps its complete narrative and every action at the
//! selected font scale. Shipped-font metrics; no decoder or persistence worker.
use super::*;
use plx_base::fontcov::advances::ShippedMeasure;
use plx_machine::machine::{FontScale, Measure, TypographyMeasure};

struct RestoreScale(FontScale);
impl Drop for RestoreScale {
    fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
}

fn scale_editor(screen: &mut OttScreen, value: &str) {
    deliver(screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal,
        session: 7, kind: OttEditorKind::Setting, key: "uiFontScale".into(),
        title: "Interface text scale (20–200%)".into(), value: value.into(),
        secret: false, limit: 3,
    }))));
}

#[test]
fn scalar_editor_complete_copy_and_controls_fit_at_every_supported_scale_and_locale() {
    let _serial = plx_base::testlock::serial();
    let _restore = RestoreScale(plx_ui::typography::current());
    let mut measure = TypographyMeasure::new(ShippedMeasure);
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for percent in [20, 100, 200] {
            plx_ui::typography::apply(percent, &mut measure).unwrap();
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            scale_editor(&mut screen, &percent.to_string());
            let view = screen.view(&measure);
            let layout = view.header.layout;
            let context = format!("{} at {}%", language.tag(), percent);
            assert_eq!(view.header.copy, msg::ott_editor_help(), "{context}");
            let title = RouteLayout::narrative_title(view.header.title).with_measure(&measure);
            let copy = plx_ui::text_view::TextView::new(view.header.copy, view.header.copy_size, theme::TEXT_READING)
                .leading(view.header.copy_size as f32 + theme::space::XS).max_lines(12).with_measure(&measure);
            let room = layout.narrative_copy_frame(true, view.header.title, layout.action.y, &measure);
            assert!(!title.truncates(layout.narrative.w), "title: {context}");
            assert!(!copy.truncates(room.w), "help: {context}");
            assert!(room.y + copy.measure_h(room.w) <= layout.action.y - theme::space::XL,
                "complete help exceeds safe narrative height: {context}");
            assert!(screen.form.table.measured_width(&measure) <= view.table.frame.w + 0.01,
                "Value/Save/Cancel were narrowed below their measured width: {context}");
            assert_eq!(measure.font_scale().percent(), percent, "layout must not lower the selected scale");
            let cx: Cx<'_, TestHost> = Cx { views: (), tick: Default::default(), measure: &measure,
                focus: Default::default(), press: Default::default(),
                owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
            for id in ["edit:value", "edit:save", "edit:cancel"] {
                let key = screen.keys[id];
                let at = Focusable::<TestHost>::place(&screen, &key, &cx, At::SpringTarget).unwrap();
                let table_at = Focusable::<TestHost>::place(&view, &key, &cx, At::SpringTarget).unwrap();
                assert_eq!(at.rect, table_at.rect, "paint/focus geometry differs: {context} {id}");
                assert!(plx_ui::consts::inside_safe(at.rect), "offscreen control: {context} {id}");
            }
        }
    }
}

#[test]
fn short_editor_at_default_scale_preserves_the_existing_column_layout() {
    let _serial = plx_base::testlock::serial();
    let _restore = RestoreScale(plx_ui::typography::current());
    let mut measure = TypographyMeasure::new(ShippedMeasure);
    plx_ui::typography::apply(100, &mut measure).unwrap();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    editor(&mut screen, "test");
    let view = screen.view(&measure);
    assert_eq!(view.header.layout.narrative, RouteLayout::screen().narrative);
    assert_eq!(view.table.frame, RouteLayout::screen().sectioned_table());
}


#[test]
fn long_address_never_squeezes_help_or_changes_the_saved_value() {
    let _serial = plx_base::testlock::serial();
    let _restore = RestoreScale(plx_ui::typography::current());
    let mut measure = TypographyMeasure::new(ShippedMeasure);
    plx_ui::typography::apply(200, &mut measure).unwrap();
    let address = format!("https://remote.example.invalid/{}", "long-path-segment/".repeat(100));
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for (key, title, secret) in [("remote.address", "Remote control server", false),
                ("profile.source", "Source address or folder", true)] {
            let open = |screen: &mut OttScreen, value: &str| {
                deliver(screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal,
                    session: 8, kind: OttEditorKind::Setting, key: key.into(),
                    title: title.into(), value: value.into(), secret, limit: 2048,
                }))));
            };
            let mut short = OttScreen::new(EntryId(1), InstanceId(1));
            open(&mut short, "https://remote.example.invalid/");
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            open(&mut screen, &address);
            let view = screen.view(&measure);
            let context = format!("{}: {title}", language.tag());
            assert_eq!(view.header.layout.narrative, short.view(&measure).header.layout.narrative,
                "a longer user value must not consume narrative width: {context}");
            if !secret {
                assert!(screen.form.table.measured_width(&measure) > view.table.frame.w,
                    "fixture must actually require user-value elision: {context}");
            }
            assert!(screen.editor_control_width(&measure) <= view.table.frame.w + 0.01,
                "app controls must fit: {context}");
            // The request title is a fallback; measure the localized title that is painted.
            let room = view.header.layout.narrative_copy_frame(true, view.header.title, view.header.layout.action.y, &measure);
            let copy = plx_ui::text_view::TextView::new(view.header.copy, view.header.copy_size, theme::TEXT_READING)
                .leading(view.header.copy_size as f32 + theme::space::XS).max_lines(12).with_measure(&measure);
            assert!(!RouteLayout::narrative_title(view.header.title).with_measure(&measure).truncates(room.w),
                "title line cap: {context}");
            assert!(!copy.truncates(room.w), "help line cap: {context}");
            assert!(room.y + copy.measure_h(room.w) <= view.header.layout.action.y - theme::space::XL,
                "help is offscreen: {context}");
            assert_eq!(screen.editor.as_ref().unwrap().draft.text(), address);
            let row = screen.form.table.sections.iter().flat_map(|section| &section.rows)
                .find(|row| row.label_origin == plx_ui::table::Origin::Server).unwrap();
            let displayed = if secret { "•".repeat(24) } else { address.clone() };
            assert_eq!(row.label, displayed, "layout must not replace the actual displayed value");
            let cx: Cx<'_, TestHost> = Cx { views: (), tick: Default::default(), measure: &measure,
                focus: Default::default(), press: Default::default(),
                owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
            for id in ["edit:value", "edit:save", "edit:cancel"] {
                let at = Focusable::<TestHost>::place(&screen, &screen.keys[id], &cx, At::SpringTarget).unwrap();
                assert!(plx_ui::consts::inside_safe(at.rect), "offscreen {id}: {context}");
            }
            let save = screen.keys["edit:save"];
            let emitted = effects(|fx| screen.activate(save, false, fx));
            assert!(emitted.iter().any(|event| matches!(&event.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { value, .. }, .. }))
                if value == &address)), "Save must submit the full address: {context}");
        }
    }
}


fn text_policy_editor(kind: OttEditorKind, key: &str, limit: usize, text_policy: OttTextPolicy) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy,
        session: 9, kind, key: key.into(), title: "Fixture field".into(),
        value: String::new(), secret: false, limit,
    }))));
    screen
}

fn policy_input(screen: &mut OttScreen, edit: TextEdit) {
    deliver(screen, ScreenEvent::Input(plx_machine::machine::InputEvent {
        at: Default::default(), source: plx_machine::machine::Source::Sdl,
        kind: InputKind::Text(edit),
    }));
}

#[test]
fn remote_address_keeps_a_35_byte_url_across_sdl_sized_commits_and_save() {
    let _serial = plx_base::testlock::serial();
    let address = "https://control.invalid/command/api";
    assert_eq!(address.len(), 35);
    let mut screen = text_policy_editor(OttEditorKind::Setting, "remote.address", 2048, OttTextPolicy::Literal);
    // These are already-decoded, distinct SDL-origin commits. The fixture does
    // not claim that a particular television splits a long input this way.
    for chunk in address.as_bytes().chunks(31) {
        policy_input(&mut screen, TextEdit::Commit(std::str::from_utf8(chunk).unwrap().into()));
    }
    assert_eq!(screen.editor.as_ref().unwrap().draft.text(), address);
    let save = screen.keys["edit:save"];
    let emitted = effects(|fx| screen.activate(save, false, fx));
    let submitted: Vec<_> = emitted.iter().filter_map(|event| match &event.fx {
        Fx::App(AppFx::Ott(OttOut::Action {
            action: OttAction::SubmitSetting { session: 9, key, value }, ..
        })) if key == "remote.address" => Some(value.as_str()),
        _ => None,
    }).collect();
    assert_eq!(submitted, vec![address]);
    assert!(screen.editor.as_ref().unwrap().pending);
}

#[test]
fn remote_address_literal_input_preserves_unicode_caret_clear_and_atomic_limit() {
    let _serial = plx_base::testlock::serial();
    let mut screen = text_policy_editor(OttEditorKind::Setting, "remote.address", 128, OttTextPolicy::Literal);
    for edit in [TextEdit::Commit("https://control.invalid/".into()),
        TextEdit::Commit("映画/一二".into()), TextEdit::Left, TextEdit::Backspace,
        TextEdit::Commit("語".into()), TextEdit::Right] {
        policy_input(&mut screen, edit);
    }
    let draft = &screen.editor.as_ref().unwrap().draft;
    assert_eq!(draft.text(), "https://control.invalid/映画/語二");
    assert_eq!(draft.caret(), draft.text().len());
    let before = draft.clone();
    policy_input(&mut screen, TextEdit::Commit("尾".repeat(200).into()));
    assert_eq!(screen.editor.as_ref().unwrap().draft, before,
        "an over-limit commit must not partially insert or replace the URL");
    policy_input(&mut screen, TextEdit::Clear);
    for text in ["日本", "語"] {
        policy_input(&mut screen, TextEdit::Commit(text.into()));
    }
    let draft = &screen.editor.as_ref().unwrap().draft;
    assert_eq!(draft.text(), "日本語");
    assert_eq!(draft.caret(), "日本語".len());
}

#[test]
fn literal_address_policy_leaves_profile_names_and_search_predictions_unchanged() {
    let _serial = plx_base::testlock::serial();
    for (kind, key) in [(OttEditorKind::Setting, "profile.name"),
        (OttEditorKind::Search, "query"), (OttEditorKind::Search, "remote.address")] {
        let mut screen = text_policy_editor(kind, key, 128, OttTextPolicy::Predictive);
        for text in ["s", "u", "m", "summer ", "holiday "] {
            policy_input(&mut screen, TextEdit::Commit(text.into()));
        }
        let draft = &screen.editor.as_ref().unwrap().draft;
        assert_eq!(draft.text(), "summer holiday ", "{kind:?} {key}");
        assert_eq!(draft.caret(), draft.text().len());
    }
}

#[test]
fn classic_editor_paints_the_matching_search_or_setting_copy_in_every_locale() {
    let _serial = plx_base::testlock::serial();
    let _restore = RestoreScale(plx_ui::typography::current());
    let mut measure = TypographyMeasure::new(ShippedMeasure);
    plx_ui::typography::apply(100, &mut measure).unwrap();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for kind in [OttEditorKind::Search, OttEditorKind::Setting] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.install(snapshot(1, &["one"]));
            deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                text_policy: OttTextPolicy::Predictive, session: 9, kind,
                key: "profile.name".into(), title: "Fixture field".into(),
                value: "draft".into(), secret: false, limit: 128,
            }))));
            let search = kind == OttEditorKind::Search;
            let help = if search { msg::ott_search_help() } else { msg::ott_editor_help() };
            let pending = if search { msg::ott_search_pending() } else { msg::settings_playback_saving() };
            for (phase, expected) in [help, pending, "Fixture rejected the draft"].into_iter().enumerate() {
                if phase == 1 {
                    let save = screen.keys["edit:save"];
                    effects(|fx| screen.activate(save, false, fx));
                    assert!(screen.editor.as_ref().unwrap().pending);
                } else if phase == 2 {
                    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult {
                        session: 9, error: Some(expected.into()),
                    }));
                    assert!(!screen.editor.as_ref().unwrap().pending);
                }
                screen.refresh_presentation(plx_machine::machine::UiMode::Classic, &measure);
                assert!(screen.classic_mode);
                assert_eq!(screen.editor_copy(), expected);
                assert!(screen.view(&measure).header.copy.is_empty(),
                    "Classic paints the copy once in its separate narrative");
                let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
                    focus: Default::default(), press: Default::default(),
                    owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
                let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
                let runs = plx_ui::draw_census::capture_runs(|| {
                    plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
                });
                // TextView wraps the copy into runs; compare the complete visible text,
                // not only view().header.copy (Classic paints a separate narrative).
                let compact = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                let painted = runs.iter().filter(|run| run.ink[3] > 0.0)
                    .map(|run| compact(&run.text)).collect::<String>();
                assert!(painted.contains(&compact(expected)), "{} {kind:?} phase {phase}: {painted}", language.tag());
                if search && phase < 2 {
                    let wrong = if phase == 0 { msg::ott_editor_help() } else { msg::settings_playback_saving() };
                    assert!(!painted.contains(&compact(wrong)), "Search must not paint Setting copy: {}", language.tag());
                }
            }
        }
    }
}


#[test]
fn empty_search_hint_keeps_its_opening_list_scope_in_every_locale() {
    let _serial = plx_base::testlock::serial();
    let measure = TypographyMeasure::new(ShippedMeasure);
    let label = |screen: &OttScreen| screen.form.table.sections[0].rows[0].label.clone();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        assert_ne!(msg::ott_search_empty(), msg::ott_search_empty_favorites());
        for mode in [plx_machine::machine::UiMode::Modern, plx_machine::machine::UiMode::Classic] {
            for favorites in [false, true] {
                let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
                let mut opening = (*snapshot(1, &["one"])).clone();
                opening.page_id = if favorites { "favorites:opening" } else { "channels:opening" }.into();
                screen.install(Arc::new(opening));
                screen.refresh_presentation(mode, &measure);
                deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                    session: 27, kind: OttEditorKind::Search, text_policy: OttTextPolicy::Predictive,
                    key: String::new(), title: String::new(),
                    value: String::new(), secret: false, limit: 256,
                }))));
                let expected = if favorites { msg::ott_search_empty_favorites() } else { msg::ott_search_empty() };
                assert_eq!(label(&screen), expected, "{} {mode:?}", language.tag());
                let help = if favorites { msg::ott_search_help_favorites() } else { msg::ott_search_help() };
                assert_eq!(screen.editor_copy(), help);
                // A background source change must not relabel the still-open draft.
                let mut newer = (*snapshot(2, &["two"])).clone();
                newer.page_id = if favorites { "channels:new" } else { "favorites:new" }.into();
                deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(newer))));
                effects(|fx| screen.edit(&TextEdit::Commit("needle".into()), fx));
                assert_eq!(label(&screen), "needle");
                effects(|fx| screen.edit(&TextEdit::Clear, fx));
                assert_eq!(label(&screen), expected);
                assert_eq!(screen.editor_copy(), help, "background snapshots retain the opening scope");
                let save = screen.keys["edit:save"];
                let changes = effects(|fx| screen.activate(save, false, fx));
                assert_eq!(changes.iter().filter(|change| matches!(&change.fx,
                    Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSearch { session: 27, value }, .. }))
                        if value.is_empty())).count(), 1, "the hint never becomes the submitted query");
            }
            let mut setting = OttScreen::new(EntryId(1), InstanceId(1));
            setting.refresh_presentation(mode, &measure);
            editor(&mut setting, "");
            assert_eq!(label(&setting), msg::ott_value_empty(), "setting copy is unrelated to search");
        }
    }
}

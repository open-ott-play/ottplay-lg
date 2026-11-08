use super::*;
use crate::ott::navigation;
use plx_machine::machine::{PressId, UiMode};

struct ModeHost;
impl Host for ModeHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &UiMode) -> UiMode { *mode }
}

fn step(screen: &mut OttScreen, mode: UiMode, focus: Option<u32>, event: ScreenEvent<ModeHost>) -> Vec<OttAction> {
    let mut cx = Cx::<ModeHost> { views: mode, tick: Default::default(),
        measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(), focus: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    cx.focus.current = focus.map(|elem| FocusKey { entry: EntryId(1), elem });
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    screen.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
    assert_eq!(screen.classic_mode, mode == UiMode::Classic);
    output.into_iter().filter_map(|event| match event.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect()
}

fn focus(screen: &mut OttScreen, mode: UiMode, row: &str) -> u32 {
    let key = screen.keys[&format!("item:{row}")];
    assert!(step(screen, mode, Some(key), ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir }).is_empty());
    key
}

fn fixture(files: bool) -> OttView {
    let mut view = (*snapshot(23, &["first", "second", "retry", "back"])).clone();
    view.page_id = if files { "torrent-files" } else { "torrent-results" }.into();
    view.title = "Torrent fixture".into();
    view.preview_context = None;
    for (i, row) in view.rows.iter_mut().take(2).enumerate() {
        row.id = if files { format!("torrent.file:23:lease-1:{}", [4, 19][i]) }
            else { format!("torznab:23:source:{}", [7, 31][i]) };
        row.action = OttAction::Play(if files { row.id.clone() } else { format!("torrent:{}", row.id) });
        let title = if files { ["First.mp4", "Folder/Second.mkv"][i] } else { ["First release", "Second release"][i] };
        // Deliberately nonordinal prefixes: display must count content, not file IDs or labels.
        row.title = format!("{}  {title}", [4, 19][i]);
        row.content_title = Some(title.into());
        row.detail = format!("{} MiB", i + 1);
        row.description = row.detail.clone();
    }
    for (row, (id, title)) in view.rows.iter_mut().skip(2).zip([
        ("torrent.retry", "Retry"), ("torrent.back", "Back"),
    ]) {
        row.id = id.into(); row.title = title.into(); row.description.clear();
        row.action = OttAction::EditSetting(id.into());
    }
    if !files {
        let mut search = view.rows[2].clone();
        search.id = "torrent.query:23:edit".into(); search.title = "Search".into();
        search.action = OttAction::EditSetting(search.id.clone());
        // Interleaved control exercises the shared content-only numbering contract.
        view.rows.insert(1, search);
        let mut more = view.rows.last().unwrap().clone();
        more.id = "torrent.more:23:next".into(); more.title = "Load more".into();
        more.action = OttAction::EditSetting(more.id.clone());
        view.rows.push(more);
    }
    view
}

fn screen(view: OttView, mode: UiMode) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    assert!(step(&mut screen, mode, None, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(view)))).is_empty());
    screen.classic_visible = true;
    screen
}

fn painted_numbers(screen: &OttScreen) -> Vec<String> {
    plx_ui::draw_census::capture_runs(|| screen.form.table.draw(plx_ui::Painter::recording(),
        screen.table_frame(), &plx_ui::fixture::FixtureMeasure))
        .into_iter().filter(|run| run.align == 2 && !run.text.is_empty()
            && run.text.bytes().all(|b| b.is_ascii_digit())).map(|run| run.text).collect()
}

#[test]
fn torrent_classic_content_uses_shared_compact_rows_and_content_only_ordinals() {
    let _serial = plx_base::testlock::serial();
    for files in [false, true] {
        let view = fixture(files);
        let expected: Vec<_> = view.rows.iter().filter(|row| torrent_catalogue_row(&view.page_id, row))
            .map(|row| row.id.clone()).collect();
        let mut screen = screen(view.clone(), UiMode::Classic);
        assert!(screen.form.table.is_classic_catalogue());
        assert_eq!(screen.catalog_keys, expected.iter().map(|id| screen.keys[&format!("item:{id}")]).collect::<Vec<_>>());
        assert_eq!(painted_numbers(&screen), ["1", "2"]);
        assert_eq!(screen.form.table.sections[0].rows.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            view.rows.iter().filter(|row| torrent_catalogue_row(&view.page_id, row))
                .map(|row| row.content_title.as_deref().unwrap()).collect::<Vec<_>>());
        for row in &view.rows {
            let key = screen.keys[&format!("item:{}", row.id)];
            assert!(screen.form.index_of_key(RowKey(key)).is_some());
            assert_eq!(step(&mut screen, UiMode::Classic, Some(key), ScreenEvent::Activate(key)), [row.action.clone()]);
        }
        assert!(screen.snapshot.rows.iter().all(|row| row.media.is_none() && row.guide.is_none()
            && row.current_programme.is_none() && !row.guide_available && !row.records_available));
        assert!(screen.preview.is_none(), "compact layout does not create preview authority");
    }
}

#[test]
fn torrent_classic_disabled_content_keeps_its_ordinal_but_cannot_activate() {
    let _serial = plx_base::testlock::serial();
    let mut view = fixture(true);
    view.rows[0].enabled = false;
    let mut screen = screen(view, UiMode::Classic);
    assert_eq!(painted_numbers(&screen), ["1", "2"]);
    let key = focus(&mut screen, UiMode::Classic, "torrent.file:23:lease-1:4");
    assert!(step(&mut screen, UiMode::Classic, Some(key), ScreenEvent::Activate(key)).is_empty());
    assert!(step(&mut screen, UiMode::Classic, Some(key), ScreenEvent::PressHold(PressId(1))).is_empty());
    assert!(!screen.browse);
    assert!(navigation::available(&screen.snapshot, screen.media, Default::default()).iter()
        .all(|action| !matches!(action.id, "nav:play" | "nav:favorite" | "nav:description")));
}

#[test]
fn torrent_classic_browse_keeps_the_selected_file_and_description_without_favorite() {
    let _serial = plx_base::testlock::serial();
    let mut screen = screen(fixture(true), UiMode::Classic);
    let key = focus(&mut screen, UiMode::Classic, "torrent.file:23:lease-1:19");
    assert!(step(&mut screen, UiMode::Classic, Some(key), ScreenEvent::PressHold(PressId(1))).is_empty());
    assert!(screen.browse);
    let actions = navigation::available(&screen.snapshot, screen.media, Default::default());
    assert!(actions.iter().any(|action| action.id == "nav:description"));
    assert!(actions.iter().any(|action| matches!(&action.action,
        FormAction::Forward(OttAction::Play(id)) if id == "torrent.file:23:lease-1:19")));
    for id in ["nav:favorite", "nav:guide", "nav:records"] {
        assert!(!actions.iter().any(|action| action.id == id));
        assert!(screen.form.index_of_key(RowKey(screen.keys[id])).is_none());
    }
    let description = screen.keys["nav:description"];
    assert!(step(&mut screen, UiMode::Classic, Some(description), ScreenEvent::Activate(description)).is_empty());
    assert!(screen.reading && !screen.browse);
    assert_eq!(screen.media.map(|index| screen.snapshot.rows[index].description.as_str()), Some("2 MiB"));
    assert!(step(&mut screen, UiMode::Classic, None, ScreenEvent::Input(plx_machine::machine::InputEvent {
        at: Default::default(), source: plx_machine::machine::Source::Script,
        kind: InputKind::Key { key: Key::Back, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
    })).is_empty());
    assert!(!screen.reading);
    assert_eq!(screen.form.selected_key(), Some(RowKey(key)));
    assert_eq!(step(&mut screen, UiMode::Classic, Some(key), ScreenEvent::Activate(key)),
        [OttAction::Play("torrent.file:23:lease-1:19".into())]);
}

#[test]
fn torrent_classic_mode_switch_preserves_identity_and_modern_row_presentation() {
    let _serial = plx_base::testlock::serial();
    let view = fixture(false);
    let mut screen = screen(view.clone(), UiMode::Classic);
    let key = focus(&mut screen, UiMode::Classic, "torznab:23:source:31");
    for mode in [UiMode::Modern, UiMode::Classic] {
        assert!(step(&mut screen, mode, Some(key), ScreenEvent::Tick(Default::default())).is_empty());
        assert_eq!(screen.form.selected_key(), Some(RowKey(key)));
        assert_eq!(screen.keys["item:torznab:23:source:31"], key);
        assert_eq!(screen.form.table.is_classic_catalogue(), mode == UiMode::Classic);
        assert!(!screen.grid_active());
        if mode == UiMode::Modern {
            for row in &view.rows {
                let index = screen.form.index_of_key(RowKey(screen.keys[&format!("item:{}", row.id)])).unwrap();
                let rendered = screen.form.table.row_mut(index as i32).unwrap();
                assert_eq!(rendered.label, row.title);
                assert_eq!(rendered.detail, row.detail);
            }
            assert!(step(&mut screen, mode, Some(key), ScreenEvent::PressHold(PressId(1))).is_empty());
            assert!(!screen.browse, "Modern unsupported Favorite does not become a new action");
        }
    }
}

#[test]
fn torrent_classic_number_admission_requires_visible_compact_owned_content() {
    let _serial = plx_base::testlock::serial();
    for files in [false, true] {
        let mut screen = screen(fixture(files), UiMode::Classic);
        assert!(screen.accepts_channel_number());
        screen.classic_visible = false; assert!(!screen.accepts_channel_number()); screen.classic_visible = true;
        screen.reading = true; assert!(!screen.accepts_channel_number()); screen.reading = false;
        effects(|fx| screen.open_browse(fx)); assert!(!screen.accepts_channel_number());
        effects(|fx| screen.close_browse(fx)); assert!(screen.accepts_channel_number());
        step(&mut screen, UiMode::Modern, None, ScreenEvent::Tick(Default::default()));
        assert!(!screen.accepts_channel_number());
    }
    for case in 0..5 {
        let mut view = fixture(true);
        match case {
            0 => view.rows.retain(|row| !torrent_catalogue_row("torrent-files", row)),
            1 => view.rows.iter_mut().take(2).for_each(|row| row.enabled = false),
            2 => view.rows[0].action = OttAction::Play("torrent.file:23:another-session:4".into()),
            3 => view.rows[0].action = OttAction::EditSetting(view.rows[0].id.clone()),
            _ => view.page_id = "torrent-files:foreign".into(),
        }
        let screen = screen(view, UiMode::Classic);
        assert!(!screen.accepts_channel_number(), "case {case}");
    }
    let mut screen = screen(fixture(true), UiMode::Classic);
    let spec = Arc::new(OttEditor { session: 1, kind: OttEditorKind::Setting,
        text_policy: OttTextPolicy::Literal, key: "fixture".into(), title: "Fixture".into(),
        value: String::new(), secret: false, limit: 64 });
    step(&mut screen, UiMode::Classic, None, ScreenEvent::App(AppMsg::OttEditor(spec)));
    assert!(!screen.accepts_channel_number());
}

#[test]
fn torrent_favorite_exclusion_does_not_change_ordinary_playable_rows() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = screen((*snapshot(1, &["ordinary"])).clone(), mode);
        let key = screen.keys["item:ordinary"];
        assert!(navigation::available(&screen.snapshot, Some(0), Default::default()).iter()
            .any(|action| action.id == "nav:favorite"));
        if mode == UiMode::Modern {
            assert_eq!(step(&mut screen, mode, Some(key), ScreenEvent::PressHold(PressId(1))),
                [OttAction::ToggleFavorite("ordinary".into())]);
        }
    }
}

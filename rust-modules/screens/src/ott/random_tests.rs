use super::*;
use plx_appkit::menu_visibility::{self, Item, Visibility};

struct Reset(Visibility);
impl Drop for Reset { fn drop(&mut self) { menu_visibility::project(self.0); } }
fn view(context: Option<&str>, revision: u64) -> Arc<OttView> {
    Arc::new(OttView {
        page_id: "channels".into(), revision,
        random_context: context.map(str::to_owned),
        rows: vec![OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, id: "item".into(), title: "Item".into(), detail: String::new(),
            description: String::new(), poster: None, media: None, enabled: true, favorite: false,
            action: OttAction::Play("item".into()) }],
        ..OttView::default()
    })
}
fn has_random(screen: &OttScreen) -> bool {
    screen.keys.get("nav:random").is_some_and(|key|
        screen.form.index_of_key(RowKey(*key)).is_some())
}

#[test]
fn actual_menu_exposes_current_random_action_without_adding_a_catalog_row() {
    let _reset = Reset(menu_visibility::projected());
    menu_visibility::project(Default::default());
    let mut screen = OttScreen::new(EntryId(97), InstanceId(97));
    screen.install(view(Some("context-a"), 1));
    assert_eq!(screen.snapshot.rows.len(), 1);
    screen.browse = true; screen.install(view(Some("context-a"), 2));
    assert!(has_random(&screen));
    assert_eq!(screen.snapshot.random_context.as_deref(), Some("context-a"));
    let index = screen.form.index_of_key(RowKey(screen.keys["nav:random"])).unwrap();
    assert!(matches!(screen.form.activate(index), Some(Activation::Action(
        FormAction::Forward(OttAction::Random(context)))) if context == "context-a"));
    screen.install(view(None, 3));
    assert!(!has_random(&screen));
}

#[test]
fn live_play_visibility_toggle_hides_random_and_reset_restores_it() {
    let _reset = Reset(menu_visibility::projected());
    menu_visibility::project(Default::default());
    let mut screen = OttScreen::new(EntryId(98), InstanceId(98));
    screen.install(view(Some("context-b"), 1));
    screen.browse = true; screen.install(view(Some("context-b"), 2));
    assert!(has_random(&screen));
    let mut hidden = Visibility::default(); hidden.hide(Item::Play);
    menu_visibility::project(hidden); screen.install(view(Some("context-b"), 3));
    assert!(!has_random(&screen));
    menu_visibility::project(Default::default()); screen.install(view(Some("context-b"), 4));
    assert!(has_random(&screen));
}

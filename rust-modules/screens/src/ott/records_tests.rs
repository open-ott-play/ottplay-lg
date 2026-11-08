use super::*;

fn view(available: bool) -> OttView {
    OttView { page_id: "channels".into(), rows: vec![OttRow {
        current_programme: None, content_title: None, guide: None, guide_available: true,
        records_available: available, media: None, id: "stable-channel".into(),
        title: "Channel".into(), detail: String::new(), description: String::new(),
        poster: None, action: OttAction::Play("stable-channel".into()),
        enabled: true, favorite: false,
    }], ..Default::default() }
}

#[test]
fn records_action_requires_explicit_selected_row_capability_and_keeps_stable_id() {
    let _serial = plx_base::testlock::serial();
    let snapshot = view(true);
    let actions = navigation::available(&snapshot, Some(0), Default::default());
    let action = actions.iter().find(|action| action.id == "nav:records").unwrap();
    assert!(matches!(&action.action, FormAction::Forward(OttAction::OpenRecords(id)) if id == "stable-channel"));
    assert!(navigation::CONTEXT_IDS.contains(&"nav:records"));
    for snapshot in [view(false), {
        let mut snapshot = view(true); snapshot.rows[0].enabled = false; snapshot
    }] {
        assert!(!navigation::available(&snapshot, Some(0), Default::default())
            .iter().any(|action| action.id == "nav:records"));
    }
    assert!(!navigation::available(&view(true), None, Default::default())
        .iter().any(|action| action.id == "nav:records"));
}

#[test]
fn records_capability_is_part_of_the_canonical_screen_shape_and_state() {
    let mut screen = OttScreen::new(EntryId(97), InstanceId(97));
    screen.snapshot = Arc::new(view(false));
    let mut before = Canon::new(); screen.write(&mut before);
    screen.snapshot = Arc::new(view(true));
    let mut after = Canon::new(); screen.write(&mut after);
    assert_ne!(before.finish(), after.finish(), "a replay must see Records authority change");
    assert!(SHAPE.contains("records_available:bool"));
}

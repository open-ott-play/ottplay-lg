use super::*;

#[test]
fn a_description_retires_when_its_subject_disappears_or_becomes_disabled() {
    for removed in [false, true] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(1, &["original", "neighbour"]));
        effects(|fx| screen.show_details(fx));
        assert!(screen.reading);
        screen.read_target = 12;
        let mut view = (*screen.snapshot).clone();
        view.revision += 1;
        if removed { view.rows.remove(0); } else { view.rows[0].enabled = false; }
        screen.install(Arc::new(view));
        assert!(!screen.reading, "the old reader must close");
        assert_eq!(screen.read_target, 0);
        assert_eq!(screen.media, None, "do not substitute a neighbour's description");
        assert_eq!(screen.focus_request, Some(screen.keys["nav:browse"]));
        assert!(screen.preview.is_none());
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter().any(|effect| matches!(effect.fx, Fx::App(_))));
        let actions = deliver_recorded(&mut screen, key_event(Key::Back));
        assert!(actions.iter().any(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Back, .. })))));
        assert!(!screen.reading);
    }
}

#[test]
fn a_description_keeps_its_own_subject_when_catalogue_order_changes() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["original", "neighbour"]));
    effects(|fx| screen.show_details(fx));
    screen.read_target = 4;
    screen.install(snapshot(2, &["inserted", "neighbour", "original"]));
    assert!(screen.reading);
    assert_eq!(screen.snapshot.rows[screen.media.unwrap()].id, "original");
    assert_eq!(screen.read_target, 4);
    deliver(&mut screen, key_event(Key::Back));
    assert!(!screen.reading);
}

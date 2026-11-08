//! Exercise the existing addressed More reducer for each new capability.
use super::*;

fn offer(fixture: &mut Fixture, action: Action) {
    fixture.offer.as_mut().unwrap().actions = Caps {
        chapters: action == Action::Chapters,
        from_beginning: action == Action::FromBeginning,
        go_to_detail: action == Action::GoToDetail,
        ..Default::default()
    };
}

#[test]
fn accessibility_actions_use_the_same_pending_request_and_refusal_path() {
    let _lock = plx_base::testlock::serial();
    for action in [Action::Chapters, Action::FromBeginning, Action::GoToDetail] {
        let mut f = Fixture::new(); offer(&mut f, action);
        let out = f.step(ScreenEvent::Activate(action.key()), ENTRY);
        assert!(!out.iter().any(|event| matches!(event.fx, Fx::Nav(NavOp::Dismiss(_)))));
        let request = out.into_iter().find_map(|event| match event.fx {
            Fx::App(AppFx::Player(PlayerReq::Menu(request))) => Some(request), _ => None,
        }).expect("an offered alternative emits an addressed request");
        assert_eq!(request.action, action); assert_eq!(request.owner, OWNER);
        assert_eq!(request.offer, f.offer.clone().unwrap());
        assert!(f.page.accepts_menu_request(&request));
        let out = f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult {
            request, outcome: Err("Action is no longer available"),
        }), ENTRY);
        assert_eq!(f.note(), Some("Action is no longer available"));
        assert!(!out.iter().any(|event| matches!(event.fx, Fx::Nav(NavOp::Dismiss(_)))));
    }
}

#[test]
fn stale_accessibility_activation_cannot_borrow_a_new_source_or_removed_capability() {
    let _lock = plx_base::testlock::serial();
    for action in [Action::Chapters, Action::FromBeginning, Action::GoToDetail] {
        for refusal in 0..3 {
            let mut f = Fixture::new(); offer(&mut f, action);
            f.step(ScreenEvent::Tick(Tick::default()), ENTRY);
            let owner = if refusal == 2 { EntryId(99) } else { ENTRY };
            match refusal {
                0 => f.offer.as_mut().unwrap().actions = Caps::default(),
                1 => plx_media::route::install_plex_playback_for_test(&mut f.ps,
                    plx_plex::plex::ServerId::from_raw(0), "replacement"),
                _ => {},
            }
            let out = f.step(ScreenEvent::Activate(action.key()), owner);
            assert!(!out.iter().any(|event| matches!(event.fx,
                Fx::App(AppFx::Player(_)) | Fx::Nav(NavOp::Dismiss(_)))));
            assert!(f.page.menu_pending.is_none());
        }
    }
}

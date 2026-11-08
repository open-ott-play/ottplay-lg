// Included in login::tests so these drive the existing screen and publication fixtures.
mod origin_return {
    use super::*;

    // The unused key between Details and alert answers becomes the visible Cancel action.
    // Keeping this local lets the tests fail behaviorally against the original Login screen.
    const CANCEL: u32 = 2;

    fn offered(phase: Phase) -> auth::owner::SessionSnapshot {
        let mut read = snapshot(phase, 41, "ABCD");
        read.flow_epoch = 7;
        read.origin_cancel = Some(auth::owner::LoginOrigin { id: 91, flow_epoch: 7 });
        read
    }

    fn assert_cancel_only(effects: &[Stamped<SessionHost>], expected: auth::owner::LoginOrigin) {
        assert_eq!(effects.len(), 1, "a cancel request must not navigate or start another operation");
        assert!(matches!(&effects[0].fx,
            Fx::App(AppFx::Session(auth::SessionCmd::CancelLoginFrom { origin, reply }))
            if *origin == expected && reply.instance == 44 && reply.correlation == 1));
    }

    #[test]
    fn origin_cancel_is_available_during_a_fresh_wait_without_enabling_retry() {
        let measure = plx_ui::fixture::FixtureMeasure;
        for phase in [Phase::Creating, Phase::Waiting, Phase::Discovering] {
            let read = offered(phase);
            let screen = LoginScreen::new(EntryId(0), read.read());
            let cx = cx_with(&measure, &read);
            assert_eq!(screen.row().as_slice(), [CANCEL]);
            assert_eq!(screen.control_kind(), None, "Cancel must not turn a fresh wait into stalled Retry");
            assert!(!screen.readout_labels().1, "a fresh wait has no slow-progress reason");
            assert!(Focusable::<SessionHost>::place(&screen, &CANCEL, &cx, At::Drawn).is_some());
            assert_eq!(Focusable::<SessionHost>::group_of(&screen, &CANCEL, &cx), Some(CONTROL_GROUP));
            assert_eq!(Focusable::<SessionHost>::seat(&screen, CONTROL_GROUP,
                Focusable::<SessionHost>::place(&screen, &CANCEL, &cx, At::Drawn).unwrap(), &cx).elem, CANCEL);
        }
    }

    #[test]
    fn visible_cancel_and_physical_back_request_the_same_addressed_origin_only() {
        let _serial = plx_base::testlock::serial();
        let read = offered(Phase::Waiting);
        let origin = read.origin_cancel.unwrap();
        let measure = plx_ui::fixture::FixtureMeasure;
        for event in [ScreenEvent::Activate(CANCEL), key_back_down()] {
            let mut screen = LoginScreen::new(EntryId(0), read.read());
            let before = (screen.phase_ms, screen.phase, screen.qr_gen);
            let (_, effects) = step_ev_with(&mut screen, &event, &read, InstanceId(44), &measure);
            assert_cancel_only(&effects, origin);
            assert_eq!((screen.phase_ms, screen.phase, screen.qr_gen), before,
                "emission is not cancellation success");
            assert!(screen.row().position(CANCEL).is_some(), "only a Session publication may retire the offer");
        }
    }

    #[test]
    fn originless_login_keeps_root_back_and_rejects_an_invisible_cancel() {
        let _serial = plx_base::testlock::serial();
        let read = snapshot(Phase::Waiting, 41, "ABCD");
        let measure = plx_ui::fixture::FixtureMeasure;
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        assert!(screen.row().position(CANCEL).is_none());
        let (_, hidden) = step_ev_with(&mut screen, &ScreenEvent::Activate(CANCEL), &read, InstanceId(44), &measure);
        assert!(hidden.is_empty());
        let (_, effects) = step_ev_with(&mut screen, &key_back_down(), &read, InstanceId(44), &measure);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0].fx,
            Fx::App(AppFx::Session(auth::SessionCmd::BackAtRoot { reply }))
            if reply.instance == 44 && reply.correlation == 1));
    }

    #[test]
    fn withdrawn_origin_makes_a_previously_visible_cancel_inert() {
        let _serial = plx_base::testlock::serial();
        let mut read = offered(Phase::Waiting);
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        assert!(screen.row().position(CANCEL).is_some());
        read.origin_cancel = None;
        read.flow_epoch += 1;
        let measure = plx_ui::fixture::FixtureMeasure;
        let _ = step_ev_with(&mut screen, &tick_ev(16), &read, InstanceId(44), &measure);
        assert!(screen.row().position(CANCEL).is_none());
        let (_, effects) = step_ev_with(&mut screen, &ScreenEvent::Activate(CANCEL), &read, InstanceId(44), &measure);
        assert!(effects.is_empty());
    }

    #[test]
    fn focused_cancel_withdrawal_reconciles_to_details_without_trapping_navigation() {
        let _serial = plx_base::testlock::serial();
        let mut read = failed_with(auth::owner::IncidentState::NotNow);
        read.origin_cancel = offered(Phase::Error).origin_cancel;
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        let measure = plx_ui::fixture::FixtureMeasure;
        assert_eq!(screen.row().as_slice(), [CONTROL, DETAILS, CANCEL]);
        let owner = InputOwner::Entry(EntryId(0));
        let mut focus = plx_ui::focus::FocusEngine::new();
        focus.set(owner, screen.key(CANCEL), Some(CONTROL_GROUP), plx_ui::screen::By::Restore);
        read.origin_cancel = None;
        let _ = step_ev_with(&mut screen, &tick_ev(16), &read, InstanceId(44), &measure);
        assert_eq!(screen.row().as_slice(), [CONTROL, DETAILS]);
        let cx = cx_with(&measure, &read);
        let outcome = focus.reconcile(owner, &screen, &cx);
        assert!(matches!(outcome, plx_ui::focus::Outcome::Moved {
            from: Some(from), to, by: plx_ui::screen::By::Reconcile,
        } if from.elem == CANCEL && to.elem == DETAILS));
        assert!(matches!(Focusable::<SessionHost>::neighbour(&screen, screen.key(DETAILS), Dir::Left, &cx),
            Step::Move(key) if key.elem == CONTROL));
    }

    #[test]
    fn a_new_origin_publication_replaces_only_the_cancellation_address() {
        let _serial = plx_base::testlock::serial();
        let mut read = offered(Phase::Waiting);
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        read.flow_epoch = 8;
        read.origin_cancel = Some(auth::owner::LoginOrigin { id: 92, flow_epoch: 8 });
        let measure = plx_ui::fixture::FixtureMeasure;
        let _ = step_ev_with(&mut screen, &tick_ev(16), &read, InstanceId(44), &measure);
        let (_, effects) = step_ev_with(&mut screen, &ScreenEvent::Activate(CANCEL), &read, InstanceId(44), &measure);
        assert_cancel_only(&effects, read.origin_cancel.unwrap());
    }

    #[test]
    fn same_visible_login_hashes_the_exact_effect_controlling_origin_address() {
        let first = offered(Phase::Waiting);
        let screen = LoginScreen::new(EntryId(0), first.read());
        let digest = |screen: &LoginScreen| {
            let mut canon = Canon::new();
            screen.state.write(&mut canon);
            canon.finish()
        };
        for next in [
            auth::owner::LoginOrigin { id: 92, flow_epoch: 7 },
            auth::owner::LoginOrigin { id: 91, flow_epoch: 8 },
        ] {
            let mut changed = offered(Phase::Waiting);
            changed.origin_cancel = Some(next);
            changed.flow_epoch = next.flow_epoch;
            let other = LoginScreen::new(EntryId(0), changed.read());
            assert_eq!(screen.row().as_slice(), other.row().as_slice());
            assert_eq!(screen.phase, other.phase);
            assert_eq!(screen.qr_gen, other.qr_gen);
            assert_ne!(digest(&screen), digest(&other),
                "same-looking screens must not hide a different cancellation address");
        }
    }

    #[test]
    fn persistence_warning_hides_cancel_and_preserves_its_acknowledgement_door() {
        let _serial = plx_base::testlock::serial();
        let mut read = offered(Phase::Ready);
        let key = auth::owner::PersistenceWarningKey { epoch: 7, req: 9 };
        read.persistence_warning = Some(auth::owner::PersistenceWarning {
            key, site: auth::owner::PersistenceWarningSite::Final,
            helper: None, candidate_errnos: [None; 8], persistence: None,
        });
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        let measure = plx_ui::fixture::FixtureMeasure;
        assert_eq!(screen.row().as_slice(), [CONTROL]);
        let (_, hidden) = step_ev_with(&mut screen, &ScreenEvent::Activate(CANCEL), &read, InstanceId(44), &measure);
        assert!(hidden.is_empty());
        let (_, effects) = step_ev_with(&mut screen, &ScreenEvent::Activate(CONTROL), &read, InstanceId(44), &measure);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0].fx,
            Fx::App(AppFx::Session(auth::SessionCmd::AcknowledgePersistenceWarning { key: actual })) if *actual == key));
    }

    #[test]
    fn details_card_back_closes_the_card_before_requesting_login_cancellation() {
        let _serial = plx_base::testlock::serial();
        let mut read = failed_with(auth::owner::IncidentState::NotNow);
        read.origin_cancel = offered(Phase::Error).origin_cancel;
        let mut screen = LoginScreen::new(EntryId(0), read.read());
        let measure = plx_ui::fixture::FixtureMeasure;
        let _ = step_ev_with(&mut screen, &ScreenEvent::Activate(DETAILS), &read, InstanceId(44), &measure);
        assert!(screen.report.alert.is_open());
        let (_, effects) = step_ev_with(&mut screen, &key_back_down(), &read, InstanceId(44), &measure);
        assert!(!screen.report.alert.is_open());
        assert!(!effects.iter().any(|effect| matches!(effect.fx, Fx::App(AppFx::Session(_)))),
            "Back belongs to the open card, not the underlying login");
    }

    #[test]
    fn cancel_keeps_retry_and_details_and_uses_safe_shared_geometry_in_every_language() {
        use plx_base::fontcov::advances::ShippedMeasure;
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let mut measure = TypographyMeasure::new(ShippedMeasure);
        for language in plx_platform::i18n::SHIPPED {
            let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
            for percent in [20, 100, 200] {
                measure.set_font_scale(FontScale::from_percent(percent).unwrap());
                let mut read = offered(Phase::Waiting);
                read.incident = Some(incident(auth::owner::IncidentState::NotNow,
                    plx_telemetry::telemetry::incident::IncidentKind::LinkStalled));
                let mut screen = LoginScreen::new(EntryId(0), read.read());
                screen.phase_ms = QR_ESCAPE_AFTER_MS;
                assert_eq!(screen.row().as_slice(), [DETAILS, CONTROL, CANCEL]);
                let cancel = screen.elem_rect(CANCEL, &measure).expect("visible Cancel has hit-test geometry");
                let details = screen.elem_rect(DETAILS, &measure).unwrap();
                assert!(inside_safe(cancel), "{} at {percent}%", language.tag());
                assert!(inside_safe(details), "{} at {percent}%", language.tag());
                assert!(cancel.x >= details.x + details.w || details.x >= cancel.x + cancel.w
                    || cancel.y >= details.y + details.h || details.y >= cancel.y + cancel.h,
                    "Cancel and Details must not overlap");
                let width = Button::pill_w_measured(plx_platform::i18n::msg::settings_cancel_c(),
                    theme::size::BODY, false, None, &measure);
                assert!(cancel.w >= width);
            }
        }
    }
}

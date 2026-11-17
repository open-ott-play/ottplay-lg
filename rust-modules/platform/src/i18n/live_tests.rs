//! Shared registry tests use isolated local instances, not process-global mutation.
use super::*;
use super::super::{current, english, language_on_this_thread_for_test, render_in_english, Clock, Language, MonthWidth, SHIPPED};
use std::sync::atomic::Ordering;

fn registry() -> Registry {
    Registry::new(Preference::En, BootInputs::default()).unwrap()
}

#[test]
fn preparing_a_language_is_not_publication() {
    let registry = registry();
    let old = registry.current() as *const _;
    let candidate = registry.prepare(Preference::Ru).unwrap();
    assert_eq!(candidate.preference(), Preference::Ru);
    assert_eq!(registry.current() as *const _, old);
    assert_eq!(registry.epoch(), 0);
    assert_eq!(registry.publish(candidate), Ok(1));
    assert_eq!(registry.current().language(), Language::Ru);
    assert_eq!(registry.current().plural(2), icu_plurals::PluralCategory::Few);
    assert_eq!(registry.current().decimal(45, 1), "4,5");
    assert_eq!(registry.current().month_day(3, 12, MonthWidth::Long).as_deref(), Some("12 марта"));
}

#[test]
fn finite_switches_reuse_contexts_but_epoch_detects_aba() {
    let registry = registry();
    let initial = registry.current() as *const _;
    let stale = registry.prepare(Preference::Ru).unwrap();
    registry.publish(registry.prepare(Preference::Ru).unwrap()).unwrap();
    registry.publish(registry.prepare(Preference::En).unwrap()).unwrap();
    assert_eq!(registry.current() as *const _, initial);
    assert_eq!(registry.epoch(), 2);
    assert_eq!(registry.publish(stale), Err(PublishError::Superseded));
    for _ in 0..4 {
        for preference in std::iter::once(Preference::System).chain(SHIPPED) {
            registry.publish(registry.prepare(preference).unwrap()).unwrap();
        }
    }
    assert_eq!(registry.contexts.iter().filter(|slot| slot.get().is_some()).count(), SHIPPED.len() + 1);
    let pointers: Vec<_> = registry.contexts.iter().map(|slot| slot.get().unwrap().as_ref() as *const _).collect();
    for preference in std::iter::once(Preference::System).chain(SHIPPED) {
        registry.publish(registry.prepare(preference).unwrap()).unwrap();
    }
    assert_eq!(pointers, registry.contexts.iter().map(|slot| slot.get().unwrap().as_ref() as *const _).collect::<Vec<_>>());
}

#[test]
fn failure_and_discard_do_not_change_publication_or_poison_retry() {
    let registry = registry();
    let before = registry.current() as *const _;
    let failed = registry.prepare_with(Preference::Ru, |_, _| Err(PrepareError::Unavailable));
    assert!(matches!(failed, Err(PrepareError::Unavailable)));
    assert_eq!(registry.current() as *const _, before);
    assert_eq!(registry.epoch(), 0);
    assert!(registry.contexts[Preference::Ru.slot() as usize].get().is_none());
    let _discarded = registry.prepare(Preference::Ru).unwrap();
    assert_eq!(registry.current() as *const _, before);
    assert_eq!(registry.epoch(), 0);
    assert_eq!(registry.publish(registry.prepare(Preference::Ru).unwrap()), Ok(1));
}

#[test]
fn no_op_and_epoch_exhaustion_are_explicit_and_do_not_wrap() {
    let registry = registry();
    assert_eq!(registry.publish(registry.prepare(Preference::En).unwrap()), Ok(0));
    let last = u64::MAX >> SLOT_BITS;
    registry.published.store((last << SLOT_BITS) | u64::from(Preference::En.slot()), Ordering::Release);
    assert_eq!(registry.publish(registry.prepare(Preference::Ru).unwrap()), Err(PublishError::Exhausted));
    assert_eq!(registry.epoch(), last);
    assert_eq!(registry.current().preference(), Preference::En);
    assert_eq!(registry.publish(registry.prepare(Preference::En).unwrap()), Ok(last));
}

#[test]
fn system_uses_captured_boot_inputs_after_switching() {
    let registry = Registry::new(Preference::En, BootInputs {
        ui: Some("ru-RU".into()), format: Some("de-DE".into()), clock: Some("24".into()),
        environment: Some("fr-FR".into()), ..Default::default()
    }).unwrap();
    registry.publish(registry.prepare(Preference::System).unwrap()).unwrap();
    let current = registry.current();
    assert_eq!(current.language(), Language::Ru);
    assert_eq!(current.format_locale(), "de-DE");
    assert_eq!(current.clock(), Clock::H24);
    assert_eq!(current.decimal(45, 1), "4,5");
    assert_eq!(current.time_minute([16, 12, 20]).as_deref(), Some("16:12"));
    registry.publish(registry.prepare(Preference::Fr).unwrap()).unwrap();
    registry.publish(registry.prepare(Preference::System).unwrap()).unwrap();
    assert!(std::ptr::eq(current, registry.current()));
}

#[test]
fn controlled_and_forced_boots_refuse_live_changes() {
    let controlled = Registry::new(Preference::Ru, BootInputs::controlled()).unwrap();
    assert_eq!(controlled.current().language(), Language::Ru);
    assert_eq!(controlled.current().format_locale(), "en-US");
    assert!(matches!(controlled.prepare(Preference::En), Err(PrepareError::Controlled)));
    let forced = Registry::new(Preference::En, BootInputs {
        forced: Some("qps-ploc".into()), ..Default::default()
    }).unwrap();
    assert_eq!(forced.current().language(), Language::Pseudo);
    assert!(matches!(forced.prepare(Preference::Ru), Err(PrepareError::ForcedOverride)));
    assert_eq!(forced.epoch(), 0);
}

#[test]
fn english_diagnostic_and_nested_test_overrides_keep_precedence() {
    let _russian = language_on_this_thread_for_test(Preference::Ru);
    assert_eq!(current().language(), Language::Ru);
    render_in_english(|| {
        assert!(std::ptr::eq(current(), english()));
        let _french = language_on_this_thread_for_test(Preference::Fr);
        assert_eq!(current().language(), Language::Fr);
        render_in_english(|| assert!(std::ptr::eq(current(), english())));
        assert_eq!(current().language(), Language::Fr);
    });
    assert_eq!(current().language(), Language::Ru);
}


/// Cold global initialization must fit the same worker budget as Plex source
/// resolution. A fresh process prevents unrelated locale tests warming it first.
#[test]
fn cold_locale_initialization_and_publication_fit_a_small_worker() {
    const CHILD: &str = "PLX_I18N_COLD_SMALL_WORKER_TEST";
    if std::env::var_os(CHILD).is_some() {
        assert!(super::super::CURRENT.get().is_none(), "child must start with a cold registry");
        std::thread::Builder::new().name("cold-locale-worker".into())
            .stack_size(256 * 1024).spawn(|| {
                // This is the original process-global current()->registry()->
                // Registry::new path reached by Plex's cold request worker.
                let initial = current();
                assert_eq!(initial.language(), Language::En);
                assert_eq!(initial.decimal(45, 1), "4.5");
                assert_eq!(initial.month_day(3, 12, MonthWidth::Long).as_deref(), Some("12 March"));
                assert!(std::ptr::eq(initial, current()));
                // A fresh independent registry also exercises lazy slot allocation
                // and immutable context reuse on this unchanged small stack.
                let registry = registry();
                let english = registry.current() as *const _;
                registry.publish(registry.prepare(Preference::Ru).unwrap()).unwrap();
                assert_eq!(registry.current().language(), Language::Ru);
                assert_eq!(registry.current().decimal(45, 1), "4,5");
                registry.publish(registry.prepare(Preference::En).unwrap()).unwrap();
                assert_eq!(registry.current() as *const _, english);
            }).unwrap().join().unwrap();
        return;
    }
    // The marker only selects the fresh-process arm; no locale, stack-size
    // environment override, production warmup, or global preference is changed.
    let module = module_path!().split_once("::").unwrap().1;
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD, "1")
        .arg("--exact").arg(format!("{module}::cold_locale_initialization_and_publication_fit_a_small_worker"))
        .arg("--nocapture").status().unwrap();
    assert!(status.success(), "cold locale initialization failed on the ordinary small worker stack");
}

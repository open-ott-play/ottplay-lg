//! Records is a projection of the existing addressed Guide, not another owner.
use super::*;
use plx_data::ott::records as core_records;

pub(super) fn is_records(query: Query) -> bool {
    matches!(query, Query::Records { .. })
}

pub(in super::super) fn available(runtime: &Runtime, profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
    query(runtime, profile, entry, (runtime.wall_clock_ms / 1000) as i64).is_ok()
}

pub(super) fn query(runtime: &Runtime, profile: &ProviderProfile, entry: &CatalogEntry, now: i64)
    -> Result<Query, &'static str>
{
    core_records::query(profile, entry, runtime.archive_fallback_hours(profile), now)
        .map_err(|_| "Records are unavailable for this channel")
}

pub(super) fn binding_current(runtime: &Runtime, state: &State) -> bool {
    match state.query {
        Query::Records { end, .. } => query(runtime, &state.profile, &state.entry, end)
            .is_ok_and(|query| query == state.query),
        _ => true,
    }
}

pub(super) fn projection(result: &ResultSet, order: OttGuideOrder) -> Result<Vec<usize>, &'static str> {
    if is_records(result.query) {
        core_records::indices(result).map_err(|_| "Records response is incomplete or unavailable")
    } else {
        Ok(order_indices(result, order))
    }
}

pub(super) fn programme_time(query: Query, programme: &Programme) -> String {
    let time = time_label(programme.start);
    if !is_records(query) { return time; }
    let date = i18n::local_calendar_date(programme.start)
        .and_then(|[year, month, day]| i18n::current().date(year, month as u8, day as u8))
        .unwrap_or_default();
    format!("{} · {}", date, time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme() -> Programme {
        Programme {
            channel: "fixture-channel".into(),
            start: 1_700_000_000,
            end: 1_700_003_600,
            title: "Programme".into(),
            description: "Description".into(),
        }
    }

    #[test]
    fn guide_labels_show_start_without_changing_the_programme_interval() {
        let _locale = i18n::language_on_this_thread_for_test(i18n::Preference::En);
        let original = programme();
        let mut longer = original.clone();
        longer.end += 86400;
        for query in [
            Query::CurrentNext { now: original.start },
            Query::Day { start: original.start - 3600, end: original.end + 3600 },
        ] {
            let label = programme_time(query, &original);
            assert!(!label.is_empty());
            assert_eq!(label, time_label(original.start));
            assert_eq!(label, programme_time(query, &longer));
            assert_eq!(original.end - original.start, 3600);
        }
    }

    #[test]
    fn records_keep_the_start_date_without_a_repeated_end_clock() {
        let _locale = i18n::language_on_this_thread_for_test(i18n::Preference::En);
        let original = programme();
        let query = Query::Records { start: original.start - 86400, end: original.end + 86400 };
        let label = programme_time(query, &original);
        let date = i18n::local_calendar_date(original.start)
            .and_then(|[year, month, day]| i18n::current().date(year, month as u8, day as u8))
            .expect("fixture has a valid local date");
        assert!(label.starts_with(&date));
        assert!(label.ends_with(&time_label(original.start)));
        let mut longer = original.clone();
        longer.end += 86400;
        assert_eq!(label, programme_time(query, &longer));
    }
}

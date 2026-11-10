//! Records is a projection of the existing addressed Guide, not another owner.
use super::*;
use plx_data::ott::records as core_records;

/// User-driven bounded pages over a fixed retention identity. Each provider
/// request owns at most one UTC day; no automatic network-draining loop exists.
#[derive(Clone)]
pub(super) struct Progress {
    start: i64,
    end: i64,
    cursor: i64,
    pub(super) pending: Option<Query>,
    pub(super) blocked: bool,
}
impl Progress {
    pub(super) fn new(query: Query) -> Option<Self> {
        let Query::Records { start,end } = query else { return None; };
        Some(Self { start,end,cursor:end,pending:None,blocked:false })
    }
    pub(super) fn next(&self) -> Option<Query> {
        if self.blocked || self.cursor <= self.start { return None; }
        Some(Query::Day { start:self.start.max(self.cursor.saturating_sub(86400)),end:self.cursor })
    }
    pub(super) fn incomplete(&self) -> bool { self.cursor > self.start }
    pub(super) fn merge(&mut self, previous: Option<&ResultSet>, result: &ResultSet) -> Result<ResultSet,&'static str> {
        let Some(Query::Day { start,end }) = self.pending else { return Err("Archive page is no longer pending"); };
        if result.query != self.pending.unwrap() || result.coverage != plx_data::ott::guide::Coverage::RequestedWindow
            || end != self.cursor { return Err("Archive page is incomplete or changed"); }
        let page_bytes:usize=result.programmes.iter().map(|row|row.channel.len()+row.title.len()+row.description.len()+64).sum();
        if result.programmes.len()>2048 || page_bytes>4*1024*1024 {
            self.blocked=true;return Err("Archive day exceeds programme limits");
        }
        let mut combined = previous.map(|r|r.programmes.clone()).unwrap_or_default();
        let mut identities: std::collections::BTreeMap<_,_> = combined.iter().enumerate()
            .map(|(index,row)|((row.channel.clone(),row.start,row.end),index)).collect();
        for row in &result.programmes {
            let identity=(row.channel.clone(),row.start,row.end);
            if let Some(index)=identities.get(&identity) {
                if combined[*index] != *row { return Err("Archive metadata changed during pagination"); }
            } else {
                identities.insert(identity,combined.len()); combined.push(row.clone());
            }
        }
        let mut merged=ResultSet { context:result.context.clone(),query:Query::Records {start,end:self.end},
            coverage:plx_data::ott::guide::Coverage::RequestedWindow,programmes:combined };
        // Records already owns latest-broadcast-per-title semantics. Compact to
        // that exact projection before retaining another page of descriptions.
        let indices=core_records::indices(&merged).map_err(|_|"Archive page exceeds catalogue limits")?;
        merged.programmes=indices.into_iter().map(|i|merged.programmes[i].clone()).collect();
        let bytes:usize=merged.programmes.iter().map(|row|row.channel.len()+row.title.len()+row.description.len()+64).sum();
        if merged.programmes.len()>20_000 || bytes>16*1024*1024 { self.blocked=true; return Err("Records capacity reached; use the day guide for older programmes"); }
        self.cursor=start; self.pending=None;
        Ok(merged)
    }
}

pub(super) fn lazy(profile:&ProviderProfile)->bool {
    matches!(profile.kind,ProviderKind::Xtream|ProviderKind::Stalker|ProviderKind::Edem|ProviderKind::Operator(_))
}

pub(super) fn is_records(query: Query) -> bool {
    matches!(query, Query::Records { .. })
}

pub(in super::super) fn available(runtime: &Runtime, profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
    query(runtime, profile, entry, (runtime.wall_clock_ms / 1000) as i64).is_ok()
}

pub(super) fn query(runtime: &Runtime, profile: &ProviderProfile, entry: &CatalogEntry, now: i64)
    -> Result<Query, &'static str>
{
    profile.validate().map_err(|_| "Invalid Records source profile")?;
    if lazy(profile) {
        let retention=entry.live.as_ref().and_then(|live|live.catchup.as_ref())
            .map(|source|source.retention_seconds).filter(|n|*n>0 && *n<=core_records::MAX_RETENTION_SECONDS)
            .ok_or("Records require advertised archive retention")?;
        let start=now.checked_sub(retention as i64).filter(|n|*n>=0).ok_or("Invalid archive clock")?;
        let interval=plx_data::ott::archive::ArchiveInterval {start:now.checked_sub(1).ok_or("Invalid archive clock")?,end:now};
        archive_play::Intent::new(profile,entry,interval,now)?;
        Ok(Query::Records {start,end:now})
    } else {
        core_records::query(profile, entry, runtime.archive_fallback_hours(profile), now)
            .map_err(|_| "Records are unavailable for this channel")
    }
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

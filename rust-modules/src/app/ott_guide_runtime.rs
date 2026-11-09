//! Addressed Guide UI over the existing metadata worker and playback admission.
//! A row is an intent for a retained programme, never an arbitrary media URL.
use super::*;
use super::ui_projection::row;
use plx_data::ott::{
    epg::Programme,
    guide::{Query, ResultSet},
    reminders::{Key, Record},
};
use plx_platform::i18n::{self, msg};
use plx_screens::ott::OttGuideOrder;

#[path = "ott_records_runtime.rs"]
mod records;
pub(super) use records::available as records_available;

#[derive(Clone)]
pub(super) struct State {
    source_generation: u64,
    request_generation: u64,
    profile: ProviderProfile,
    entry: CatalogEntry,
    selection_id: String,
    query: Query,
    records_progress: Option<records::Progress>,
    result: Option<ResultSet>,
    order: OttGuideOrder,
    programme_order: Vec<usize>,
    error: Option<&'static str>,
    programme: Option<String>,
    return_page: &'static str,
}

pub(super) fn is_live(profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
    match (&profile.kind, &entry.media) {
        (ProviderKind::M3u, MediaLocator::Http { .. }) => entry.duration_ms.is_none(),
        (ProviderKind::Edem | ProviderKind::Operator(_), MediaLocator::Http { .. }) => entry.live.is_some() && entry.provider_id_field.is_none(),
        (ProviderKind::Stalker, MediaLocator::StalkerApi { .. }) => true,
        (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. }) => stream_kind == "itv",
        (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. }) => stream_kind == "live",
        _ => false,
    }
}

fn supports_day(profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
    if matches!((&profile.kind, &entry.media), (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. }) if stream_kind == "live")
        || matches!((&profile.kind, &entry.media), (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. }) if stream_kind == "itv") { return true; }
    matches!(
        (&profile.kind, &entry.media),
        (ProviderKind::M3u | ProviderKind::Edem | ProviderKind::Operator(_), MediaLocator::Http { .. })
            | (ProviderKind::Stalker, MediaLocator::StalkerApi { .. })
    )
}

fn programme_id(programme: &Programme) -> String {
    // Length framing prevents ambiguous channel/time concatenations. The
    // provider's editable title is deliberately not programme identity.
    plx_data::ott::favorites_wire::hash(
        format!(
            "{}:{}:{}:{}",
            programme.channel.len(),
            programme.channel,
            programme.start,
            programme.end
        )
        .as_bytes(),
    )
}

/// Build the row projection only at publication/explicit input boundaries.
/// The accepted result stays in its original order, so stable time ties survive
/// any number of title/time round trips. Repeats are never deduplicated here.
fn order_indices(result: &ResultSet, order: OttGuideOrder) -> Vec<usize> {
    let mut indices: Vec<_> = (0..result.programmes.len()).collect();
    match order {
        OttGuideOrder::Chronological => indices.sort_by_key(|&i| result.programmes[i].start),
        // Classic uses JavaScript string comparison (UTF-16 code units), then
        // start time. Rust scalar/UTF-8 ordering differs for non-BMP titles.
        OttGuideOrder::Alphabetical => indices.sort_by(|&a, &b| {
            let a = &result.programmes[a];
            let b = &result.programmes[b];
            a.title.encode_utf16().cmp(b.title.encode_utf16())
                .then_with(|| a.start.cmp(&b.start))
        }),
    }
    indices
}

fn selected<'a>(state: &'a State, id: &str) -> Result<&'a Programme, &'static str> {
    fn unique<'a>(mut matches: impl Iterator<Item = &'a Programme>) -> Result<&'a Programme, &'static str> {
        let programme = matches.next().ok_or("Programme is no longer available")?;
        if matches.next().is_some() { return Err("Programme identity is ambiguous"); }
        Ok(programme)
    }
    let result = state.result.as_ref().ok_or("Programme is no longer available")?;
    if records::is_records(state.query) {
        // Overlapping feeds may repeat the same broadcast. Only the retained
        // projection owns a Records action; hidden duplicates have no authority.
        unique(state.programme_order.iter().filter_map(|&index| result.programmes.get(index))
            .filter(|programme| programme_id(programme) == id))
    } else {
        unique(result.programmes.iter().filter(|programme| programme_id(programme) == id))
    }
}

fn source_current(runtime: &Runtime, state: &State) -> bool {
    if runtime.generation != state.source_generation
        || !runtime.loaded
        || runtime.persistence_uncertain
        || !records::binding_current(runtime, state)
    {
        return false;
    }
    runtime
        .prepare_play_request(PlaybackRequest::ordinary(&state.selection_id, 0))
        .is_ok_and(|(_, profile, entry, _)| {
            profile == state.profile
                && entry.as_ref().is_none_or(|entry| {
                    entry.id == state.entry.id
                        && entry.media == state.entry.media
                        && entry.live == state.entry.live
                })
        })
}

fn checked(runtime: &Runtime, generation: u64) -> Result<&State, &'static str> {
    runtime
        .guide
        .as_ref()
        .filter(|state| state.request_generation == generation && source_current(runtime, state))
        .ok_or("Guide source changed; reopen the guide")
}

fn enqueue(runtime: &mut Runtime, mut state: State, query: Query) -> Result<(), &'static str> {
    // A fresh selection must not steal or clear metadata still owned by a
    // suspended source. The frame lifecycle owner performs rearm before input.
    if !runtime.metadata_work_allowed() {
        return Err("Guide is waiting for foreground or metadata retirement");
    }
    query.validate().map_err(|_| "Invalid guide interval")?;
    // All fallible admissions precede cancellation and publication replacement.
    // Explicit Local reads share this worker slot and may not be interrupted.
    if runtime.local_asset_work_ready().is_some() {
        return Err("Local metadata is still being published");
    }
    let generation = runtime
        .metadata_generation
        .checked_add(1)
        .ok_or("Guide request counter exhausted")?;
    let selection = if let Some(identity) = state.selection_id.strip_prefix("favorite:") {
        let favorite = runtime
            .saved
            .favorites
            .entries()
            .iter()
            .find(|favorite| favorite.identity() == identity)
            .cloned()
            .ok_or("Favorite no longer exists")?;
        let permit = runtime.favorite_permit(&state.profile, &favorite)?;
        MetadataSelection::Favorite(favorite, permit)
    } else {
        MetadataSelection::Entry(state.entry.clone())
    };
    let preserve=state.query==query && state.records_progress.is_some();
    state.query = query;
    if !preserve { state.records_progress=if records::lazy(&state.profile) {records::Progress::new(query)} else {None}; }
    let wire_query=if let Some(progress)=state.records_progress.as_mut() {
        let next=progress.next().ok_or("All archive days have been loaded")?;
        progress.pending=Some(next); next
    } else {query};
    if !preserve { state.result=None; state.programme_order.clear(); }
    runtime.metadata_cancel.store(true, Ordering::Release);
    runtime.metadata_cancel = Arc::new(AtomicBool::new(false));
    runtime.metadata_generation = generation;
    state.request_generation = generation;
    state.error = None;
    state.programme = None;
    runtime.metadata_target = Some((state.profile.id.clone(), state.selection_id.clone()));
    runtime.metadata_query = Some(wire_query);
    runtime.metadata_local_selection = None;
    runtime.metadata_playback_identity = runtime
        .playing
        .as_ref()
        .filter(|playing| playing.profile == state.profile && playing.entry.id == state.entry.id)
        .and_then(|playing| playing.identity.clone());
    runtime.metadata_refresh_ms = 0;
    runtime.metadata_next = Some((state.profile.clone(), selection));
    runtime.page = "guide";
    runtime.guide = Some(state);
    runtime.launch_metadata();
    if runtime.metadata_query.is_none() {
        if let Some(state) = runtime.guide.as_mut() {
            state.error = Some("Could not start guide request");
            if let Some(progress)=state.records_progress.as_mut(){progress.pending=None;}
        }
    }
    runtime.changed();
    Ok(())
}

fn open(runtime: &mut Runtime, id: &str) -> Result<(), &'static str> {
    open_mode(runtime, id, false)
}

fn open_mode(runtime: &mut Runtime, id: &str, records: bool) -> Result<(), &'static str> {
    let (_, profile, entry, _) = runtime.prepare_play_request(PlaybackRequest::ordinary(id, 0))?;
    let entry = entry
        .or_else(|| {
            runtime
                .favorite_preview
                .as_ref()
                .filter(|(selection, _)| selection == id)
                .map(|(_, entry)| entry.clone())
        })
        .or_else(|| {
            runtime
                .playing
                .as_ref()
                .filter(|playing| playing.selection_id == id && playing.profile == profile)
                .map(|playing| playing.entry.clone())
        })
        .ok_or("Select the channel and wait for its metadata")?;
    if !is_live(&profile, &entry) {
        return Err("This item has film metadata, not a programme guide");
    }
    let now = (runtime.wall_clock_ms / 1000) as i64;
    let query = if records {
        records::query(runtime, &profile, &entry, now)?
    } else if supports_day(&profile, &entry) {
        i18n::local_day_window(now, 0)
            .map(|(start, end)| Query::Day { start, end })
            .unwrap_or(Query::CurrentNext { now })
    } else {
        Query::CurrentNext { now }
    };
    let state = State {
        source_generation: runtime.generation,
        request_generation: 0,
        profile,
        entry,
        selection_id: id.into(),
        query,
        records_progress: None,
        result: None,
        order: OttGuideOrder::Chronological,
        programme_order: Vec::new(),
        error: None,
        programme: None,
        return_page: if runtime.page == "favorites" {
            "favorites"
        } else {
            "channels"
        },
    };
    enqueue(runtime, state, query)
}

/// The same receiver gate is used before retaining rows and before projecting
/// current/next metadata. A past-day reply never overwrites the playing footer.
pub(super) fn accept(runtime: &mut Runtime, event: &super::super::ott_metadata::Guide) -> bool {
    let is_day = !matches!(event.request.query, Query::CurrentNext { .. });
    let Some(state) = runtime.guide.as_ref() else {
        return is_day;
    };
    if !matches!(runtime.page, "guide" | "guide-programme")
        || !source_current(runtime, state)
        || state.request_generation != event.generation
        || state.records_progress.as_ref().and_then(|p|p.pending).unwrap_or(state.query) != event.request.query
        || !event.is_current(
            runtime.metadata_generation,
            runtime.metadata_target.as_ref(),
            runtime.metadata_query.as_ref(),
            runtime.metadata_cancel.load(Ordering::Acquire),
        )
    {
        return true;
    }
    let state = runtime.guide.as_mut().expect("validated guide");
    if state.entry.id != event.entry.id
        || state.entry.media != event.entry.media
        || state.entry.live != event.entry.live
    {
        state.result = None;
        state.programme_order.clear();
        state.error = Some("Channel source changed; reopen the guide");
        runtime.changed();
        return true;
    }
    match &event.result {
        Ok(result) => {
            let merged=if let Some(progress)=state.records_progress.as_mut() {
                progress.merge(state.result.as_ref(),result)
            } else { Ok(result.clone()) };
            let result=match merged { Ok(result)=>result,Err(error)=> {
                if let Some(progress)=state.records_progress.as_mut(){progress.pending=None;}
                state.error=Some(error); runtime.changed(); return is_day;
            }};
            match records::projection(&result, state.order) {
                Ok(indices) => {
                    state.programme_order = indices;
                    state.result = Some(result.clone());
                    state.error = None;
                }
                Err(error) => {
                    state.programme_order.clear();
                    state.result = None;
                    state.error = Some(error);
                }
            }
        }
        Err(error) => {
            if let Some(progress)=state.records_progress.as_mut() {progress.pending=None;}
            else {state.result=None;state.programme_order.clear();}
            state.error = Some(error);
        }
    }
    runtime.changed();
    is_day
}

/// Failed favorite resolution has no resolved entry, so the shared metadata
/// worker reports a Preview error. Consume its addressed failure here too.
pub(super) fn accept_failure(
    runtime: &mut Runtime,
    event: &super::super::ott_metadata::Preview,
) -> bool {
    let Err(error) = &event.result else {
        return false;
    };
    let Some(state) = runtime.guide.as_ref() else {
        return false;
    };
    if !source_current(runtime, state)
        || state.request_generation != event.generation
        || runtime.metadata_generation != event.generation
        || runtime.metadata_cancel.load(Ordering::Acquire)
        || runtime.metadata_query != Some(state.records_progress.as_ref().and_then(|p|p.pending).unwrap_or(state.query))
        || runtime.metadata_target.as_ref()
            != Some(&(event.profile_id.clone(), event.entry_id.clone()))
    {
        return false;
    }
    let state = runtime.guide.as_mut().expect("validated guide");
    if let Some(progress)=state.records_progress.as_mut(){progress.pending=None;}
    else {state.result=None;state.programme_order.clear();}
    state.error = Some(error);
    runtime.changed();
    true
}

pub(super) fn start_failed(runtime: &mut Runtime) {
    if let Some(state) = runtime
        .guide
        .as_mut()
        .filter(|state| state.request_generation == runtime.metadata_generation)
    {
        state.error = Some("Could not start guide request");
        if let Some(progress)=state.records_progress.as_mut(){progress.pending=None;}
        else {state.result=None;state.programme_order.clear();}
        runtime.changed();
    }
}

pub(super) fn close(runtime: &mut Runtime) {
    if let Some(state) = runtime.guide.take() {
        if runtime.metadata_generation == state.request_generation {
            runtime.metadata_cancel.store(true, Ordering::Release);
            runtime.metadata_target = None;
            runtime.metadata_query = None;
            runtime.metadata_next = None;
            runtime.metadata_refresh_ms = 0;
        }
        if matches!(runtime.page, "guide" | "guide-programme") {
            runtime.page = state.return_page;
        }
    }
}

pub(super) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    let result = match action {
        OttAction::OpenGuide(id) => open(runtime, id),
        OttAction::OpenRecords(id) => open_mode(runtime, id, true),
        OttAction::GuideDay {
            generation,
            day_start_s,
        } => (|| {
            let state = checked(runtime, *generation)?;
            if records::is_records(state.query) {
                let progress=state.records_progress.as_ref().ok_or("Records use the complete archive window")?;
                if progress.pending.is_some() {return Err("An archive page is already loading");}
                let Some(Query::Day {start,..})=progress.next() else {return Err("All archive days have been loaded");};
                if start!=*day_start_s {return Err("Archive page cursor changed");}
                let state=state.clone();let query=state.query;
                return enqueue(runtime,state,query);
            }
            if !supports_day(&state.profile, &state.entry) {
                return Err("This source only supplies a short guide");
            }
            let now = (runtime.wall_clock_ms / 1000) as i64;
            let (start, end) =
                i18n::local_day_window(*day_start_s, 0).ok_or("Guide day is unavailable")?;
            if start != *day_start_s || (start as i128 - now as i128).abs() > 32 * 86400 {
                return Err("Guide day is outside the supported window");
            }
            let state = runtime.guide.as_ref().expect("validated guide").clone();
            enqueue(runtime, state, Query::Day { start, end })
        })(),
        OttAction::GuideOrder { generation, order } => (|| {
            if runtime.page != "guide" {
                return Err("Reopen the guide list to change its order");
            }
            let state = checked(runtime, *generation)?;
            if records::is_records(state.query) {
                return Err("Records are ordered by title");
            }
            let state = runtime.guide.as_mut().expect("validated guide");
            if state.order != *order {
                state.order = *order;
                if let Some(result) = state.result.as_ref() {
                    state.programme_order = order_indices(result, *order);
                }
                runtime.changed();
            }
            Ok(())
        })(),
        OttAction::GuideProgramme {
            generation,
            programme_id,
        } => (|| {
            selected(checked(runtime, *generation)?, programme_id)?;
            runtime.guide.as_mut().expect("validated guide").programme = Some(programme_id.clone());
            runtime.page = "guide-programme";
            runtime.changed();
            Ok(())
        })(),
        OttAction::PlayArchive {
            generation,
            programme_id,
        } => (|| {
            if !playback_allowed(app) {
                return Err("Playback is suspended");
            }
            let state = checked(runtime, *generation)?;
            let programme = selected(state, programme_id)?;
            let interval = plx_data::ott::archive::ArchiveInterval {
                start: programme.start,
                end: programme.end,
            };
            let intent = runtime.archive_intent(
                &state.profile,
                &state.entry,
                interval,
                (runtime.wall_clock_ms / 1000) as i64,
            )?
            .with_programme(programme)?;
            let mut request = PlaybackRequest::ordinary(&state.selection_id, 0);
            request.advance_on_failure = false;
            request.archive = Some(intent);
            // This is the same bound parental/kiosk path as an ordinary UI Play.
            runtime.play_request_with_policy(app, request)?;
            close(runtime);
            Ok(())
        })(),
        OttAction::ReturnLive { generation } => (|| {
            if !playback_allowed(app) {
                return Err("Playback is suspended");
            }
            let state = checked(runtime, *generation)?;
            let mut request = PlaybackRequest::ordinary(&state.selection_id, 0);
            request.advance_on_failure = false;
            runtime.play_request_with_policy(app, request)?;
            close(runtime);
            Ok(())
        })(),
        OttAction::ToggleReminder {
            generation,
            programme_id,
        } => (|| {
            let state = checked(runtime, *generation)?;
            let programme = selected(state, programme_id)?;
            let source_id = runtime
                .reminder_source_id()
                .filter(|_| runtime.active() == Some(&state.profile))
                .ok_or("Reminders require the active saved source")?;
            let record = Record {
                key: Key {
                    source_id,
                    channel_id: state.entry.id.clone(),
                    programme_id: programme_id.clone(),
                    start_s: programme.start,
                },
                end_s: programme.end,
                title: programme.title.clone(),
            };
            runtime.toggle_reminder(record, state.source_generation)
        })(),
        OttAction::Back if runtime.page == "guide-programme" => {
            runtime.page = "guide";
            // Returning from details keeps the programme identity as the row
            // hint. A later sort must not silently select a different broadcast.
            runtime.changed();
            Ok(())
        }
        OttAction::Back if runtime.page == "guide" => {
            close(runtime);
            runtime.changed();
            Ok(())
        }
        OttAction::Refresh if matches!(runtime.page, "guide" | "guide-programme") => (|| {
            let state = runtime.guide.as_ref().ok_or("Guide is no longer open")?;
            if !source_current(runtime, state) {
                return Err("Guide source changed; reopen the guide");
            }
            let now = (runtime.wall_clock_ms / 1000) as i64;
            let query = if records::is_records(state.query) {
                records::query(runtime, &state.profile, &state.entry, now)?
            } else {
                refreshed_query(state.query, now)
            };
            let mut state = runtime.guide.as_ref().expect("validated guide").clone();
            state.records_progress=None;
            enqueue(runtime, state, query)
        })(),
        _ => {
            if !matches!(
                action,
                OttAction::Preview(_) | OttAction::ReminderAnswer { .. }
            ) {
                close(runtime);
            }
            return None;
        }
    };
    Some(result)
}

fn refreshed_query(query: Query, now: i64) -> Query {
    match query {
        Query::CurrentNext { .. } => Query::CurrentNext { now },
        day @ (Query::Day { .. } | Query::Records { .. }) => day,
    }
}

fn time_label(seconds: i64) -> String {
    i18n::local_clock_parts(seconds)
        .and_then(|parts| i18n::current().time(parts))
        .unwrap_or_default()
}

fn programme_row(runtime: &Runtime, state: &State, programme: &Programme, source_id: Option<&str>) -> OttRow {
    let id = programme_id(programme);
    // Only the committed book can mark a row. Pending saves and stale source
    // snapshots do not acquire presentation authority.
    let reminder = programme.end > (runtime.wall_clock_ms / 1000) as i64
        && source_id.is_some_and(|source_id| {
            runtime.reminder_contains(&Key {
                source_id: source_id.into(), channel_id: state.entry.id.clone(),
                programme_id: id.clone(), start_s: programme.start,
            })
        });
    let mut item = row(
        id.clone(),
        programme.title.clone(),
        records::programme_time(state.query, programme),
        OttAction::GuideProgramme {
            generation: state.request_generation,
            programme_id: id,
        },
        reminder,
    );
    item.description = programme.description.clone();
    item.poster = entry_poster(&state.entry);
    item
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    // placeholder-exempt: publishes typed pending state; OttScreen counts the visible status paint.
    if !matches!(runtime.page, "guide" | "guide-programme") {
        return None;
    }
    let state = runtime.guide.as_ref()?;
    let current = source_current(runtime, state);
    let reminder_source = current.then(|| runtime.reminder_source_id()).flatten();
    let mut rows = Vec::new();
    let generation = state.request_generation;
    if runtime.page == "guide-programme" {
        if let Some(programme) = state
            .programme
            .as_ref()
            .and_then(|id| selected(state, id).ok())
        {
            let id = programme_id(programme);
            rows.push(programme_row(runtime, state, programme, reminder_source.as_deref()));
            let now = (runtime.wall_clock_ms / 1000) as i64;
            let interval = plx_data::ott::archive::ArchiveInterval {
                start: programme.start,
                end: programme.end,
            };
            if runtime.archive_intent(&state.profile, &state.entry, interval, now).is_ok() {
                rows.push(row(
                    "guide:archive".into(),
                    msg::ott_guide_watch_archive().into(),
                    String::new(),
                    OttAction::PlayArchive {
                        generation,
                        programme_id: id.clone(),
                    },
                    false,
                ));
            }
            rows.push(row(
                "guide:live".into(),
                msg::ott_guide_watch_live().into(),
                String::new(),
                OttAction::ReturnLive { generation },
                false,
            ));
            if let Some(source_id) = runtime
                .reminder_source_id()
                .filter(|_| runtime.active() == Some(&state.profile))
            {
                let key = Key {
                    source_id,
                    channel_id: state.entry.id.clone(),
                    programme_id: id.clone(),
                    start_s: programme.start,
                };
                let saved = runtime.reminder_contains(&key);
                if programme.start > now || saved {
                    rows.push(row(
                        "guide:reminder".into(),
                        if saved {
                            msg::ott_reminder_remove()
                        } else {
                            msg::ott_reminder_add()
                        }
                        .into(),
                        String::new(),
                        OttAction::ToggleReminder {
                            generation,
                            programme_id: id,
                        },
                        saved,
                    ));
                }
            }
        }
    } else {
        if !records::is_records(state.query) {
            for (id, label, order) in [
                ("guide:order:chronological", msg::ott_guide_order_time(), OttGuideOrder::Chronological),
                ("guide:order:alphabetical", msg::ott_guide_order_title(), OttGuideOrder::Alphabetical),
            ] {
                rows.push(row(
                    id.into(),
                    label.into(),
                    if state.order == order { msg::ott_guide_order_selected().into() } else { String::new() },
                    OttAction::GuideOrder { generation, order },
                    false,
                ));
            }
            if supports_day(&state.profile, &state.entry) {
                let anchor = match state.query {
                    Query::Day { start, .. } => start,
                    Query::CurrentNext { now } => now,
                    Query::Records { end, .. } => end,
                };
                for (id, label, offset, stamp) in [
                    ("guide:previous", msg::ott_guide_previous_day(), -1, anchor),
                    (
                        "guide:today",
                        msg::ott_guide_today(),
                        0,
                        (runtime.wall_clock_ms / 1000) as i64,
                    ),
                    ("guide:next", msg::ott_guide_next_day(), 1, anchor),
                ] {
                    if let Some((start, _)) = i18n::local_day_window(stamp, offset) {
                        if (start as i128 - (runtime.wall_clock_ms / 1000) as i128).abs() <= 32 * 86400
                        {
                            rows.push(row(
                                id.into(),
                                label.into(),
                                String::new(),
                                OttAction::GuideDay {
                                    generation,
                                    day_start_s: start,
                                },
                                false,
                            ));
                        }
                    }
                }
            }
        }
        if let Some(result) = &state.result {
            rows.extend(
                state.programme_order.iter()
                    .map(|&i| programme_row(runtime, state, &result.programmes[i], reminder_source.as_deref())),
            );
        }
        if let Some(progress)=&state.records_progress {
            if progress.pending.is_none() {
                if let Some(Query::Day {start,..})=progress.next() {
                    rows.push(row("records:more".into(),msg::ott_records_more().into(),String::new(),
                        OttAction::GuideDay {generation,day_start_s:start},false));
                }
            }
        }
    }
    if !current {
        for row in &mut rows {
            row.enabled = false;
        }
    }
    let status = if !current {
        msg::ott_guide_source_changed().into()
    } else if let Some(error) = state.error {
        error.into()
    } else if state.result.is_none() {
        msg::ott_guide_loading().into()
    } else if state.records_progress.as_ref().is_some_and(|p|p.pending.is_some()) {
        msg::ott_guide_loading().into()
    } else if state.records_progress.as_ref().is_some_and(|p|p.incomplete()) {
        msg::ott_records_partial().into()
    } else if state.programme_order.is_empty() {
        if records::is_records(state.query) {
            msg::ott_records_empty().into()
        } else {
            msg::ott_guide_empty().into()
        }
    } else if state.records_progress.is_some() {
        String::new()
    } else if !supports_day(&state.profile, &state.entry) {
        msg::ott_guide_short().into()
    } else {
        runtime.status.clone()
    };
    let day = match state.query {
        Query::Day { start, .. } => i18n::local_calendar_date(start)
            .and_then(|[year, month, day]| i18n::current().date(year, month as u8, day as u8))
            .unwrap_or_default(),
        Query::CurrentNext { .. } => msg::ott_guide_current_next().into(),
        Query::Records { .. } => String::new(),
    };
    Some(OttView { pending_play: None, shelf_context: None, shelves: Vec::new(), status_working: current && state.error.is_none() && (state.result.is_none()
            || state.records_progress.as_ref().is_some_and(|progress|progress.pending.is_some())), classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: None, random_context: None,
        catalog_progress: None,
        search: None,
        page_id: format!("{}:{}", runtime.page, generation),
        revision: runtime.revision,
        title: if records::is_records(state.query) {
            msg::ott_records().into()
        } else {
            format!("{} · {}", msg::ott_guide(), day)
        },
        description: state.entry.title.clone(),
        status,
        selected_row: state.programme.clone().or_else(|| {
            state
                .result
                .as_ref()
                .and_then(|result| state.programme_order.first()
                    .map(|&i| programme_id(&result.programmes[i])))
        }),
        rows,
    })
}

#[cfg(test)]
#[path = "ott_guide_runtime_tests.rs"]
mod tests;

#[cfg(test)]
mod edem_guide_admission_tests {
    use super::*;
    #[test]
    fn edem_direct_portal_movie_without_duration_has_no_live_guide() {
        let profile=ProviderProfile{id:"edem-1".into(),name:"Edem".into(),kind:ProviderKind::Edem,source:r#"{"key":"test_key"}"#.into(),source_alias:String::new(),enabled:true};
        let template=plx_data::ott::edem::Config::parse(&profile.source).unwrap().template_profile(&profile);
        let mut row=plx_data::ott::parse_m3u("#EXTM3U\n#EXTINF:-1,Movie\nhttps://movies.invalid/video.mp4\n",&template).unwrap().entries.remove(0);
        assert!(is_live(&profile,&row));
        row.live=None;row.duration_ms=None;row.provider_id_field=Some("id".into());
        assert!(!is_live(&profile,&row));
    }
}

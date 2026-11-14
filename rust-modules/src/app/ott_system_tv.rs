//! System television transfers decoder ownership to LG, never inventing media URLs.
use super::ui_projection::row;
use super::*;
use crate::app::ott_remote::television::Pending;
#[derive(Clone, Deserialize)]
struct Channel {
    id: String,
    name: String,
    number: String,
    #[serde(default)]
    mode: String,
}
#[derive(Default)]
pub(super) struct State {
    channels: Vec<Channel>,
    generation: u64,
    connection: u64,
    flight: Option<(Operation, Pending)>,
    release: Option<Release>,
}
struct Release {
    id: String,
    deadline: u64,
    connection: u64,
}
#[derive(Clone)]
enum Operation {
    List,
    Select(String),
}
fn text(s: &str, n: usize) -> bool {
    s.len() <= n && !s.chars().any(char::is_control)
}
fn error(value: &Value) -> &'static str {
    match value["code"].as_str() {
        Some("channel_pairing_required") => "Pair the physical TV again with channel permissions",
        Some("not_paired" | "pairing_required") => {
            "Pair the physical TV with the command server first"
        }
        _ => "System TV request was not confirmed; inspect the TV before retrying",
    }
}
fn channels(value: &Value) -> Result<Vec<Channel>, &'static str> {
    if value["version"] != 1
        || value["action"] != "channels"
        || value["outcome"] != "confirmed"
        || value.get("code").is_some()
        || value["uncertain"] == true
    {
        return Err(error(value));
    }
    let rows: Vec<Channel> = match value.get("channels") {
        None | Some(Value::Null) => return Err("Invalid system TV catalogue"),
        Some(list) => {
            serde_json::from_value(list.clone()).map_err(|_| "Invalid system TV catalogue")?
        }
    };
    let mut ids = std::collections::HashSet::new();
    if rows.len() > 20000
        || rows.iter().any(|c| {
            c.id.trim().is_empty()
                || !text(&c.id, 512)
                || !text(&c.name, 512)
                || !text(&c.number, 64)
                || !text(&c.mode, 64)
                || !ids.insert(c.id.clone())
        })
    {
        return Err("Invalid system TV catalogue");
    }
    Ok(rows)
}
fn busy(state: &State) -> bool {
    state.flight.is_some() || state.release.is_some()
}
pub(super) fn entry(runtime: &Runtime) -> OttRow {
    let mut r = row(
        "system-tv".into(),
        "System television / LG Channels".into(),
        "Channels configured on this television".into(),
        OttAction::EditSetting("system.tv.open".into()),
        false,
    );
    r.enabled = runtime.loaded && !runtime.saving;
    r
}
pub(super) fn rows(runtime: &Runtime) -> Vec<OttRow> {
    let state = &runtime.system_tv;
    let mut result = vec![row(
        "system.tv.refresh".into(),
        "Refresh TV channels".into(),
        "System TV handles playback; OTTPlay does not control its kiosk".into(),
        OttAction::EditSetting("system.tv.refresh".into()),
        false,
    )];
    result[0].enabled = !busy(state);
    for (index, c) in state.channels.iter().enumerate() {
        let id = format!("system.tv.select:{}:{index}", state.generation);
        let mut r = row(
            id.clone(),
            format!("{}  {}", c.number, c.name),
            c.mode.clone(),
            OttAction::EditSetting(id),
            false,
        );
        r.enabled = !busy(state);
        result.push(r);
    }
    result
}
fn list(runtime: &mut Runtime) -> Result<(), &'static str> {
    if busy(&runtime.system_tv) {
        return Err("A system TV request is already in progress");
    }
    let task = runtime.remote.television_request("channels", None)?;
    runtime.system_tv.channels.clear();
    runtime.system_tv.generation = runtime.system_tv.generation.wrapping_add(1);
    runtime.system_tv.connection = runtime.remote.television_generation();
    runtime.system_tv.flight = Some((Operation::List, task));
    runtime.page = "system-tv";
    runtime.status = "Loading TV channels…".into();
    runtime.changed();
    Ok(())
}
pub(super) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    key: &str,
) -> Option<Result<(), &'static str>> {
    if !key.starts_with("system.tv.") {
        return None;
    }
    Some((|| {
        input_policy::replace_source(runtime)?;
        if matches!(key, "system.tv.open" | "system.tv.refresh") {
            return list(runtime);
        }
        if runtime.page != "system-tv" || busy(&runtime.system_tv) {
            return Err("System TV selection is no longer current");
        }
        let (generation, index) = key
            .strip_prefix("system.tv.select:")
            .and_then(|s| s.split_once(':'))
            .ok_or("Invalid system TV selection")?;
        let generation = generation
            .parse::<u64>()
            .map_err(|_| "Invalid system TV selection")?;
        let index = index
            .parse::<usize>()
            .map_err(|_| "Invalid system TV selection")?;
        if generation != runtime.system_tv.generation
            || runtime.system_tv.connection != runtime.remote.television_generation()
        {
            return Err("TV channels changed; refresh the list");
        }
        let id = runtime
            .system_tv
            .channels
            .get(index)
            .ok_or("TV channel is no longer available")?
            .id
            .clone();
        // All system channels are unclassified; require a channel grant before
        // leaving the application whose parental and kiosk rules we can enforce.
        runtime.require_parental(parental::Target::Channel { marked: true })?;
        if !runtime.remote.television_available() {
            return Err("Pair this player with a command server first");
        }
        runtime.stop_catalog(app);
        plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
        plx_media::route::cancel_play(&mut app.player.session);
        runtime.system_tv.release = Some(Release {
            id,
            deadline: runtime.now_ms().saturating_add(10000),
            connection: runtime.remote.television_generation(),
        });
        runtime.status = "Opening system television…".into();
        runtime.changed();
        Ok(())
    })())
}
pub(super) fn leave(runtime: &mut Runtime) {
    runtime.system_tv.flight = None;
    runtime.system_tv.release = None;
    runtime.status.clear();
}
fn select_status(value: &Value, id: &str) -> &'static str {
    if value["version"] != 1
        || value["action"] != "channel_select"
        || value.get("code").is_some()
        || value["uncertain"] == true
    {
        return error(value);
    }
    match value["outcome"].as_str() {
        Some("confirmed") if value["channel"]["id"].as_str() == Some(id) => {
            "System TV channel confirmed. Reopen OTTPlay to return."
        }
        Some("accepted") => {
            "System TV request submitted; tuning is not confirmed. Reopen OTTPlay to return."
        }
        _ => "System TV channel was not confirmed; inspect the TV before retrying",
    }
}
fn decoder_released(player: &plx_media::player::adapter::PlayerAdapter) -> bool {
    // A timed-out Load can outlive both the visible engine and the Unload machine.
    // System TV must wait for its native object to be reaped too.
    !player.release_busy() && !player.is_live() && !player.has_abandoned_load()
}
pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    if runtime.page != "system-tv" && busy(&runtime.system_tv) {
        leave(runtime);
        return;
    }
    let connection = runtime.remote.television_generation();
    if runtime.system_tv.connection != 0 && runtime.system_tv.connection != connection {
        leave(runtime);
        runtime.system_tv.channels.clear();
        runtime.system_tv.generation = runtime.system_tv.generation.wrapping_add(1);
        runtime.system_tv.connection = connection;
        if runtime.page == "system-tv" {
            runtime.status = "System TV connection changed; refresh the list".into();
            runtime.changed();
        }
    }
    if runtime.system_tv.release.is_some() {
        let allowed = runtime.page == "system-tv"
            && playback_allowed(app)
            && !app.pages.nav.suspended
            && current_ui_owner(app) == runtime.reply
            && input_policy::replace_source(runtime).is_ok()
            && runtime
                .require_parental(parental::Target::Channel { marked: true })
                .is_ok();
        let release = runtime.system_tv.release.as_ref().expect("pending handoff");
        if !allowed || release.connection != connection || runtime.now_ms() >= release.deadline {
            runtime.system_tv.release = None;
            runtime.status = "System TV handoff cancelled or decoder release timed out".into();
            runtime.changed();
        } else if decoder_released(&app.adapters.player) {
            let id = runtime
                .system_tv
                .release
                .take()
                .expect("pending handoff")
                .id;
            match runtime
                .remote
                .television_request("channel_select", Some(&id))
            {
                Ok(task) => runtime.system_tv.flight = Some((Operation::Select(id), task)),
                Err(message) => {
                    runtime.status = message.into();
                    runtime.changed();
                }
            }
        }
    }
    let Some((operation, task)) = runtime.system_tv.flight.as_ref() else {
        return;
    };
    let Some(result) = task.poll(&runtime.remote) else {
        return;
    };
    let operation = operation.clone();
    runtime.system_tv.flight = None;
    match (operation, result) {
        (Operation::List, Ok(value)) => match channels(&value) {
            Ok(rows) => {
                runtime.system_tv.channels = rows;
                runtime.system_tv.generation = runtime.system_tv.generation.wrapping_add(1);
                runtime.status = if runtime.system_tv.channels.is_empty() {
                    "No channels returned; check the TV channel scan and region".into()
                } else {
                    String::new()
                };
            }
            Err(message) => runtime.status = message.into(),
        },
        (Operation::Select(id), Ok(value)) => runtime.status = select_status(&value, &id).into(),
        (_, Err(message)) => runtime.status = message.into(),
    }
    runtime.changed();
}
#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    #[test]
    fn duplicate_titles_keep_distinct_channel_identity() {
        let rows = channels(
            &json!({"version":1,"action":"channels","outcome":"confirmed","channels":[
            {"id":"A","name":"News","number":"8"},{"id":"B","name":"News","number":"9"}]}),
        )
        .unwrap();
        assert_eq!(rows[1].id, "B");
        assert_eq!(rows[0].name, rows[1].name);
    }
    #[test]
    fn malformed_or_uncertain_catalogues_are_not_published() {
        for value in [
            json!({"version":1,"action":"channels","code":"channel_pairing_required"}),
            json!({"version":1,"action":"channels","outcome":"uncertain","channels":[]}),
            json!({"version":1,"action":"channels","outcome":"confirmed","code":"error","channels":[]}),
            json!({"version":1,"action":"channels","outcome":"confirmed"}),
            json!({"version":1,"action":"channels","outcome":"confirmed","channels":[
            {"id":"A","name":"N","number":"1"},{"id":"A","name":"M","number":"2"}]}),
        ] {
            assert!(channels(&value).is_err());
        }
        assert!(channels(
            &json!({"version":1,"action":"channels","outcome":"confirmed","channels":[]})
        )
        .unwrap()
        .is_empty());
    }
    #[test]
    fn selection_acceptance_is_not_confirmation_and_errors_win() {
        assert!(select_status(
            &json!({"version":1,"action":"channel_select","outcome":"accepted"}),
            "A"
        )
        .contains("not confirmed"));
        assert!(!select_status(&json!({"version":1,"action":"channel_select","outcome":"confirmed","channel":{"id":"B"}}),"A").contains("channel confirmed"));
        assert!(!select_status(&json!({"version":1,"action":"channel_select","outcome":"confirmed","channel":{"id":"A"},"uncertain":true}),"A").contains("channel confirmed"));
    }
    fn fixture() -> Runtime {
        let mut r = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        r.remote = Remote::disabled_for_test("system_tv_fixture", "fixture");
        r.loaded = true;
        r.page = "system-tv";
        r
    }
    #[test]
    fn abandoned_load_keeps_decoder_owned_until_the_worker_returns() {
        use plx_media::player::{engine, SHARED};
        use std::sync::{mpsc, atomic::Ordering};
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        SHARED.reset_session();
        SHARED.test_force_native_idle();
        let epoch = SHARED.begin_native_session().expect("native object ownership");
        let mut eng = engine::engine_after_reload_for_test();
        eng.native_epoch = epoch;
        let (release, held) = mpsc::channel::<()>();
        // The real teardown path parks this unfinished Load instead of joining it.
        // recv_timeout bounds a broken test without leaving a worker behind.
        eng.load_th = Some(std::thread::spawn(move || {
            let _ = held.recv_timeout(std::time::Duration::from_secs(5));
        }));
        app.adapters.player.install(eng);
        SHARED.load_timed_out.store(true, Ordering::Release);
        plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
        assert!(!app.adapters.player.release_busy());
        assert!(!app.adapters.player.is_live());
        assert!(app.adapters.player.has_abandoned_load(), "precondition: teardown parked the worker");
        let blocked = !decoder_released(&app.adapters.player);
        let not_reaped = !engine::reap_abandoned_load(&mut app.adapters.player);
        let mut runtime = fixture();
        runtime.system_tv.release = Some(Release {
            id: "A".into(),
            deadline: 0,
            connection: runtime.remote.television_generation(),
        });
        tick(&mut runtime, &mut app);
        assert!(runtime.system_tv.release.is_none());
        assert!(runtime.system_tv.flight.is_none(), "expired ownership wait cannot dispatch a tune");
        release.send(()).expect("release held worker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !engine::reap_abandoned_load(&mut app.adapters.player) {
            assert!(std::time::Instant::now() < deadline, "worker did not settle");
            std::thread::yield_now();
        }
        let settled = decoder_released(&app.adapters.player);
        SHARED.test_force_native_idle();
        SHARED.reset_session();
        assert!(blocked, "handoff must not dispatch while the abandoned Load owns the decoder");
        assert!(not_reaped, "an unfinished Load must retain ownership");
        assert!(settled, "handoff may proceed after the native release completes");
    }
    #[test]
    fn leaving_or_expired_handoff_never_late_tunes() {
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let mut r = fixture();
        for page in ["providers", "system-tv"] {
            r.page = page;
            r.system_tv.release = Some(Release {
                id: "A".into(),
                deadline: 0,
                connection: r.remote.television_generation(),
            });
            tick(&mut r, &mut app);
            assert!(r.system_tv.release.is_none());
            assert!(r.system_tv.flight.is_none());
            tick(&mut r, &mut app);
            assert!(r.system_tv.flight.is_none());
        }
    }
    #[test]
    fn old_catalogue_rows_and_connection_change_cannot_tune() {
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let mut r = fixture();
        r.system_tv.generation = 8;
        r.system_tv.connection = r.remote.television_generation();
        r.system_tv.channels = vec![Channel {
            id: "A".into(),
            name: "News".into(),
            number: "1".into(),
            mode: String::new(),
        }];
        assert!(action(&mut r, &mut app, "system.tv.select:7:0")
            .unwrap()
            .is_err());
        assert!(r.system_tv.release.is_none());
        assert!(r.system_tv.flight.is_none());
        r.system_tv.connection = r.remote.television_generation().wrapping_add(1);
        tick(&mut r, &mut app);
        assert!(r.system_tv.channels.is_empty());
    }
}

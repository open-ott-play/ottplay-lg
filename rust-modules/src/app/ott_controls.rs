//! Shared admission and catalogue view for native remote playback commands.
use super::*;
use plx_media::player::{adapter::PlayerAdapter, PlaybackState};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(super) struct Channel {
    pub id: String,
    pub name: String,
    pub number: usize,
}

pub(super) fn channels(runtime: &Runtime) -> Vec<Channel> {
    if runtime.view_selection().1 {
        runtime
            .saved
            .favorites
            .entries()
            .iter()
            .enumerate()
            .map(|(index, favorite)| Channel {
                id: format!("favorite:{}", favorite.identity()),
                name: favorite.title.clone(),
                number: index + 1,
            })
            .collect()
    } else {
        runtime.organization_rows(runtime.effective_category()).into_iter().map(|(index, entry)| Channel {
            id: entry.id.clone(), name: entry.title.clone(), number: index + 1,
        }).collect()
    }
}

pub(super) fn catalog_channels(entries: &[CatalogEntry], category: &str) -> Vec<Channel> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| category.is_empty() || entry.category == category)
        .map(|(index, entry)| Channel {
            id: entry.id.clone(),
            name: entry.title.clone(),
            number: index + 1,
        })
        .collect()
}

pub(super) fn resolve(rows: &[Channel], query: &str) -> Option<String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    if query.bytes().all(|byte| byte.is_ascii_digit()) {
        let number = query.parse::<usize>().ok()?;
        return rows
            .iter()
            .find(|row| row.number == number)
            .map(|row| row.id.clone());
    }
    let exact: Vec<_> = rows
        .iter()
        .filter(|row| row.name.to_lowercase() == query)
        .collect();
    if exact.len() == 1 {
        return Some(exact[0].id.clone());
    }
    if !exact.is_empty() {
        return None;
    }
    let mut matches = rows
        .iter()
        .filter(|row| row.name.to_lowercase().contains(&query));
    let first = matches.next()?;
    matches.next().is_none().then(|| first.id.clone())
}

pub(super) fn step(ids: &[String], selected: Option<&str>, offset: i64) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let length = ids.len() as i64;
    let current = selected.and_then(|id| ids.iter().position(|row| row == id));
    let base = current
        .map(|index| index as i64)
        .unwrap_or(if offset > 0 { -1 } else { 0 });
    Some(ids[(base + offset.rem_euclid(length)).rem_euclid(length) as usize].clone())
}

fn accepts_transport(foreground: bool, live: bool, state: PlaybackState) -> bool {
    foreground
        && live
        && matches!(
            state,
            PlaybackState::Playing
                | PlaybackState::Paused
                | PlaybackState::Buffering
                | PlaybackState::Seeking
        )
}

pub(super) fn transport_ready(app: &App) -> bool {
    accepts_transport(
        super::super::playback_allowed(app) && matches!(app.route(), AppArg::Player),
        app.adapters.player.is_live(),
        plx_media::player::state(&app.player.session),
    )
}

pub(super) fn set_paused(adapter: &mut PlayerAdapter, desired: bool) -> bool {
    // A presentation hold must not undo an explicit user Pause.
    lifecycle::set_transport_paused(adapter, desired)
}

fn volume_level(params: &Value, current: u8) -> Option<u8> {
    let absolute = params.get("volume");
    let value = absolute.or_else(|| params.get("volume_step"))?.as_f64()?;
    if !value.is_finite() {
        return None;
    }
    let target = if absolute.is_some() {
        value
    } else {
        f64::from(current) + value
    };
    Some(target.clamp(0.0, 100.0).round() as u8)
}

pub(super) fn volume_command(app: &mut App, inv: &Invocation) -> Value {
    if !transport_ready(app) {
        return rejected("No active foreground playback");
    }
    let mut params = inv.params.clone();
    if !inv.expects_response() {
        if let Some(params) = params.as_object_mut() {
            for name in ["id", "created_at", "expires_at"] {
                params.remove(name);
            }
        }
    }
    if !subset(&params, &["command", "volume", "volume_step"]) {
        return rejected("Volume accepts an absolute level or a relative step");
    }
    let Some(level) = volume_level(&params, app.player.volume.desired().level()) else {
        return rejected("Use a finite volume level or step");
    };
    let desired = plx_media::player::volume::DesiredVolume::new(i32::from(level), false).unwrap();
    let dispatched = plx_media::player::volume::set_volume(
        &mut app.adapters.player,
        &mut app.player.volume,
        desired,
    );
    // This firmware has no reliable volume getter. Keep requested state distinct
    // from measured output, including when the native setter defers or refuses it.
    let mut result = json!({"status":if dispatched {"ok"} else {"rejected"},"data":{"dispatched":dispatched,"volume":null,"desired_volume":level,"muted":false}});
    if !dispatched {
        result["data"]["error"] =
            json!("The volume is retained but native application is not confirmed");
    }
    result
}

pub(super) fn restartable(runtime: &Runtime, app: &App) -> bool {
    let session = &app.player.session;
    let state = plx_media::player::state(session);
    let external = plx_media::route::external_source(session);
    let retryable = plx_media::route::can_retry_current_play(session);
    // A failed first resolve can retain a request without ever installing a
    // source. A failed plan may also carry its rating key but no server. Neither
    // case is an incoming selection competing with an older installed source.
    let failed_initial_resolve = state == PlaybackState::Error
        && retryable
        && external.is_none()
        && !plx_media::route::has_url(session)
        && !app.adapters.player.is_live()
        && plx_media::route::cur_sid(session) == plx_plex::plex::ServerId::UNSET
        && plx_media::route::playback_request_identity(session).is_some_and(|(_, requested)| {
            let installed = plx_media::route::cur_rk(session);
            installed.is_empty() || installed == requested
        });
    super::super::playback_allowed(app)
        && (runtime.play_target.is_none()
            || runtime.restore_play_generation == Some(runtime.play_generation))
        && (!super::super::retained_request_replaces_installed(app) || failed_initial_resolve)
        && !plx_media::route::play_pending()
        && state != PlaybackState::Idle
        && (external.is_some() || retryable)
}

pub(super) struct RestartedStream {
    duration: i64,
    position: i64,
    paused: bool,
}

/// Typed native outcome shared by UI and command projection.
pub(super) fn restart_stream(runtime: &mut Runtime, app: &mut App) -> Result<RestartedStream, &'static str> {
    if !restartable(runtime, app) {
        return Err("There is no owned, restartable stream");
    }
    let duration = playback::dur();
    let position = if duration > 0 {
        playback::intended_pos(&app.player.session)
            .max(plx_media::route::unpresented_resume_ns(&app.player.session))
            .max(0)
    } else {
        0
    };
    let paused = lifecycle::viewer_paused();
    // Reload the actual route. The last selected catalogue row may have failed
    // to open, and is not proof of ownership of the stream currently playing.
    let accepted = plx_media::player::engine::restart_current(
        &mut app.player.session,
        &mut app.adapters.player,
        app.bridge.metadata_mut(),
    );
    if !accepted {
        return Err("The native decoder refused stream restart");
    }
    // Only an accepted restart commits the actual source. Keep the restore
    // generation until this shared hook fences it and reanchors continuity.
    runtime.viewer_transport(app);
    supersede_deferred(runtime);
    app.bridge.start_store_jobs();
    Ok(RestartedStream { duration, position, paused })
}

pub(super) fn restart(runtime: &mut Runtime, app: &mut App, lifecycle: bool) -> Value {
    let RestartedStream { duration, position, paused } = match restart_stream(runtime, app) {
        Ok(value) => value,
        Err(reason) => return rejected(reason),
    };
    if lifecycle {
        ok(json!({"accepted":true,"dispatched":true,"operation":"restart_stream"}))
    } else {
        ok(
            json!({"accepted":true,"dispatched":true,"target":"stream","kind":if duration>0 {"vod"} else {"live"},"paused":paused,"position":position as f64/1e9}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtering_a_category_preserves_the_numbers_drawn_by_the_native_list() {
        let entries: Vec<_> = (1..=7)
            .map(|number| CatalogEntry { protection: None,
                synopsis: None,
                id: number.to_string(),
                title: format!("Film {number}"),
                category: if number == 3 || number == 7 {
                    "Selected"
                } else {
                    "Other"
                }
                .into(),
                description: String::new(),
                poster: String::new(),
                media: MediaLocator::Local {
                    relative_path: format!("{number}.mp4"),
                },
                duration_ms: None,
                provider_id: None,
                provider_id_field: None,
                live: None,
            })
            .collect();
        let rows = catalog_channels(&entries, "Selected");
        assert_eq!(
            rows.iter().map(|row| row.number).collect::<Vec<_>>(),
            [3, 7]
        );
        assert_eq!(resolve(&rows, "7"), Some("7".into()));
        assert_eq!(resolve(&rows, "2"), None);
    }
    #[test]
    fn volume_clamps_steps_and_prefers_the_explicit_absolute_level() {
        assert_eq!(volume_level(&json!({"volume_step":5}), 30), Some(35));
        assert_eq!(volume_level(&json!({"volume_step":-100}), 30), Some(0));
        assert_eq!(volume_level(&json!({"volume":200}), 30), Some(100));
        assert_eq!(
            volume_level(&json!({"volume":20,"volume_step":50}), 30),
            Some(20)
        );
        assert_eq!(volume_level(&json!({"volume":"20"}), 30), None);
        assert_eq!(volume_level(&json!({}), 30), None);
    }
    #[test]
    fn visible_numbers_and_unique_names_resolve_the_same_rows() {
        let rows = vec![
            Channel {
                id: "favorite:first".into(),
                name: "Film".into(),
                number: 3,
            },
            Channel {
                id: "favorite:second".into(),
                name: "Film sequel".into(),
                number: 7,
            },
        ];
        assert_eq!(resolve(&rows, " 3 "), Some("favorite:first".into()));
        assert_eq!(resolve(&rows, "7"), Some("favorite:second".into()));
        assert_eq!(resolve(&rows, "1"), None);
        assert_eq!(resolve(&rows, "film"), Some("favorite:first".into()));
        assert_eq!(resolve(&rows, "SEQUEL"), Some("favorite:second".into()));
        assert_eq!(resolve(&rows, "fil"), None);
        assert_eq!(resolve(&rows, "0"), None);
        assert_eq!(resolve(&rows, ""), None);
    }
    #[test]
    fn stepping_handles_no_selection_wrap_and_large_signed_offsets() {
        let ids = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(step(&ids, None, 1), Some("a".into()));
        assert_eq!(step(&ids, None, -1), Some("c".into()));
        assert_eq!(step(&ids, Some("c"), 1), Some("a".into()));
        assert_eq!(step(&ids, Some("a"), -1), Some("c".into()));
        assert!(step(&ids, None, i64::MAX).is_some());
        assert!(step(&ids, None, i64::MIN).is_some());
        assert_eq!(step(&[], None, 1), None);
    }
    #[test]
    fn a_retained_player_page_does_not_authorize_parked_or_failed_transport() {
        assert!(accepts_transport(true, true, PlaybackState::Paused));
        assert!(accepts_transport(true, true, PlaybackState::Buffering));
        assert!(!accepts_transport(false, true, PlaybackState::Playing));
        assert!(!accepts_transport(true, false, PlaybackState::Playing));
        for state in [
            PlaybackState::Idle,
            PlaybackState::Resolving,
            PlaybackState::Error,
        ] {
            assert!(!accepts_transport(true, true, state));
        }
    }
    #[test]
    fn explicit_pause_revokes_an_internal_holds_resume_even_if_already_paused() {
        let _guard = plx_base::testlock::serial();
        use plx_media::player::{claim_hold, transport};
        let was_paused = transport::paused();
        let mut adapter = PlayerAdapter::new(unsafe { plx_base::task::MainThread::assume() });
        transport::set_paused(true);
        claim_hold::hold_for_test(7, true);
        assert!(set_paused(&mut adapter, true));
        let restore = claim_hold::take(7);
        transport::set_paused(was_paused);
        assert!(
            !restore,
            "the completed internal hold must honor the viewer's pause"
        );
    }
}

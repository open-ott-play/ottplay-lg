//! Active-slot transport never enters the fullscreen route or global Player TX.
use super::*;
use plx_appkit::player_hud::command::HudCommand;
use plx_screens::registry::PlayerReq;

fn target(runtime: &Runtime, driver: &Driver, offset_us: i64) -> Result<i64, &'static str> {
    let (position, duration, _) = driver
        .active_timeline(runtime)
        .ok_or("No active window picture")?;
    Ok(position
        .saturating_add(offset_us)
        .clamp(0, duration.unwrap_or(24 * 60 * 60 * 1_000_000)))
}
impl Driver {
    pub fn seek_relative(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        offset_us: i64,
    ) -> Result<i64, &'static str> {
        let position = target(runtime, self, offset_us)?;
        self.seek_active(runtime, app, position)?;
        Ok(position)
    }
}
pub(in crate::app::ott_runtime) fn player_request(app: &mut App, request: &PlayerReq) -> bool {
    let recognized = matches!(
        request,
        PlayerReq::Exit
            | PlayerReq::Transport(_)
            | PlayerReq::SeekTo(_)
            | PlayerReq::CommitSeek(_)
            | PlayerReq::RetryPlayback
            | PlayerReq::PlayAutomatically
            | PlayerReq::StandIn
    ) || matches!(
        request,
        PlayerReq::Command(
            HudCommand::TogglePlay
                | HudCommand::Seek(_)
                | HudCommand::Previous
                | HudCommand::Next
                | HudCommand::FromBeginning
        )
    );
    if !recognized
        || !app
            .ott
            .as_ref()
            .is_some_and(|r| super::super::local_multiview_enabled(r) && r.multiview_local.active())
    {
        return false;
    }
    let Some(mut runtime) = app.ott.take() else {
        return false;
    };
    if matches!(request, PlayerReq::Exit) {
        super::super::player_browse::present(&mut runtime, app, None, "menu");
        super::super::multiview_local_menu::action(
            &mut runtime,
            app,
            &OttAction::EditSetting("localmv.open".into()),
        );
        app.ott = Some(runtime);
        return true;
    }
    let mut driver = std::mem::take(&mut runtime.multiview_local);
    let result = match request {
        PlayerReq::Transport(play) => {
            let paused = play.map(|play| !play).or_else(|| {
                driver
                    .active_timeline(&runtime)
                    .map(|(_, _, paused)| !paused)
            });
            paused
                .ok_or("No active window picture")
                .and_then(|paused| driver.set_active_paused(&mut runtime, app, paused))
        }
        PlayerReq::SeekTo(ns) => driver
            .seek_active(&mut runtime, app, ns / 1000)
            .and_then(|()| driver.set_active_paused(&mut runtime, app, false)),
        PlayerReq::CommitSeek(ns) => driver.seek_active(&mut runtime, app, ns / 1000),
        PlayerReq::Command(command) => match command {
            HudCommand::TogglePlay => driver
                .active_timeline(&runtime)
                .map(|(_, _, paused)| !paused)
                .ok_or("No active window picture")
                .and_then(|paused| driver.set_active_paused(&mut runtime, app, paused)),
            HudCommand::Seek(by) => driver
                .seek_relative(
                    &mut runtime,
                    app,
                    by.step_ns(plx_appkit::player_hud::scrub_step_ns()) / 1000,
                )
                .map(|_| ()),
            HudCommand::Previous | HudCommand::Next => driver
                .step_active(
                    &mut runtime,
                    app,
                    if *command == HudCommand::Next { 1 } else { -1 },
                )
                .map_err(|_| "Window source changed before continuation"),
            HudCommand::FromBeginning => driver.seek_active(&mut runtime, app, 0),
            _ => Err("Window transport is unavailable"),
        },
        _ => Err("Automatic fullscreen playback cannot replace a multiview window"),
    };
    runtime.multiview_local = driver;
    if result.is_ok() {
        super::super::multiview_local_present::flush_audio(&mut runtime, app);
    } else if let Err(error) = result {
        runtime.status = error.into();
        runtime.changed();
    }
    app.ott = Some(runtime);
    true
}

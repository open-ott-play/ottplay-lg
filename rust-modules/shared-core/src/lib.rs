//! VM and type conversion only. Playback rules are the canonical compiled core.
//! No native library, network, filesystem, module loader or host callback is
//! installed in the VM. Callers retain admission and execution of returned plans.

pub mod operators;

use rquickjs::{Context, Function, Object, Runtime};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

const CORE: &str = include_str!("../../../vendor/ottplay-core.js");

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackKind {
    Live,
    Archive,
    Vod,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeekTarget {
    pub source_id: String,
    pub channel_id: String,
    pub kind: PlaybackKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_start: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeekIntent {
    Offset,
    Absolute,
    Begin,
    Restart,
    GoLive,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Overshoot {
    #[default]
    Noop,
    Clamp,
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeekRequest {
    pub intent: SeekIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<f64>,
    #[serde(default)]
    pub archive_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_earliest: Option<f64>,
    #[serde(default)]
    pub live_edge_tolerance: f64,
    #[serde(default = "enabled")]
    pub restart_allowed: bool,
    #[serde(default)]
    pub overshoot: Overshoot,
}

impl SeekRequest {
    pub fn finite(intent: SeekIntent, value: f64, position: f64, duration: f64) -> Self {
        Self {
            intent,
            value: Some(value),
            position: Some(position),
            duration: Some(duration),
            now: None,
            archive_available: false,
            archive_earliest: None,
            live_edge_tolerance: 0.0,
            restart_allowed: true,
            overshoot: Overshoot::Clamp,
        }
    }

    fn representable(&self) -> bool {
        [
            self.value,
            self.position,
            self.duration,
            self.now,
            self.archive_earliest,
        ]
        .into_iter()
        .flatten()
        .all(f64::is_finite)
            && self.live_edge_tolerance.is_finite()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SeekPlan {
    Seek {
        position: f64,
    },
    OpenArchive {
        #[serde(rename = "archiveStart")]
        archive_start: f64,
    },
    GoLive,
    Restart,
    Noop,
}

/// Stable errors deliberately exclude VM exception text and source identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    RuntimeUnavailable,
    InvalidInput,
    InvalidResult,
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::RuntimeUnavailable => "Shared playback runtime is unavailable",
            Self::InvalidInput => "Shared playback request is invalid",
            Self::InvalidResult => "Shared playback result is invalid",
        })
    }
}
impl std::error::Error for Error {}

pub struct Planner {
    context: Context,
}

impl Planner {
    pub fn new() -> Result<Self, Error> {
        let runtime = Runtime::new().map_err(|_| Error::RuntimeUnavailable)?;
        runtime.set_max_stack_size(1024 * 1024);
        runtime.set_memory_limit(32 * 1024 * 1024);
        let context = Context::full(&runtime).map_err(|_| Error::RuntimeUnavailable)?;
        context.with(|ctx| {
            ctx.eval::<(), _>(CORE).map_err(|_| {
                let _ = ctx.catch();
                Error::RuntimeUnavailable
            })
        })?;
        Ok(Self { context })
    }

    pub fn seek_plan(
        &self,
        target: Option<&SeekTarget>,
        request: &SeekRequest,
    ) -> Result<SeekPlan, Error> {
        // JSON cannot express these values. Reject instead of serde's null coercion.
        if !request.representable()
            || target
                .and_then(|value| value.archive_start)
                .is_some_and(|value| !value.is_finite())
        {
            return Err(Error::InvalidInput);
        }
        let target = serde_json::to_vec(&target).map_err(|_| Error::InvalidInput)?;
        let request = serde_json::to_vec(request).map_err(|_| Error::InvalidInput)?;
        let result = self.context.with(|ctx| -> Result<String, Error> {
            let call = || -> rquickjs::Result<String> {
                let core: Object = ctx.globals().get("OttPlayCore")?;
                let method: Function = core.get("playbackSeekPlan")?;
                let result: Object =
                    method.call((ctx.json_parse(target)?, ctx.json_parse(request)?))?;
                ctx.json_stringify(result)?
                    .ok_or(rquickjs::Error::Unknown)?
                    .to_string()
            };
            call().map_err(|_| {
                let _ = ctx.catch();
                Error::InvalidInput
            })
        })?;
        let plan: SeekPlan = serde_json::from_str(&result).map_err(|_| Error::InvalidResult)?;
        match &plan {
            SeekPlan::Seek { position } if !position.is_finite() || *position < 0.0 => {
                Err(Error::InvalidResult)
            }
            SeekPlan::OpenArchive { archive_start }
                if !archive_start.is_finite() || *archive_start < 0.0 =>
            {
                Err(Error::InvalidResult)
            }
            _ => Ok(plan),
        }
    }
}

// Each caller thread retains only its isolated VM. No application state or
// provider data is retained; the core function receives the complete observation.
thread_local! { static PLANNER: RefCell<Option<Planner>> = const { RefCell::new(None) }; }

pub fn seek_plan(target: Option<&SeekTarget>, request: &SeekRequest) -> Result<SeekPlan, Error> {
    PLANNER.with(|slot| {
        let mut planner = slot
            .try_borrow_mut()
            .map_err(|_| Error::RuntimeUnavailable)?;
        if planner.is_none() {
            *planner = Some(Planner::new()?);
        }
        planner
            .as_ref()
            .ok_or(Error::RuntimeUnavailable)?
            .seek_plan(target, request)
    })
}

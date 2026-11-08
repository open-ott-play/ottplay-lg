//! One passive, process-local failure observation after native admission.
//! Private playback ownership never enters the live command reply.
//! Details are captured with the cause, never sampled from a later playback during a read.
use plx_media::{player::{report::{FailureDetails, IoFailureDetails}, FailureKind}, route::PlaybackIdentity};
use plx_telemetry::telemetry::classes::{DemuxExitClass, TraceOutcome};
use serde::Serialize;

// No Debug or Serialize: ownership is internal, not remote metadata.
struct Owner {
    generation: u64,
    identity: PlaybackIdentity,
}

#[derive(Default)]
pub(super) struct State {
    owner: Option<Owner>,
    failure: Option<Observation>,
}

struct Observation {
    kind: FailureKind,
    details: Option<FailureDetails>,
}

#[derive(Clone, Copy)]
pub(super) enum Version { V2, V3 }

#[derive(Serialize)]
pub(super) struct Snapshot {
    version: u8,
    play_generation: u64,
    code: &'static str,
    context: Option<ContextSnapshot>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ContextSnapshot {
    V2 { demux_exit: &'static str, pipeline_error: i32 },
    V3 { demux_exit: &'static str, pipeline_error: i32, io_failure: Option<IoFailureSnapshot> },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum IoFailureSnapshot {
    SourceIo { transport: &'static str },
    Hls { exit: &'static str, outcome: Option<&'static str> },
    AutoFallback { outcome: &'static str },
    AudioDecode { outcome: &'static str },
    Unclassified { outcome: &'static str },
}

impl From<IoFailureDetails> for IoFailureSnapshot {
    fn from(details: IoFailureDetails) -> Self {
        match details {
            IoFailureDetails::SourceIo { transport } => Self::SourceIo { transport: transport.code() },
            IoFailureDetails::Hls { exit, outcome } => Self::Hls {
                exit: match exit {
                    DemuxExitClass::HlsFailed | DemuxExitClass::HlsNotReady | DemuxExitClass::Other
                    | DemuxExitClass::HlsPlaylistFetchFailed | DemuxExitClass::HlsPlaylistInvalid
                    | DemuxExitClass::HlsPlaylistWindow | DemuxExitClass::HlsTransportOpenFailed
                    | DemuxExitClass::HlsSegmentProbeFailed | DemuxExitClass::HlsSegmentCodec
                    | DemuxExitClass::HlsVideoFraming | DemuxExitClass::HlsFirstIdr
                    | DemuxExitClass::HlsShortBody | DemuxExitClass::HlsAudioIdentity
                    | DemuxExitClass::HlsAudioEmpty | DemuxExitClass::HlsVideoTimestamp
                    | DemuxExitClass::HlsAudioTimestamp | DemuxExitClass::HlsAudioFraming
                    | DemuxExitClass::HlsNoVideoAu | DemuxExitClass::HlsSizeLimit
                    | DemuxExitClass::HlsTransportFailed => exit.code(),
                    _ => DemuxExitClass::Other.code(),
                },
                outcome: outcome.filter(|value| *value == TraceOutcome::Deadline).map(TraceOutcome::code),
            },
            IoFailureDetails::AutoFallback => Self::AutoFallback { outcome: "refused" },
            IoFailureDetails::AudioDecode => Self::AudioDecode { outcome: "inconclusive" },
            IoFailureDetails::Unclassified => Self::Unclassified { outcome: "inconclusive" },
        }
    }
}

impl State {
    /// A newly accepted request cannot inherit the outgoing attempt's evidence.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn admitted(&mut self, generation: u64, identity: Option<PlaybackIdentity>) {
        self.clear();
        self.owner = identity.map(|identity| Owner { generation, identity });
    }

    /// Missing identity after our own cleanup is not a new playback owner.
    pub(super) fn reconcile_owner(&mut self, identity: Option<&PlaybackIdentity>) {
        if self.owner.as_ref().zip(identity)
            .is_some_and(|(owner, current)| owner.identity != *current)
        {
            self.clear();
        }
    }

    /// Called only after the existing owned-Error guard, before it tears down.
    /// Preserve the first observation; a later counter is not this owner's generation.
    pub(super) fn capture(&mut self, identity: Option<&PlaybackIdentity>, kind: FailureKind,
        details: Option<FailureDetails>) {
        if self.failure.is_none() && self.owner.as_ref().zip(identity)
            .is_some_and(|(owner, current)| owner.identity == *current)
        {
            self.failure = Some(Observation { kind, details });
        }
    }

    pub(super) fn snapshot(&self) -> Option<Snapshot> {
        self.snapshot_version(Version::V2)
    }

    pub(super) fn snapshot_version(&self, version: Version) -> Option<Snapshot> {
        let owner = self.owner.as_ref()?;
        let failure = self.failure.as_ref()?;
        Some(Snapshot {
            version: match version { Version::V2 => 2, Version::V3 => 3 },
            play_generation: owner.generation,
            code: failure.kind.code(),
            context: failure.details.map(|details| match version {
                Version::V2 => ContextSnapshot::V2 {
                    demux_exit: details.demux_exit.code(),
                    pipeline_error: details.pipeline_error,
                },
                Version::V3 => ContextSnapshot::V3 {
                    demux_exit: details.demux_exit.code(),
                    pipeline_error: details.pipeline_error,
                    io_failure: details.io_failure.map(IoFailureSnapshot::from),
                },
            }),
        })
    }
}

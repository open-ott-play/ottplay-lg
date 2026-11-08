//! Process-local, source-bound fullscreen picture controls. A submission is
//! accepted renderer intent, never proof of displayed pixels or a portable setting.
use plx_machine::machine::Addr;
use plx_media::player::{
    video_geometry::Transform,
    viewport::{ContentState, Failure, Source},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerGeometryOffer {
    pub source: Source,
    pub state: ContentState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerGeometryRequest {
    pub reply: Addr,
    pub offer: PlayerGeometryOffer,
    pub transform: Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerGeometryFailure {
    Inactive,
    StaleOffer,
    Media(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerGeometryOutcome {
    Submitted(ContentState),
    Rejected(PlayerGeometryFailure),
}

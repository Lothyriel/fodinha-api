mod actor;
mod manager;
mod message;
mod projection;
mod registry;

pub(crate) use actor::MatchActor;
pub use manager::{ManagerHandle, PlayerConnectionContext};
pub(crate) use message::{
    MatchActorMessage, MatchReceiver, MatchSender, OutboundMessage, PlayerReceiver, PlayerSender,
};
pub(crate) use projection::project_match_metadata;
pub(crate) use registry::{MatchEntries, MatchRegistry, PlayerRoutes, SenderLookup};

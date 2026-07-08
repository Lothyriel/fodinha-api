mod actor;
mod manager;
mod message;
mod projection;
mod registry;

pub(crate) use actor::{MatchActor, MatchActorResources};
pub(crate) use manager::ManagerResources;
pub use manager::{ManagerHandle, PlayerConnectionContext};
pub use message::WAITING_LOBBY_INACTIVITY_CLOSE_CODE;
pub(crate) use message::{
    MatchActorMessage, MatchReceiver, MatchSender, OutboundMessage, PlayerReceiver, PlayerSender,
    WAITING_LOBBY_INACTIVITY_CLOSE_REASON,
};
pub(crate) use projection::project_match_metadata;
pub(crate) use registry::{
    MatchActorContext, MatchEntries, MatchRegistry, PlayerRoutes, SenderLookup,
};

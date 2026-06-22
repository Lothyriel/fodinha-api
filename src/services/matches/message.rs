use tokio::sync::{mpsc, oneshot};

use crate::{
    infra::UserClaims,
    models::{
        commands::{GameCommand, GetLobbyDto, LobbyInfo},
        game::GameSettings,
        id::PlayerId,
    },
    services::ManagerError,
};

use crate::models::commands::ServerMessage;

pub type MatchSender = flume::Sender<MatchActorMessage>;
pub type MatchReceiver = flume::Receiver<MatchActorMessage>;
pub type PlayerSender = mpsc::Sender<ServerMessage>;
pub type PlayerReceiver = mpsc::Receiver<ServerMessage>;

pub enum MatchActorMessage {
    ConnectPlayer {
        player_id: PlayerId,
        outbound_tx: PlayerSender,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    DisconnectPlayer {
        player_id: PlayerId,
        outbound_tx: PlayerSender,
    },
    CreateMatch {
        settings: GameSettings,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    JoinLobby {
        user_claims: UserClaims,
        respond: oneshot::Sender<Result<LobbyInfo, ManagerError>>,
    },
    StatusChange {
        player_id: PlayerId,
        ready: bool,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    GameCommand {
        player_id: PlayerId,
        command: GameCommand,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    GetLobbySummary {
        respond: oneshot::Sender<Result<Option<GetLobbyDto>, ManagerError>>,
    },
}

use tokio::sync::{mpsc, oneshot};

use std::collections::HashMap;

use crate::{
    models::{
        Card, Turn,
        commands::{GameCommand, GetLobbyDto},
        game::GameSettings,
        id::PlayerId,
        lobby::{LobbyInfoInternal, MatchSnapshotInternal},
    },
    services::ManagerError,
};

pub type MatchSender = flume::Sender<MatchActorMessage>;
pub type MatchReceiver = flume::Receiver<MatchActorMessage>;
pub type PlayerSender = mpsc::Sender<OutboundMessage>;
pub type PlayerReceiver = mpsc::Receiver<OutboundMessage>;

type PlayerPoints = HashMap<PlayerId, usize>;

#[derive(Clone, Debug)]
pub enum OutboundMessage {
    PlayerTurn {
        player_id: PlayerId,
    },
    TurnPlayed {
        pile: Vec<Turn>,
    },
    PlayerBidded {
        player_id: PlayerId,
        bid: usize,
    },
    PlayerBiddingTurn {
        player_id: PlayerId,
        possible_bids: Vec<usize>,
    },
    PlayerStatusChange {
        player_id: PlayerId,
        ready: bool,
    },
    RoundEnded(PlayerPoints),
    PlayerDeck(Vec<Card>),
    SetStart {
        upcard: Card,
    },
    SetEnded {
        lifes: PlayerPoints,
    },
    GameEnded {
        lifes: PlayerPoints,
    },
    PlayerJoined(PlayerId),
    PlayerLeft {
        player_id: PlayerId,
    },
    Snapshot(MatchSnapshotInternal),
}

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
        creator_id: PlayerId,
        settings: GameSettings,
        respond: oneshot::Sender<Result<(), ManagerError>>,
    },
    JoinLobby {
        player_id: PlayerId,
        respond: oneshot::Sender<Result<LobbyInfoInternal, ManagerError>>,
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

impl MatchActorMessage {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::ConnectPlayer { .. } => "connect_player",
            Self::DisconnectPlayer { .. } => "disconnect_player",
            Self::CreateMatch { .. } => "create_match",
            Self::JoinLobby { .. } => "join_lobby",
            Self::StatusChange { .. } => "status_change",
            Self::GameCommand { command, .. } => match command {
                GameCommand::PlayTurn { .. } => "game.play_turn",
                GameCommand::PutBid { .. } => "game.put_bid",
            },
            Self::GetLobbySummary { .. } => "get_lobby_summary",
        }
    }
}

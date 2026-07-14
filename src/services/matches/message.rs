use tokio::sync::{mpsc, oneshot};

use std::collections::HashMap;

use crate::{
    models::{
        Card, Turn,
        commands::GetLobbyDto,
        game::{GameCommand, GameSettings},
        id::{MercenaryId, PlayerId},
        lobby::{LobbyInfoInternal, MatchSnapshotInternal},
    },
    services::{ManagerError, PlayerManaDto, PowerCardDto},
};

pub type MatchSender = flume::Sender<MatchActorMessage>;
pub type MatchReceiver = flume::Receiver<MatchActorMessage>;
pub type PlayerSender = mpsc::Sender<OutboundMessage>;
pub type PlayerReceiver = mpsc::Receiver<OutboundMessage>;

pub const WAITING_LOBBY_INACTIVITY_CLOSE_CODE: u16 = 4001;
pub const WAITING_LOBBY_INACTIVITY_CLOSE_REASON: &str = "waiting_lobby_inactive";

type PlayerPoints = HashMap<PlayerId, usize>;
type PlayerMana = HashMap<PlayerId, PlayerManaDto>;

#[derive(Clone, Debug)]
pub enum OutboundMessage {
    Public(OutboundPayload),
    SinglePlayer(OutboundPayload),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundAudience {
    Public,
    SinglePlayer,
}

impl OutboundMessage {
    pub fn audience(&self) -> OutboundAudience {
        match self {
            Self::Public(_) => OutboundAudience::Public,
            Self::SinglePlayer(_) => OutboundAudience::SinglePlayer,
        }
    }

    pub fn payload(&self) -> &OutboundPayload {
        match self {
            Self::Public(payload) | Self::SinglePlayer(payload) => payload,
        }
    }
}

#[derive(Clone, Debug)]
pub enum OutboundPayload {
    Common(CommonOutboundMessage),
    Fodinha(FodinhaOutboundMessage),
    Power(PowerOutboundMessage),
}

#[derive(Clone, Debug)]
pub enum CommonOutboundMessage {
    Close {
        code: u16,
        reason: String,
    },
    PlayerStatusChange {
        player_id: PlayerId,
        ready: bool,
    },
    PlayerMercenarySelected {
        player_id: PlayerId,
        mercenary_id: MercenaryId,
    },
    PlayerJoined(PlayerId),
    PlayerLeft {
        player_id: PlayerId,
    },
    Snapshot(MatchSnapshotInternal),
}

#[derive(Clone, Debug)]
pub enum FodinhaOutboundMessage {
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
    PlayersLifesChanged(PlayerPoints),
    PlayerBiddingTurn {
        player_id: PlayerId,
        possible_bids: Vec<usize>,
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
}

#[derive(Clone, Debug)]
pub enum PowerOutboundMessage {
    PlayersManaChanged(PlayerMana),
    PlayerPowerTurn {
        player_id: PlayerId,
        phase: crate::services::PowerPhaseDto,
    },
    DeckRevealed {
        target_player_id: PlayerId,
        cards: Vec<Card>,
    },
    PlayerPowerCards(Vec<PowerCardDto>),
    PowerCardPlayed {
        player_id: PlayerId,
        card: PowerCardDto,
        targets: Vec<PlayerId>,
        lifes: PlayerPoints,
    },
}

impl From<CommonOutboundMessage> for OutboundPayload {
    fn from(message: CommonOutboundMessage) -> Self {
        Self::Common(message)
    }
}

impl From<FodinhaOutboundMessage> for OutboundPayload {
    fn from(message: FodinhaOutboundMessage) -> Self {
        Self::Fodinha(message)
    }
}

impl From<PowerOutboundMessage> for OutboundPayload {
    fn from(message: PowerOutboundMessage) -> Self {
        Self::Power(message)
    }
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
        shutting_down: bool,
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
    SelectMercenary {
        player_id: PlayerId,
        mercenary_id: MercenaryId,
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
            Self::SelectMercenary { .. } => "select_mercenary",
            Self::GameCommand { command, .. } => command.kind(),
            Self::GetLobbySummary { .. } => "get_lobby_summary",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn audience_is_independent_from_game_payload() {
        let payload = OutboundPayload::Fodinha(FodinhaOutboundMessage::PlayerTurn {
            player_id: PlayerId(Arc::from("player-1")),
        });
        let public = OutboundMessage::Public(payload.clone());
        let private = OutboundMessage::SinglePlayer(payload);

        assert_eq!(public.audience(), OutboundAudience::Public);
        assert_eq!(private.audience(), OutboundAudience::SinglePlayer);
        assert!(matches!(
            public.payload(),
            OutboundPayload::Fodinha(FodinhaOutboundMessage::PlayerTurn { .. })
        ));
        assert!(matches!(
            private.payload(),
            OutboundPayload::Fodinha(FodinhaOutboundMessage::PlayerTurn { .. })
        ));
    }

    #[test]
    fn power_payload_cannot_be_constructed_as_classic_placeholder() {
        let payload =
            OutboundPayload::Power(PowerOutboundMessage::PlayersManaChanged(HashMap::new()));

        assert!(matches!(payload, OutboundPayload::Power(_)));
    }
}

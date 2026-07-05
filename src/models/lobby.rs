use std::collections::HashMap;

use indexmap::{IndexMap, map::Entry};

use crate::{
    models::{GameError, LobbyState, game::GameSettings, game::GameType, id::PlayerId},
    services::{GameInfoDto, LobbyError},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LobbyPlayerStatus {
    pub ready: bool,
}

impl LobbyPlayerStatus {
    fn new() -> Self {
        Self { ready: false }
    }
}

pub struct Lobby {
    pub players: IndexMap<PlayerId, LobbyPlayerStatus>,
    pub state: LobbyState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LobbyInfoInternal {
    NotStarted(HashMap<PlayerId, LobbyPlayerStatus>),
    Playing(GameInfoDto),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchSnapshotInternal {
    Waiting(HashMap<PlayerId, LobbyPlayerStatus>),
    Playing(PlayingMatchSnapshotInternal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayingMatchSnapshotInternal {
    pub players: HashMap<PlayerId, LobbyPlayerStatus>,
    pub game: GameInfoDto,
}

impl Lobby {
    pub fn new(settings: GameSettings) -> Self {
        Self {
            players: IndexMap::new(),
            state: LobbyState::NotStarted(settings),
        }
    }

    pub fn game_type(&self) -> GameType {
        match &self.state {
            LobbyState::NotStarted(settings) => settings.game_type(),
            LobbyState::Playing(game) => game.game_type(),
        }
    }

    pub fn get_players_id(&self) -> Vec<PlayerId> {
        self.players.keys().cloned().collect()
    }

    pub fn get_players(&self) -> Vec<LobbyPlayerStatus> {
        self.players.values().cloned().collect()
    }

    pub fn join(&mut self, player_id: PlayerId) -> Result<(), LobbyError> {
        let max_players = match &self.state {
            LobbyState::NotStarted(s) => Ok(s.max_players()),
            LobbyState::Playing(_) => Err(LobbyError::GameAlreadyStarted),
        };

        let player_count = self.players.len();

        match self.players.entry(player_id) {
            Entry::Occupied(_) => Ok(()),
            Entry::Vacant(e) => {
                if player_count == max_players? {
                    return Err(LobbyError::GameError(GameError::TooManyPlayers));
                }
                e.insert(LobbyPlayerStatus::new());
                Ok(())
            }
        }
    }

    pub fn get_info(&self, player_id: &PlayerId) -> LobbyInfoInternal {
        match &self.state {
            LobbyState::NotStarted(_) => {
                let players = self
                    .players
                    .iter()
                    .map(|(id, p)| (id.clone(), p.clone()))
                    .collect();

                LobbyInfoInternal::NotStarted(players)
            }
            LobbyState::Playing(game) => LobbyInfoInternal::Playing(game.get_game_info(player_id)),
        }
    }

    pub fn get_snapshot(&self, player_id: &PlayerId) -> MatchSnapshotInternal {
        match &self.state {
            LobbyState::NotStarted(_) => {
                let players = self
                    .players
                    .iter()
                    .map(|(id, p)| (id.clone(), p.clone()))
                    .collect();

                MatchSnapshotInternal::Waiting(players)
            }
            LobbyState::Playing(game) => {
                let players = self
                    .players
                    .iter()
                    .map(|(id, p)| (id.clone(), p.clone()))
                    .collect();

                MatchSnapshotInternal::Playing(PlayingMatchSnapshotInternal {
                    players,
                    game: game.get_game_info(player_id),
                })
            }
        }
    }
}

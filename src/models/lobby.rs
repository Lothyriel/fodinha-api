use indexmap::{IndexMap, map::Entry};

use crate::{
    infra::UserClaims,
    models::{GameError, LobbyState, commands::LobbyInfo, game::GameSettings, id::PlayerId},
    services::LobbyError,
};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PlayerStatus {
    pub ready: bool,
    pub player: UserClaims,
}

impl PlayerStatus {
    fn new(claims: UserClaims) -> Self {
        Self {
            ready: false,
            player: claims,
        }
    }
}

pub struct Lobby {
    pub players: IndexMap<PlayerId, PlayerStatus>,
    pub state: LobbyState,
}

impl Lobby {
    pub fn new(settings: GameSettings) -> Self {
        Self {
            players: IndexMap::new(),
            state: LobbyState::NotStarted(settings),
        }
    }

    pub fn get_players_id(&self) -> Vec<PlayerId> {
        self.players.keys().cloned().collect()
    }

    pub fn get_players(&self) -> Vec<PlayerStatus> {
        self.players.values().cloned().collect()
    }

    pub fn join(&mut self, user_claims: UserClaims) -> Result<(), LobbyError> {
        let max_players = match &self.state {
            LobbyState::NotStarted(s) => Ok(s.max_players),
            LobbyState::Playing(_) => Err(LobbyError::GameAlreadyStarted),
        };

        let player_count = self.players.len();

        match self.players.entry(user_claims.id()) {
            Entry::Occupied(_) => Ok(()),
            Entry::Vacant(e) => {
                if player_count == max_players? {
                    return Err(LobbyError::GameError(GameError::TooManyPlayers).into());
                }
                e.insert(PlayerStatus::new(user_claims));
                Ok(())
            }
        }
    }

    pub fn get_info(&self, player_id: &PlayerId) -> LobbyInfo {
        match &self.state {
            LobbyState::NotStarted(_) => {
                let players = self
                    .players
                    .iter()
                    .map(|(id, p)| (id.clone(), p.clone()))
                    .collect();

                LobbyInfo::NotStarted(players)
            }
            LobbyState::Playing(game) => LobbyInfo::Playing(game.get_game_info(player_id)),
        }
    }
}

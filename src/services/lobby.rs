use std::collections::HashMap;

use tokio::sync::oneshot;

use crate::{
    infra::UserClaims,
    models::{
        LobbyState,
        commands::{GetLobbyDto, LobbyInfo},
        game::GameSettings,
        id::{LobbyId, PlayerId},
        lobby::Lobby,
    },
    services::{LobbyError, ManagerError},
};

pub struct LobbiesManager {
    lobbies: HashMap<LobbyId, Lobby>,
    players_lobby: HashMap<PlayerId, LobbyId>,
}

impl LobbiesManager {
    pub fn new() -> Self {
        Self {
            lobbies: HashMap::new(),
            players_lobby: HashMap::new(),
        }
    }

    pub fn create_lobby(&mut self, lobby_id: LobbyId, settings: GameSettings) {
        self.lobbies.insert(lobby_id, Lobby::new(settings));
    }

    pub fn get_lobbies_info(&self) -> Vec<GetLobbyDto> {
        self.lobbies
            .iter()
            .filter(|(_, lobby)| matches!(lobby.state, LobbyState::NotStarted(_)))
            .map(|(id, lobby)| GetLobbyDto {
                id: id.clone(),
                player_count: lobby.players.len(),
            })
            .collect()
    }

    pub fn join_lobby(
        &mut self,
        lobby_id: &LobbyId,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        let lobby = self
            .lobbies
            .get_mut(lobby_id)
            .ok_or(LobbyError::InvalidLobby)?;

        let player_id = user_claims.id();

        lobby.join(user_claims)?;

        let info = lobby.get_info(&player_id);

        self.players_lobby.insert(player_id, lobby_id.clone());

        Ok(info)
    }

    pub fn get_players(&self, lobby_id: &LobbyId) -> Option<Vec<&PlayerId>> {
        self.lobbies
            .get(lobby_id)
            .map(|l| l.players.keys().collect())
    }

    pub fn get(&self, id: PlayerId) -> Result<&Lobby, LobbyError> {
        self.players_lobby
            .get(&id)
            .and_then(|l| self.lobbies.get(l))
            .ok_or(LobbyError::PlayerNotInLobby)
    }
}

use tokio::sync::oneshot;

use crate::{
    infra::UserClaims,
    models::{
        commands::*,
        game::GameSettings,
        id::{self, LobbyId, PlayerId},
    },
    services::{InboundSender, ManagerError, repositories::game::GamesRepository},
};

#[derive(Clone)]
pub struct ManagerHandle {
    dispather: InboundSender,
    pub games_repo: GamesRepository,
}

impl ManagerHandle {
    pub fn new(games: GamesRepository, tx: InboundSender) -> Self {
        Self {
            dispather: tx,
            games_repo: games,
        }
    }

    pub async fn create_lobby(
        &self,
        _player_id: PlayerId,
        settings: GameSettings,
    ) -> CreateLobbyResponse {
        let lobby_id = id::gen_lobbyid();

        let msg = LobbyCommand::CreateLobby(lobby_id.clone(), settings);
        self.send(msg.into()).await;

        CreateLobbyResponse { lobby_id }
    }

    pub async fn join_lobby(
        &self,
        lobby_id: LobbyId,
        user_claims: UserClaims,
    ) -> Result<LobbyInfo, ManagerError> {
        let (tx, rx) = oneshot::channel();

        let msg = LobbyCommand::JoinLobby {
            lobby_id,
            user_claims,
            respond: tx,
        };

        self.send(msg.into()).await;

        let result = rx.await.expect("gameloop sender should never drop");

        Ok(result?)
    }

    pub async fn get_lobbies(&self) -> Vec<GetLobbyDto> {
        let (tx, rx) = oneshot::channel();

        let msg = LobbyCommand::GetLobbies(tx);

        self.send(msg.into()).await;

        rx.await.expect("gameloop sender should never drop")
    }

    pub async fn send_error(&self, id: &PlayerId, error: ManagerError) {
        let msg = ServerMessage::Error {
            msg: error.to_string(),
        };

        self.unicast_msg(id, &msg).await;
    }

    async fn send(&self, msg: ClientMessage) {
        self.dispather
            .send_async(msg)
            .await
            .expect("Game loop consumer dropped")
    }
}

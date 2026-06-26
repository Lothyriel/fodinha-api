use std::time::Duration;

use crate::{
    AppSettings,
    services::{
        matches::ManagerHandle,
        repositories::{
            get_mongo_client, matches::MatchesRepository, stats::StatsRepository,
            users::UsersRepository,
        },
        stats::StatsProjector,
    },
};

pub struct GameManager;

const WAITING_LOBBY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

impl GameManager {
    pub async fn start(settings: &AppSettings) -> ManagerHandle {
        Self::start_with_waiting_lobby_timeout(settings, WAITING_LOBBY_TIMEOUT).await
    }

    pub(crate) async fn start_with_waiting_lobby_timeout(
        settings: &AppSettings,
        waiting_lobby_timeout: Duration,
    ) -> ManagerHandle {
        let database = match settings.mongo_database.is_empty() {
            true => "oh_hell",
            false => settings.mongo_database.as_str(),
        };

        let db = get_mongo_client(&settings.mongo_conn_string)
            .await
            .expect("Expected to create mongo client")
            .database(database);
        let matches_repo = MatchesRepository::new(&db);
        let stats_repo = StatsRepository::new(&db);
        let users_repo = UsersRepository::new(&db);

        if let Err(e) = matches_repo.ensure_indexes().await {
            tracing::error!("Error creating match indexes: {e}");
        }

        if let Err(e) = stats_repo.ensure_indexes().await {
            tracing::error!("Error creating stats indexes: {e}");
        }

        if let Err(e) = users_repo.ensure_indexes().await {
            tracing::error!("Error creating users indexes: {e}");
        }

        let stats_projector = StatsProjector::start(matches_repo.clone(), stats_repo.clone());

        ManagerHandle::new(
            matches_repo,
            stats_repo,
            users_repo,
            stats_projector,
            waiting_lobby_timeout,
        )
    }
}

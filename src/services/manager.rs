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

const WAITING_LOBBY_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const EMPTY_PLAYING_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ABANDONED_MATCH_SCAN_INTERVAL: Duration = Duration::from_secs(60);

impl GameManager {
    pub async fn start(settings: &AppSettings) -> ManagerHandle {
        Self::start_with_timeouts(
            settings,
            WAITING_LOBBY_TIMEOUT,
            EMPTY_PLAYING_TIMEOUT,
            ABANDONED_MATCH_SCAN_INTERVAL,
        )
        .await
    }

    pub(crate) async fn start_with_timeouts(
        settings: &AppSettings,
        waiting_lobby_timeout: Duration,
        empty_playing_timeout: Duration,
        abandoned_match_scan_interval: Duration,
    ) -> ManagerHandle {
        let database = match settings.mongo_database.is_empty() {
            true => "oh_hell",
            false => settings.mongo_database.as_str(),
        };

        let db = get_mongo_client(&settings.mongo_conn_string, settings.mongo_max_pool_size)
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

        let manager = ManagerHandle::new(
            matches_repo,
            stats_repo,
            users_repo,
            stats_projector,
            waiting_lobby_timeout,
            empty_playing_timeout,
        );

        manager.start_abandoned_match_janitor(empty_playing_timeout, abandoned_match_scan_interval);

        manager
    }
}

use crate::{
    AppSettings,
    services::{
        matches::ManagerHandle,
        repositories::{get_mongo_client, matches::MatchesRepository, stats::StatsRepository},
        stats::StatsProjector,
    },
};

pub struct GameManager;

impl GameManager {
    pub async fn start(settings: &AppSettings) -> ManagerHandle {
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

        if let Err(e) = stats_repo.ensure_indexes().await {
            tracing::error!("Error creating stats indexes: {e}");
        }

        let stats_projector = StatsProjector::start(matches_repo.clone(), stats_repo.clone());

        ManagerHandle::new(matches_repo, stats_repo, stats_projector)
    }
}

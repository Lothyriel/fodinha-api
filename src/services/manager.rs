use std::time::Duration;

use crate::{
    AppSettings,
    models::game::fodinha_power,
    services::{
        card_definitions::CardDefinitionsService,
        matches::{ManagerHandle, ManagerResources},
        mercenaries::MercenariesService,
        object_storage::ObjectStorage,
        repositories::{
            card_decks::CardDecksRepository, card_definitions::CardDefinitionsRepository,
            get_mongo_client, matches::MatchesRepository, mercenaries::MercenariesRepository,
            stats::StatsRepository, users::UsersRepository,
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
        let card_definitions_repo = CardDefinitionsRepository::new(&db);
        let card_decks_repo = CardDecksRepository::new(&db);
        let mercenaries_repo = MercenariesRepository::new(&db);
        let stats_repo = StatsRepository::new(&db);
        let users_repo = UsersRepository::new(&db);
        let object_storage = ObjectStorage::new(settings);
        let power_card_registry = fodinha_power::PowerCardRegistryStore::default();
        let card_definitions = CardDefinitionsService::new(
            card_definitions_repo.clone(),
            card_decks_repo.clone(),
            mercenaries_repo.clone(),
            object_storage.clone(),
            users_repo.clone(),
            power_card_registry.clone(),
        );
        let mercenaries = MercenariesService::new(
            mercenaries_repo.clone(),
            object_storage,
            users_repo.clone(),
            power_card_registry.clone(),
        );

        if let Err(e) = matches_repo.ensure_indexes().await {
            tracing::error!("Error creating match indexes: {e}");
        }

        if let Err(e) = stats_repo.ensure_indexes().await {
            tracing::error!("Error creating stats indexes: {e}");
        }

        if let Err(e) = users_repo.ensure_indexes().await {
            tracing::error!("Error creating users indexes: {e}");
        }

        if let Err(e) = card_definitions_repo.ensure_indexes().await {
            tracing::error!("Error creating card definition indexes: {e}");
        }

        if let Err(e) = card_decks_repo.ensure_indexes().await {
            tracing::error!("Error creating power deck indexes: {e}");
        }

        if let Err(e) = mercenaries_repo.ensure_indexes().await {
            tracing::error!("Error creating mercenary indexes: {e}");
        }

        match mercenaries.load_mercenary_registry().await {
            Ok(count) => tracing::info!("Loaded {count} FodinhaPower mercenary definitions"),
            Err(e) => tracing::warn!("Could not load FodinhaPower mercenary definitions: {e}"),
        }

        match card_definitions.load_power_card_registry().await {
            Ok(count) => tracing::info!("Loaded {count} FodinhaPower card definitions"),
            Err(e) => tracing::warn!("Could not load FodinhaPower card definitions: {e}"),
        }

        let stats_projector = StatsProjector::start(matches_repo.clone(), stats_repo.clone());

        let manager = ManagerHandle::new(ManagerResources {
            repo: matches_repo,
            stats_repo,
            users_repo,
            card_definitions,
            mercenaries,
            stats_projector,
            power_card_registry,
            waiting_lobby_timeout,
            empty_playing_timeout,
        });

        manager.start_abandoned_match_janitor(empty_playing_timeout, abandoned_match_scan_interval);

        manager
    }
}

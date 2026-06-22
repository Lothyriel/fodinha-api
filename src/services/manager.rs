use crate::{
    AppSettings,
    services::{
        matches::ManagerHandle,
        repositories::{get_mongo_client, matches::MatchesRepository},
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

        ManagerHandle::new(MatchesRepository::new(&db))
    }
}

use crate::{
    AppSettings,
    services::{
        dispatcher::ManagerHandle,
        repositories::{game::GamesRepository, get_mongo_client},
    },
};

pub struct GameManager;

impl GameManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn start(self, settings: &AppSettings) -> ManagerHandle {
        let db = get_mongo_client(&settings.mongo_conn_string)
            .await
            .expect("Expected to create mongo client")
            .database("oh_hell");

        ManagerHandle::new(GamesRepository::new(&db))
    }
}

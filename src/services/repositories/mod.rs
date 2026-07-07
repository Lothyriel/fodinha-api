use mongodb::{Client, error::Result, options::ClientOptions};

pub mod card_decks;
pub mod card_definitions;
pub mod matches;
pub mod mercenaries;
pub mod stats;
pub mod users;

pub async fn get_mongo_client(conn_string: &str, max_pool_size: u32) -> Result<Client> {
    let mut options = ClientOptions::parse(conn_string).await?;

    options.max_pool_size = Some(max_pool_size);
    options.min_pool_size = Some(2);

    Client::with_options(options)
}

use mongodb::{Client, error::Result, options::ClientOptions};

pub mod game;

pub async fn get_mongo_client(conn_string: &str) -> Result<Client> {
    let options = ClientOptions::parse(conn_string).await?;

    Client::with_options(options)
}

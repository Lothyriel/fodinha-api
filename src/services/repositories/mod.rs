use mongodb::{Client, error::Result, options::ClientOptions};

pub mod game;

pub async fn get_mongo_client() -> Result<Client> {
    let connection_string = std::env::var("MONGO_CONNECTION_STRING")
        .unwrap_or_else(|_| "mongodb://localhost/?retryWrites=true".to_string());

    let options = ClientOptions::parse(connection_string).await?;

    Client::with_options(options)
}

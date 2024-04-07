use torrent::{Client, ClientConfig, Torrent};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let codecrafters = Torrent::from_file("codecrafters.torrent").unwrap();
    let handle = client.download(".", codecrafters).await.unwrap();
    handle.await.unwrap().unwrap();
}

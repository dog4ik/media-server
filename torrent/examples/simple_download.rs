use std::fs;

use torrent::{Client, ClientConfig, Torrent};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let mut client = Client::new(ClientConfig::default()).await.unwrap();
    let codecrafters = Torrent::from_file("torrents/codecrafters.torrent").unwrap();
    let orig = fs::read("codecrafters_original.txt").unwrap();
    client.download(codecrafters).await;
    let downloaded = fs::read("sample.txt").unwrap();
    assert_eq!(orig, downloaded);
}

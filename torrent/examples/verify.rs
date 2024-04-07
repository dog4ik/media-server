use torrent::Torrent;
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let torrent = Torrent::from_file("codecrafters.torrent").unwrap();
    torrent.verify_integrity(".").await.unwrap();
}

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
    let codecrafters = Torrent::from_mangnet_link("magnet:?xt=urn:btih:2770FE270845674966E184BE60ED1BE0FE494F3A&dn=Dune%20Part%20Two%20(2024)%20%5B1080p%5D%20%5BWEBRip%5D%2088&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopen.demonii.com%3A1337%2Fannounce").await.unwrap();
    let handle = client
        .download(".", codecrafters, |p| println!("Progress: {}", p))
        .await
        .unwrap();
    handle.await.unwrap().unwrap();
}

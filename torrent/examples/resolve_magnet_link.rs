use torrent::{Client, ClientConfig};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let url = "magnet:?xt=urn:btih:FEC8D3A3A6F2EAD02BD8F0EBD0DD92E2F0E7D7EE&dn=Dexter%20(2006)%20Season%201-8%20S01-S08%20(1080p%20BluRay%20x265%20HEVC%2010bit%20A&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopen.demonii.com%3A1337%2Fannounce";
    let magnet_link = url.parse().unwrap();
    let link = client.resolve_magnet_link(&magnet_link).await.unwrap();
    println!("Resolved name: {}", link.name);
}

use torrent::{Client, ClientConfig, DownloadProgress};
use tracing::Level;
use tracing_subscriber::EnvFilter;

fn bytes_in_mb(speed: u64) -> u64 {
    let bytes = speed;
    let kb = bytes / 1024;
    kb / 1024
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let url = "magnet:?xt=urn%3Abtih%3AE653457D2B6654FEE48CFFBB0C2D9689E195EF18&dn=Fallout+2024+S01E04+1080p+HEVC+x265-MeGusta&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Ftracker.openbittorrent.com%3A6969%2Fannounce&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopentracker.i2p.rocks%3A6969%2Fannounce";
    let magnet_link = url.parse().unwrap();
    let torrent = client.from_magnet_link(magnet_link).await.unwrap();
    let handle = client
        .download(".", torrent, |p: DownloadProgress| {
            println!("Progress: {}", p.percent);
            println!("Total Peers: {}", p.peers.len());
            println!("Pending pieces: {}", p.pending_pieces);
            println!("");
            println!("");
        })
        .await
        .unwrap();
    handle.handle.await.unwrap().unwrap();
}

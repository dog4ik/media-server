use torrent::{file::TorrentFile, Client, ClientConfig, DownloadProgress};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .with_line_number(true)
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let torrent_file = TorrentFile::from_path("sample.torrent").expect("Torrent file to exist");
    let trackers = torrent_file.all_trackers();
    let torrent = client
        .create_torrent(torrent_file.info, trackers)
        .await
        .unwrap();
    let mut handle = client
        .download(".", torrent, |p: DownloadProgress| {
            println!("Progress: {}", p.percent);
        })
        .await
        .unwrap();
    handle.wait().await;
}

use tokio::sync::mpsc;
use torrent::{Client, ClientConfig, DownloadState, TorrentFile};
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
    let (tx, mut rx) = mpsc::channel(100);
    client
        .download(
            ".",
            torrent_file.all_trackers(),
            torrent_file.info,
            vec![0],
            tx,
        )
        .await
        .unwrap();
    while let Some(progress) = rx.recv().await {
        println!("Progress: {}", progress.percent);
        if progress.state == DownloadState::Seeding {
            break;
        }
    }
}

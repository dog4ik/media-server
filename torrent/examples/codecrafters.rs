use tokio::sync::mpsc;
use torrent::{
    Client, ClientConfig, DownloadParams, DownloadState, Priority, Progress, TorrentFile,
};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .with_line_number(true)
        .init();
    let (tx, mut rx) = mpsc::channel(100);
    let client = Client::new(ClientConfig::default(), tx).await.unwrap();
    let torrent_file = TorrentFile::from_path("sample.torrent").expect("Torrent file to exist");
    let torrent_size = torrent_file.info.total_size();
    let tracker_list = torrent_file.all_trackers();
    let resume_data = DownloadParams::empty(
        torrent_file.info,
        tracker_list,
        vec![Priority::default()],
        ".".into(),
    );
    client.open(resume_data).await.unwrap();
    while let Some(Progress {
        changed_torrents, ..
    }) = rx.recv().await
    {
        if let Some(changed_torrent) = changed_torrents.first() {
            println!(
                "Progress: {}",
                changed_torrent.total_downloaded as f64 / torrent_size as f64 * 100.
            );
            if changed_torrent.state == DownloadState::Seeding {
                break;
            }
        }
    }
}

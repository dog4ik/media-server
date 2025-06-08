use tokio::sync::mpsc;
use torrent::{
    Client, ClientConfig, DownloadParams, Priority, ProgressDownloadState, StateChange, TorrentFile,
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
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let torrent_file = TorrentFile::from_path("sample.torrent").expect("Torrent file to exist");
    let (tx, mut rx) = mpsc::channel(100);
    let tracker_list = torrent_file.all_trackers();
    let resume_data = DownloadParams::empty(
        torrent_file.info,
        tracker_list,
        vec![Priority::default()],
        ".".into(),
    );
    client.open(resume_data, tx).await.unwrap();
    while let Some(progress) = rx.recv().await {
        println!("Progress: {}", progress.percent);
        if progress.changes.contains(&StateChange::DownloadStateChange(
            ProgressDownloadState::Seeding,
        )) {
            break;
        }
    }
}

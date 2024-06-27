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
    let mut handle = client
        .download(
            ".",
            torrent_file.all_trackers(),
            torrent_file.info,
            vec![0],
            |p: DownloadProgress| {
                println!("Progress: {}", p.percent);
            },
        )
        .await
        .unwrap();
    handle.wait().await;
}

use torrent::{Client, ClientConfig, DownloadProgress};
use tracing::Level;
use tracing_subscriber::EnvFilter;

fn bytes_in_mb(speed: u64) -> f64 {
    let bytes = speed;
    let kb = bytes as f64 / 1024.;
    kb / 1024.
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let save_location = ".";
    let url = "magnet:?xt=urn%3Abtih%3A641B77D3B839FAABA35898113DB111DC4355A3F5&dn=The.Boys.S04E07.1080p.WEB.h264-ETHEL%5BTGx%5D&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Ftracker.openbittorrent.com%3A6969%2Fannounce&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopentracker.i2p.rocks%3A6969%2Fannounce";
    let magnet_link = url.parse().unwrap();
    let info = client.resolve_magnet_link(&magnet_link).await.unwrap();
    tracing::info!("Resolved magnet link: {}", info.name);
    let files = (0..info.output_files("").len()).collect();
    client
        .download(
            save_location,
            magnet_link.announce_list.unwrap(),
            info,
            files,
            |p: DownloadProgress| {
                println!("");
                println!("");
                let mut total_download_speed = 0;
                let mut total_upload_speed = 0;
                for peer in &p.peers {
                    total_download_speed += peer.download_speed;
                    total_upload_speed += peer.upload_speed;

                    // println!("");
                    // println!("Speed: {} mb", bytes_in_mb(peer.speed));
                    // println!("Downloaded: {}", bytes_in_mb(peer.downloaded));
                    // println!("Blocks: {}", peer.pending_blocks);
                }
                println!("");
                println!("Progress: {}", p.percent);
                println!("Total Peers: {}", p.peers.len());
                println!("Pending pieces: {}", p.pending_pieces);
                println!("Download state: {}", p.state);
                println!(
                    "TOTAL DOWNLOAD SPEED: {} mb",
                    bytes_in_mb(total_download_speed)
                );
                println!("TOTAL UPLOAD SPEED: {} mb", bytes_in_mb(total_upload_speed));
            },
        )
        .await
        .unwrap();
    let _ = tokio::signal::ctrl_c().await;
    client.shutdown().await;
    println!("Done");
}

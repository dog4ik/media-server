use std::{path::PathBuf, time::Instant};

use torrent::{BitField, Client, ClientConfig, DownloadParams, TorrentFile};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .init();
    let client = Client::new(ClientConfig::default()).await.unwrap();
    let torrent = include_bytes!("../sample.torrent");
    let torrent = TorrentFile::from_bytes(torrent).unwrap();
    let total_pieces = torrent.info.pieces.len();
    let enabled_files = (0..torrent.info.files_amount()).into_iter().collect();
    let mut full_bitfield = BitField::empty(total_pieces);
    for piece in 0..torrent.info.pieces.len() {
        full_bitfield.add(piece).unwrap();
    }
    let params = DownloadParams {
        bitfield: full_bitfield,
        trackers: torrent.all_trackers(),
        info: torrent.info,
        enabled_files,
        save_location: PathBuf::from(".").canonicalize().unwrap(),
    };
    let start = Instant::now();
    let validated_bitfield = client.validate(params).await.unwrap();
    println!(
        "Validation took: {:?}",
        start.elapsed()
    );
    if validated_bitfield.is_full(total_pieces) {
        println!("Torrent contents are valid!")
    }
}

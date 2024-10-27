use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::{ArgGroup, Parser, Subcommand};
use torrent::{
    file::{MagnetLink, TorrentFile},
    protocol::Info,
    Client, ClientConfig, DownloadProgress,
};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Resolve magnet link
    ResolveMagnet {
        /// Magnet Link
        magnet_link: Option<String>,
    },
    /// View contents of .torrent file
    ParseTorrentFile {
        /// Path to the .torrent file
        file: PathBuf,
    },
    /// Download torrent
    #[clap(group = ArgGroup::new("input"))]
    Download {
        /// Magnet link to download the torrent
        #[arg(long, group = "input")]
        magnet: Option<String>,

        /// Path to a .torrent file
        #[arg(long, group = "input")]
        torrent: Option<String>,
        /// Path to output location
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// List of enabled files indexes, separated by commas
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<usize>>,
    },
}

fn bytes_in_mb(speed: u64) -> f64 {
    let bytes = speed;
    let kb = bytes as f64 / 1024.;
    kb / 1024.
}

async fn parse_magnet_link(link: impl Into<Option<String>>) -> MagnetLink {
    match link.into() {
        Some(link) => link.parse().unwrap(),
        None => {
            let stdin = std::io::stdin();
            if !stdin.is_terminal() {
                let text = std::io::read_to_string(stdin).expect("Can not read stdin");
                text.parse().unwrap()
            } else {
                panic!("Provide magnet url in stdin or as argument");
            }
        }
    }
}

async fn resolve_magnet_link(client: &Client, url: &MagnetLink) -> anyhow::Result<Info> {
    client.resolve_magnet_link(url).await
}

fn parse_torrent_file(path: impl AsRef<Path>) -> anyhow::Result<TorrentFile> {
    TorrentFile::from_path(path)
}

fn show_progress(p: DownloadProgress) {
    println!("");
    println!("");
    let mut total_download_speed = 0;
    let mut total_upload_speed = 0;
    for peer in &p.peers {
        total_download_speed += peer.download_speed;
        total_upload_speed += peer.upload_speed;
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
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let client = Client::new(ClientConfig::default()).await.unwrap();

    match args.command {
        Commands::ResolveMagnet { magnet_link } => {
            let magnet_link = parse_magnet_link(magnet_link).await;
            let info = resolve_magnet_link(&client, &magnet_link).await.unwrap();
            print!("{info}");
        }
        Commands::ParseTorrentFile { file } => {
            let file = parse_torrent_file(file).unwrap();
            println!("{}", file.info);
        }
        Commands::Download {
            magnet,
            torrent,
            output,
            files,
        } => {
            let (announce_list, info) = match (torrent, magnet) {
                (None, Some(url)) => {
                    let magnet_link = url.parse().unwrap();
                    let info = resolve_magnet_link(&client, &magnet_link).await.unwrap();
                    (magnet_link.announce_list.unwrap(), info)
                }
                (Some(path), None) => {
                    let file = parse_torrent_file(path).unwrap();
                    (file.all_trackers(), file.info)
                }
                (None, None) => {
                    let magnet_link = parse_magnet_link(None).await;
                    let info = resolve_magnet_link(&client, &magnet_link).await.unwrap();
                    (magnet_link.announce_list.unwrap(), info)
                }
                _ => {
                    unreachable!();
                }
            };
            let output = output.unwrap_or(PathBuf::from("."));
            let files = files.unwrap_or_else(|| (0..info.output_files(&output).len()).collect());
            let mut handle = client
                .download(output, announce_list, info, files, show_progress)
                .await
                .unwrap();
            tokio::select! {
                _ = handle.wait() => {},
                _ = tokio::signal::ctrl_c() => {}
            }
            client.shutdown().await;
            println!("Done");
        }
    }
}

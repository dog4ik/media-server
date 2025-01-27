use std::time::Duration;

use clap::Parser;
use upnp::search_client::{SearchClient, SearchOptions};

#[derive(Parser, Debug)]
enum Command {
    Play,
    Pause,
    Seek {
        /// Time to seek into in secs
        #[clap(short, long)]
        time: u64,
    },
    PositionInfo,
}

#[derive(Parser, Debug)]
struct Args {
    #[clap(subcommand)]
    action: Command,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let client = SearchClient::bind().await.unwrap();

    let options = SearchOptions::new()
        .take(Some(1))
        .with_timeout(Duration::from_secs(4));

    let client = match client
        .search_for(options)
        .await
        .map(|c| c.into_iter().next())
    {
        Ok(Some(c)) => c,
        Ok(None) => panic!("Requested client is not found"),
        Err(e) => panic!("Search failed: {e}"),
    };

    match args.action {
        Command::Play => {
            client.play("1").await.unwrap();
            println!("Resumed playback");
        }
        Command::Pause => {
            client.pause().await.unwrap();
            println!("Paused playback");
        }
        Command::Seek { time } => {
            let duration = Duration::from_secs(time);
            client.seek(duration).await.unwrap();
            println!("Seek to {:?}", duration);
        }
        Command::PositionInfo => {
            let pos = client.position_info().await.unwrap();
            println!(
                "({}) - {:?}/{:?} track {}",
                pos.url, pos.abs_time, pos.duration, pos.track
            );
        }
    }
}

use clap::Parser;
use serde::{Deserialize, Serialize};

use super::{CONFIG, Port, TmdbKey};

#[derive(Debug, Parser, Deserialize, Serialize)]
#[command(version)]
pub struct Args {
    /// Override port
    #[arg(short, long)]
    pub port: Option<u16>,
    /// Override tmdb api token
    #[arg(long)]
    pub tmdb_token: Option<String>,
}

impl Args {
    pub fn apply_configuration(self) {
        if let Some(port) = self.port {
            CONFIG.apply_cli_value(Port(port));
        }
        if let Some(token) = self.tmdb_token {
            CONFIG.apply_cli_value(TmdbKey(Some(token)));
        }
    }
}

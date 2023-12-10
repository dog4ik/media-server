use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::utils;

use super::{LibraryItem, Source};

pub struct ShowExtractor(pub ShowFile);

#[derive(Debug, Deserialize, Clone)]
pub struct ShowParams {
    pub show_name: String,
    pub season: usize,
    pub episode: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowFile {
    pub local_title: String,
    pub episode: u8,
    pub season: u8,
    pub source: Source,
}

impl ShowFile {
    pub fn new(path: PathBuf) -> Result<Self, anyhow::Error> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_os_string().into_string().ok())
            .ok_or(anyhow!("Failed to get file name:{}", path.display()))?;
        let tokens = utils::tokenize_filename(file_name);
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;

        for token in tokens {
            let chars: Vec<char> = token.chars().into_iter().collect();
            let is_year = token.len() == 6
                && chars[0] == '('
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
                && chars[4].is_ascii_digit()
                && chars[5] == ')';
            if is_year && season.is_none() && episode.is_none() {
                continue;
            }

            if token.len() == 6
                && chars[0] == 's'
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3] == 'e'
                && chars[4].is_ascii_digit()
                && chars[5].is_ascii_digit()
            {
                let s: Option<u8> = token[1..3].parse().ok();
                let e: Option<u8> = token[4..6].parse().ok();
                if let (Some(se), Some(ep)) = (s, e) {
                    season = Some(se);
                    episode = Some(ep);
                    break;
                };
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name.clone(), season, episode) {
            let resources_path = generate_resources_path(&name, season, episode);
            utils::generate_resources(&resources_path)?;
            let source = Source::new(path, resources_path)?;
            let show_file = Self {
                local_title: name,
                episode,
                season,
                source,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::anyhow!(
                "Failed to construct a show name ({:?}, {:?}, {:?})",
                name,
                season,
                episode
            ));
        }
    }
}

impl LibraryItem for ShowFile {
    fn resources_path(&self) -> &Path {
        &self.source.resources_path
    }
    fn source(&self) -> &Source {
        &self.source
    }
    fn title(&self) -> String {
        self.local_title.clone()
    }
    fn url(&self) -> String {
        format!("/{}/{}/{}", self.local_title, self.season, self.episode)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}

fn generate_resources_path(title: &str, season: u8, episode: u8) -> PathBuf {
    let mut episode_dir_path =
        PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
    episode_dir_path.push(title);
    episode_dir_path.push(season.to_string());
    episode_dir_path.push(episode.to_string());
    episode_dir_path
}

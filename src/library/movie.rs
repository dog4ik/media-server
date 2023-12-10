use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::utils;

use super::{LibraryItem, Source};

#[derive(Debug, Clone, Serialize)]
pub struct MovieFile {
    pub local_title: String,
    pub source: Source,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MovieParams {
    pub movie_name: String,
}

impl MovieFile {
    pub fn new(path: PathBuf) -> Result<Self, anyhow::Error> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_os_string().into_string().ok())
            .ok_or(anyhow!("Failed to get file name:{}", path.display()))?;
        let tokens = utils::tokenize_filename(file_name);
        let mut name: Option<String> = None;
        for token in tokens {
            let chars: Vec<char> = token.chars().into_iter().collect();
            let is_year = token.len() == 6
                && chars[0] == '('
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
                && chars[4].is_ascii_digit()
                && chars[5] == ')';
            if is_year {
                continue;
            }
            if (token.len() == 4 || token.len() == 5)
                && chars[0].is_ascii_digit()
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
            {
                break;
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let Some(name) = name {
            let resources_path = generate_resources_path(&name);
            utils::generate_resources(&resources_path)?;
            let source = Source::new(path, resources_path)?;
            let show_file = Self {
                local_title: name,
                source,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to construct a movie name"));
        }
    }
}

impl LibraryItem for MovieFile {
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
        format!("/{}", self.local_title)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}

fn generate_resources_path(title: &str) -> PathBuf {
    let mut movie_dir_path = PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
    movie_dir_path.push(title);
    movie_dir_path
}

use std::{fs, path::PathBuf};

use serde::Serialize;
use tokio::process::Command;

use crate::{
    get_metadata,
    process_file::{self, FFprobeOutput},
};

#[derive(Debug, Clone, Serialize)]
pub struct ShowFile {
    pub title: String,
    pub episode: u8,
    pub season: u8,
    pub video_path: PathBuf,
    pub resources_path: PathBuf,
    pub metadata: FFprobeOutput,
}

impl ShowFile {
    pub fn new(path: PathBuf) -> Result<ShowFile, anyhow::Error> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let mut is_spaced = false;
        if file_name.contains(" ") {
            is_spaced = true
        }
        let tokens = match is_spaced {
            true => file_name.split(" "),
            false => file_name.split("."),
        };
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;
        for token in tokens.map(|x| x.to_string().to_lowercase()) {
            if token.len() == 6 && token.starts_with("s") {
                match (
                    Some(token.get(1..3).unwrap().parse().unwrap()),
                    Some(token.get(4..6).unwrap().parse().unwrap()),
                ) {
                    (Some(se), Some(ep)) => {
                        season = Some(se);
                        episode = Some(ep);
                        break;
                    }
                    _ => (),
                };
            }
            match name {
                Some(ref mut n) => n.push_str(format!(" {}", token).as_str()),
                None => name = Some(token),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name, season, episode) {
            let resource = generate_resources(&name, season, episode)?;
            let metadata = get_metadata(&path).unwrap();
            let show_file = ShowFile {
                title: name,
                episode,
                season,
                video_path: path,
                resources_path: resource,
                metadata,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to build"));
        }
    }

    pub async fn get_subtitles(&self, lang: Option<String>) -> Option<String> {
        let mut subs_dir =
            tokio::fs::read_dir(format!("{}/subs", self.resources_path.to_str().unwrap()))
                .await
                .unwrap();
        let mut subs: Option<String> = None;
        loop {
            if let Some(file) = subs_dir.next_entry().await.unwrap() {
                let file_path = file.path();
                let file_name = file_path.file_stem().unwrap().to_str().unwrap();

                subs = match &lang {
                    Some(lang) => {
                        if file_name == lang {
                            Some(tokio::fs::read_to_string(file.path()).await.unwrap())
                        } else {
                            continue;
                        }
                    }
                    None => {
                        if &file_name == &"unknown" || &file_name == &"eng" {
                            Some(tokio::fs::read_to_string(file_path).await.unwrap())
                        } else {
                            continue;
                        }
                    }
                };
            } else {
                break;
            }
        }
        subs
    }

    pub fn get_previews(&self) -> Result<Vec<Vec<u8>>, std::io::Error> {
        let previews_dir = fs::read_dir(format!(
            "{}/previews",
            self.resources_path.to_str().unwrap()
        ))?;
        let mut previews_vec = vec![];
        for file in previews_dir {
            let file = file.unwrap().path();
            if file.extension().unwrap() == "str" {
                previews_vec.push(fs::read(file).unwrap());
            }
        }
        Ok(previews_vec)
    }

    pub async fn generate_subtitles(
        &self,
        track: i32,
        language: &str,
    ) -> Result<(), anyhow::Error> {
        Command::new("ffmpeg")
            .args(&[
                "-i",
                self.video_path.to_str().unwrap(),
                "-map",
                format!("0:{}", track).as_str(),
                format!(
                    "{}/subs/{}.srt",
                    self.resources_path.to_str().unwrap(),
                    language
                )
                .as_str(),
                "-y",
            ])
            .spawn()
            .unwrap()
            .wait()
            .await?;
        println!(
            "Generating subs for {:?}",
            format!("{} {} {}", self.title, self.season, self.episode)
        );
        Ok(())
    }

    pub async fn transcode_audio(&self, track: i32) -> Result<(), anyhow::Error> {
        let buffer_path = format!("{}buffer", self.video_path.to_str().unwrap(),);
        println!(
            "Transcoding audio for {:?}",
            format!("{} {} {}", self.title, self.season, self.episode)
        );
        fs::rename(&self.video_path, &buffer_path)?;
        Command::new("ffmpeg")
            .args(&[
                "-i",
                &buffer_path,
                "-map",
                "0:v:0",
                "-map",
                format!("0:{}", track).as_str(),
                "-acodec",
                "aac",
                "-vcodec",
                "copy",
                format!("{}", self.video_path.to_str().unwrap()).as_str(),
            ])
            .spawn()
            .unwrap()
            .wait()
            .await?;
        println!("Removed file: {:?}", buffer_path);
        fs::remove_file(buffer_path)?;
        Ok(())
    }

    pub async fn get_metadata(&self) -> Result<process_file::FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path)
    }

    pub async fn generate_previews(&self) -> Result<(), anyhow::Error> {
        Command::new("ffmpeg")
            .args([
                "-i",
                self.video_path.to_str().unwrap(),
                "-vf",
                "fps=1/10,scale=120:-1",
                format!("{}/previews/%d.jpg", self.resources_path.to_str().unwrap()).as_str(),
            ])
            .spawn()
            .unwrap()
            .wait()
            .await?;
        println!(
            "Generating previews for {:?}",
            format!("{} {} {}", self.title, self.season, self.episode)
        );
        Ok(())
    }
}

fn generate_resources(title: &str, season: u8, episode: u8) -> Result<PathBuf, std::io::Error> {
    let episode_dir_path = format!(
        "{}/{}/{}/{}",
        std::env::var("RESOURCES_PATH").unwrap(),
        title,
        season,
        episode
    );
    fs::create_dir_all(format!("{}/subs", &episode_dir_path))?;
    fs::create_dir_all(format!("{}/previews", &episode_dir_path))?;
    let folder = PathBuf::from(episode_dir_path);
    Ok(folder)
}

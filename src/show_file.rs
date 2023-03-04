use std::{fs, path::PathBuf};

use tokio::process::Command;

use crate::{get_metadata, process_file};

#[derive(Debug, Clone)]
pub struct ShowFile {
    pub title: String,
    pub episode: u8,
    pub season: u8,
    pub video_path: PathBuf,
    pub resources_path: PathBuf,
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
            let show_file = ShowFile {
                title: name,
                episode,
                season,
                video_path: path,
                resources_path: resource,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to build"));
        }
    }

    pub fn get_subtitles(&self) -> Option<String> {
        match fs::read_to_string(format!(
            "{}/subs/eng.srt",
            self.resources_path.to_str().unwrap()
        )) {
            Ok(sub) => Some(sub),
            Err(_) => None,
        }
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
        let out = Command::new("ffmpeg")
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
            .output()
            .await?;
        println!("{:?}", out);
        Ok(())
    }

    pub async fn transcode_audio(&self, track: i32) -> Result<(), anyhow::Error> {
        let buffer_path = format!("{}buffer", self.video_path.to_str().unwrap(),);
        println!("{buffer_path}");
        fs::rename(&self.video_path, &buffer_path)?;
        let out = Command::new("ffmpeg")
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
            .output()
            .await?;
        println!("Remove {:?}", buffer_path);
        fs::remove_file(buffer_path)?;
        Ok(())
    }

    pub async fn get_metadata(&self) -> Result<process_file::FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path).await
    }

    pub async fn generate_previews(&self) -> Result<(), anyhow::Error> {
        let out = Command::new("ffmpeg")
            .args([
                "-i",
                self.video_path.to_str().unwrap(),
                "-vf",
                "fps=1/10,scale=120:-1",
                format!("{}/previews/%d.jpg", self.resources_path.to_str().unwrap()).as_str(),
            ])
            .output()
            .await?;
        println!("{:?}", out);
        Ok(())
    }
}

fn generate_resources(title: &str, season: u8, episode: u8) -> Result<PathBuf, std::io::Error> {
    let episode_dir_path = format!(
        "/home/dog4ik/Documents/dev/rust/media-server/resources/{}/{}/{}",
        title, season, episode
    );
    fs::create_dir_all(format!("{}/subs", &episode_dir_path))?;
    fs::create_dir_all(format!("{}/previews", &episode_dir_path))?;
    let folder = PathBuf::from(episode_dir_path);
    Ok(folder)
}

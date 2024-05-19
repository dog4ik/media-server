use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, ErrorKind, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::OnceLock,
};

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::ffmpeg::{self, H264Preset};

#[derive(Debug, Serialize, Clone)]
pub struct ServerConfiguration {
    pub port: u16,
    pub capabilities: Capabilities,
    pub movie_folders: Vec<PathBuf>,
    pub show_folders: Vec<PathBuf>,
    pub resources: AppResources,
    #[serde(skip_serializing)]
    pub config_file: ConfigFile,
    pub scan_max_concurrency: usize,
    pub h264_preset: H264Preset,
    pub ffprobe_path: Option<PathBuf>,
    pub ffmpeg_path: Option<PathBuf>,
    pub tmdb_token: Option<String>,
    pub hw_accel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlConfig {
    port: u16,
    movie_folders: Vec<PathBuf>,
    show_folders: Vec<PathBuf>,
    scan_max_concurrency: usize,
    h264_preset: H264Preset,
    ffprobe_path: PathBuf,
    ffmpeg_path: PathBuf,
    hw_accel: bool,
}

impl Default for TomlConfig {
    fn default() -> Self {
        Self {
            show_folders: Vec::new(),
            movie_folders: Vec::new(),
            port: 6969,
            scan_max_concurrency: 10,
            h264_preset: H264Preset::default(),
            // TODO: move to full path to local dependency
            ffprobe_path: "ffprobe".into(),
            ffmpeg_path: "ffmpeg".into(),
            hw_accel: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigFile(pub PathBuf);

impl ConfigFile {
    pub fn open(config_path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let path = config_path.as_ref().to_path_buf();
        fs::create_dir_all(config_path.as_ref().parent().ok_or(anyhow::anyhow!(
            "config path does not have parent directory"
        ))?)?;
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&config_path)
            .map_err(|e| e.kind())
        {
            Err(ErrorKind::NotFound) => {
                let default_config = TomlConfig::default();
                let mut file = fs::File::create_new(&config_path)?;
                let _ = file.write_all(&toml::to_string_pretty(&default_config)?.as_bytes());
                tracing::info!(
                    "Created configuration file with defaults: {}",
                    config_path.as_ref().display()
                );
                Ok(ConfigFile(path))
            }
            Err(e) => Err(anyhow::anyhow!(
                "Unknown fs error occured while creating config: {e}"
            )),
            Ok(_) => Ok(ConfigFile(path)),
        }
    }

    /// Reads contents of config and resets it to defaults in case of parse error
    fn read(&self) -> Result<TomlConfig, anyhow::Error> {
        tracing::info!("Reading config file {}", self.0.display());
        let buf = fs::read_to_string(&self.0)?;
        Ok(toml::from_str(&buf).unwrap_or_else(|e| {
            tracing::error!("Failed to read config: {}", e);
            tracing::info!("Resetting broken config");
            if let Ok(repaired_config) = repair_config(&buf) {
                tracing::info!("Successfuly repaired config");
                let _ = fs::write(&self.0, &toml::to_string_pretty(&repaired_config).unwrap());
                repaired_config
            } else {
                tracing::error!("Failed to repair config, creating default one");
                let default_config = TomlConfig::default();
                let _ = fs::write(&self.0, &toml::to_string_pretty(&default_config).unwrap());
                default_config
            }
        }))
    }

    fn flush(&self, config: TomlConfig) -> Result<(), anyhow::Error> {
        let config_text = toml::to_string_pretty(&config)?;
        fs::write(&self.0, &config_text)?;
        Ok(())
    }
}

//NOTE: I hope to find solution to this mess
fn repair_config(raw: &str) -> Result<TomlConfig, anyhow::Error> {
    tracing::trace!("Trying to repair config");
    let default = TomlConfig::default();
    let parsed: toml::Table = toml::from_str(raw)?;
    let port: u16 = parsed
        .get("port")
        .and_then(|v| v.as_integer())
        .map_or(default.port, |v| v as u16);
    let movie_folders = parsed
        .get("movie_folders")
        .and_then(|v| v.as_array())
        .map(|v| {
            v.iter()
                .filter_map(|v| v.as_str())
                .map(|x| x.into())
                .collect()
        })
        .unwrap_or(default.movie_folders);
    let show_folders = parsed
        .get("show_folders")
        .and_then(|v| v.as_array())
        .map(|v| {
            v.iter()
                .filter_map(|v| v.as_str())
                .map(|x| x.into())
                .collect()
        })
        .unwrap_or(default.show_folders);
    let scan_max_concurrency = parsed
        .get("scan_max_concurrency")
        .and_then(|v| v.as_integer())
        .map_or(default.scan_max_concurrency, |v| v as usize);
    let h264_preset = parsed
        .get("h264_preset")
        .and_then(|v| v.as_str())
        .and_then(|v| H264Preset::from_str(v).ok())
        .unwrap_or(default.h264_preset);
    let ffmpeg_path = parsed
        .get("ffmpeg_path")
        .and_then(|v| v.as_str())
        .and_then(|v| PathBuf::from_str(v).ok())
        .unwrap_or(default.ffmpeg_path);
    let ffprobe_path = parsed
        .get("ffprobe_path")
        .and_then(|v| v.as_str())
        .and_then(|v| PathBuf::from_str(v).ok())
        .unwrap_or(default.ffprobe_path);
    let hw_accel = parsed
        .get("hw_accel")
        .and_then(|v| v.as_bool())
        .unwrap_or(default.hw_accel);

    let repaired_config = TomlConfig {
        port,
        movie_folders,
        show_folders,
        scan_max_concurrency,
        h264_preset,
        ffmpeg_path,
        ffprobe_path,
        hw_accel,
    };
    Ok(repaired_config)
}

impl ServerConfiguration {
    /// Into Json Config
    fn into_toml(&self) -> TomlConfig {
        TomlConfig {
            port: self.port,
            movie_folders: self.movie_folders.clone(),
            show_folders: self.show_folders.clone(),
            scan_max_concurrency: self.scan_max_concurrency,
            h264_preset: self.h264_preset,
            ffmpeg_path: self.ffmpeg_path.clone().unwrap_or("ffmpeg".into()),
            ffprobe_path: self.ffprobe_path.clone().unwrap_or("ffprobe".into()),
            hw_accel: self.hw_accel,
        }
    }
    /// Try to load config or creates default config file
    /// Errors when can't create or read file
    pub fn new(config: ConfigFile) -> Result<Self, anyhow::Error> {
        let file_config = config.read()?;
        let ffmpeg_path = ffmpeg::healthcheck_ffmpeg_command(&file_config.ffmpeg_path)
            .map(|version| {
                tracing::info!(version, "Found ffmpeg");
                file_config.ffmpeg_path
            })
            .map_err(|e| {
                tracing::warn!("Could not find ffmpeg: {}", e);
                e
            })
            .ok();
        let ffprobe_path = ffmpeg::healthcheck_ffmpeg_command(&file_config.ffprobe_path)
            .map(|version| {
                tracing::info!(version, "Found ffprobe");
                file_config.ffprobe_path
            })
            .map_err(|e| {
                tracing::error!("Could not find ffprobe: {}", e);
                e
            })
            .ok();

        let config = ServerConfiguration {
            resources: AppResources::new(
                config.0.clone(),
                ffmpeg_path.clone(),
                ffprobe_path.clone(),
            ),
            port: file_config.port,
            capabilities: ffprobe_path
                .as_ref()
                .and_then(|x| Capabilities::parse(&x).ok())
                .unwrap_or_default(),
            movie_folders: file_config.movie_folders,
            show_folders: file_config.show_folders,
            config_file: config,
            scan_max_concurrency: file_config.scan_max_concurrency,
            h264_preset: H264Preset::default(),
            ffprobe_path,
            ffmpeg_path,
            tmdb_token: std::env::var("TMDB_TOKEN").ok(),
            hw_accel: file_config.hw_accel,
        };
        Ok(config)
    }

    pub fn apply_args(&mut self, args: Args) {
        args.port.map(|x| self.port = x);
        args.tmdb_token.map(|x| self.tmdb_token = Some(x));
    }

    pub fn add_show_folder(&mut self, show_folder: PathBuf) -> Result<(), anyhow::Error> {
        self.show_folders.push(show_folder);
        self.flush()
    }

    pub fn add_movie_folder(&mut self, movie_folder: PathBuf) -> Result<(), anyhow::Error> {
        self.movie_folders.push(movie_folder);
        self.flush()
    }

    pub fn remove_show_folder(
        &mut self,
        show_folder: impl AsRef<Path>,
    ) -> Result<(), anyhow::Error> {
        let position = self
            .show_folders
            .iter()
            .position(|x| x == &show_folder.as_ref())
            .ok_or(anyhow::anyhow!("Could not find required folder"))?;
        self.show_folders.remove(position);
        self.flush()
    }

    pub fn remove_movie_folder(
        &mut self,
        show_folder: impl AsRef<Path>,
    ) -> Result<(), anyhow::Error> {
        let position = self
            .movie_folders
            .iter()
            .position(|x| x == &show_folder.as_ref())
            .ok_or(anyhow::anyhow!("Could not find required folder"))?;
        self.movie_folders.remove(position);
        self.flush()
    }

    pub fn set_resources_folder(&mut self, resources: AppResources) -> Result<(), anyhow::Error> {
        self.resources = resources;
        self.flush()
    }

    pub fn set_port(&mut self, port: u16) -> Result<(), anyhow::Error> {
        self.port = port;
        self.flush()
    }

    pub fn set_h264_preset(&mut self, preset: H264Preset) -> Result<(), anyhow::Error> {
        self.h264_preset = preset;
        self.flush()
    }

    /// Flush current configuration in config file
    pub fn flush(&mut self) -> Result<(), anyhow::Error> {
        self.config_file.flush(self.into_toml())?;
        Ok(())
    }
}

#[derive(Debug, Parser)]
pub struct Args {
    /// Override port
    #[arg(short, long)]
    pub port: Option<u16>,
    /// Provide custom config location
    #[arg(short, long)]
    pub config_path: Option<PathBuf>,
    /// Override tmdb api token
    #[arg(long)]
    pub tmdb_token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecType {
    Audio,
    Video,
    Subtitle,
    Data,
    Attachment,
}

impl CodecType {
    pub fn from_char(char: char) -> Option<Self> {
        match char {
            'V' => Some(Self::Video),
            'A' => Some(Self::Audio),
            'S' => Some(Self::Subtitle),
            'D' => Some(Self::Data),
            'T' => Some(Self::Attachment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codec {
    pub codec_type: CodecType,
    pub name: String,
    pub long_name: String,
    pub decode_supported: bool,
    pub encode_supported: bool,
}

impl Codec {
    pub fn from_capability_line(line: String) -> Self {
        let mut split = line.split_terminator(' ').filter(|chunk| chunk.len() != 0);
        let mut params = split.next().unwrap().chars();
        let name = split.next().unwrap().to_string();
        let long_name: String = split.intersperse(" ").collect();
        let decode_supported = params.next().unwrap() == 'D';
        let encode_supported = params.next().unwrap() == 'E';
        let codec_type = CodecType::from_char(params.next().unwrap()).unwrap();
        Self {
            name,
            long_name,
            codec_type,
            encode_supported,
            decode_supported,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capabilities {
    pub codecs: Vec<Codec>,
}

impl Capabilities {
    pub fn parse(ffmpeg_path: impl AsRef<OsStr>) -> Result<Self, anyhow::Error> {
        let output = Command::new(ffmpeg_path)
            .args(["-hide_banner", "-codecs"])
            .output()?;
        let lines = if output.status.code().unwrap_or(1) != 0 {
            return Err(anyhow::anyhow!("ffmpeg -codces command failed"));
        } else {
            output.stdout.lines()
        };

        // skip ffmpeg heading
        let mut lines = lines.skip_while(|line| {
            !line
                .as_ref()
                .map(|l| l.starts_with(" ---"))
                .unwrap_or(false)
        });
        lines.next();

        let mut codecs = Vec::new();
        while let Some(Ok(line)) = lines.next() {
            codecs.push(Codec::from_capability_line(line));
        }

        Ok(Self { codecs })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResources {
    pub database_path: PathBuf,
    #[serde(skip)]
    pub config_path: PathBuf,
    pub resources_path: PathBuf,
    pub temp_path: PathBuf,
    pub cache_path: PathBuf,
    pub ffmpeg_path: Option<PathBuf>,
    pub ffprobe_path: Option<PathBuf>,
    pub binary_path: Option<PathBuf>,
    pub base_path: PathBuf,
}

pub static APP_RESOURCES: OnceLock<AppResources> = OnceLock::new();

impl AppResources {
    pub const APP_NAME: &'static str = "media-server";

    fn prod_storage() -> PathBuf {
        dirs::data_dir()
            .expect("target to have data directory")
            .join(Self::APP_NAME)
    }

    fn debug_storage() -> PathBuf {
        PathBuf::from(".").canonicalize().unwrap()
    }

    fn data_storage() -> PathBuf {
        let is_prod = !cfg!(debug_assertions);
        if is_prod {
            Self::prod_storage()
        } else {
            Self::debug_storage()
        }
    }

    pub fn default_config_path() -> PathBuf {
        Self::data_storage().join("configuration.toml")
    }

    fn temp_storage() -> PathBuf {
        std::env::temp_dir().join(Self::APP_NAME)
    }

    fn cache_storage() -> PathBuf {
        dirs::cache_dir().unwrap().join(Self::APP_NAME)
    }

    fn database_directory() -> PathBuf {
        Self::data_storage().join("db")
    }

    fn resources() -> PathBuf {
        Self::data_storage().join("resources")
    }

    fn database() -> PathBuf {
        Self::database_directory().join("database.sqlite")
    }

    pub fn initiate() -> Result<(), std::io::Error> {
        fs::create_dir_all(Self::resources())?;
        fs::create_dir_all(Self::database_directory())?;
        fs::create_dir_all(Self::temp_storage())?;
        fs::create_dir_all(Self::cache_storage())?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::database())?;
        Ok(())
    }

    pub fn new(
        config_path: PathBuf,
        ffmpeg_path: Option<PathBuf>,
        ffprobe_path: Option<PathBuf>,
    ) -> Self {
        let resources_path = Self::resources();
        let database_path = Self::database();
        let temp_path = Self::temp_storage();
        let cache_path = Self::cache_storage();
        let binary_path = std::env::current_exe()
            .ok()
            .and_then(|d| d.parent().map(|x| x.to_path_buf()));

        let base_path = if cfg!(debug_assertions) {
            "".into()
        } else {
            binary_path.clone().unwrap()
        };
        Self {
            config_path,
            database_path,
            resources_path,
            temp_path,
            cache_path,
            ffmpeg_path,
            ffprobe_path,
            binary_path,
            base_path,
        }
    }
    pub fn ffmpeg(&self) -> &PathBuf {
        if let Some(path) = &self.ffmpeg_path {
            path
        } else {
            tracing::error!("Cannot operate without ffmpeg");
            panic!("ffmpeg required");
        }
    }
}

impl Default for AppResources {
    fn default() -> Self {
        let config_path = Self::default_config_path();
        Self::new(config_path, None, None)
    }
}

#[test]
fn parse_capabilities() {
    let capabilities = Capabilities::parse("ffmpeg");
    assert!(capabilities.is_ok())
}

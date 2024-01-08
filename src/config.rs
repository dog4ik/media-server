use std::{
    fs::{self},
    io::{BufRead, ErrorKind, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::ffmpeg::H264Preset;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConfigLogLevel {
    #[default]
    Trace,
    Warn,
    Debug,
    Error,
    Info,
}

impl FromStr for ConfigLogLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Trace" => Ok(Self::Trace),
            "Warn" => Ok(Self::Warn),
            "Debug" => Ok(Self::Debug),
            "Error" => Ok(Self::Error),
            "Info" => Ok(Self::Info),
            _ => Err(anyhow::anyhow!("{} does not match any log level", s)),
        }
    }
}

impl From<tracing::Level> for ConfigLogLevel {
    fn from(value: tracing::Level) -> Self {
        match value {
            tracing::Level::TRACE => Self::Trace,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::ERROR => Self::Error,
            tracing::Level::INFO => Self::Info,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct ServerConfiguration {
    pub port: u16,
    pub log_level: ConfigLogLevel,
    pub capabilities: Capabilities,
    pub log_path: PathBuf,
    pub movie_folders: Vec<PathBuf>,
    pub show_folders: Vec<PathBuf>,
    pub resources: AppResources,
    #[serde(skip_serializing)]
    pub config_file: ConfigFile,
    pub scan_max_concurrency: usize,
    pub is_setup: bool,
    pub h264_preset: H264Preset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlConfig {
    port: u16,
    log_level: ConfigLogLevel,
    log_path: PathBuf,
    movie_folders: Vec<PathBuf>,
    show_folders: Vec<PathBuf>,
    resources: AppResources,
    scan_max_concurrency: usize,
    h264_preset: H264Preset,
    is_setup: bool,
}

impl Default for TomlConfig {
    fn default() -> Self {
        Self {
            show_folders: Vec::new(),
            movie_folders: Vec::new(),
            port: 6969,
            log_level: ConfigLogLevel::Trace,
            log_path: PathBuf::from("log.log"),
            resources: AppResources::default(),
            scan_max_concurrency: 10,
            h264_preset: H264Preset::default(),
            is_setup: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigFile(pub PathBuf);

impl ConfigFile {
    pub fn open(config_path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let path = config_path.as_ref().to_path_buf();
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
        tracing::info!("reading file");
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
    let log_level = parsed
        .get("log_level")
        .and_then(|v| v.as_str())
        .and_then(|s| ConfigLogLevel::from_str(s).ok())
        .unwrap_or(default.log_level);
    let log_path = parsed
        .get("log_path")
        .and_then(|v| v.as_str())
        .map_or(default.log_path, |x| x.into());
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
    let resources =
        parsed
            .get("resources")
            .and_then(|x| x.as_table())
            .map_or(default.resources.clone(), |x| {
                let database_path = x
                    .get("database_path")
                    .and_then(|f| f.as_str())
                    .map_or(default.resources.database_path, |x| x.into());
                let config_path = x
                    .get("config_path")
                    .and_then(|f| f.as_str())
                    .map_or(default.resources.config_path, |x| x.into());
                let resources_path = x
                    .get("resources_path")
                    .and_then(|f| f.as_str())
                    .map_or(default.resources.resources_path, |x| x.into());
                let cache_path = x
                    .get("cache_path")
                    .and_then(|f| f.as_str())
                    .map_or(default.resources.cache_path, |x| x.into());
                AppResources {
                    database_path,
                    config_path,
                    resources_path,
                    cache_path,
                }
            });
    let scan_max_concurrency = parsed
        .get("scan_max_concurrency")
        .and_then(|v| v.as_integer())
        .map_or(default.scan_max_concurrency, |v| v as usize);
    let is_setup = parsed
        .get("is_setup")
        .and_then(|v| v.as_bool())
        .unwrap_or(default.is_setup);
    let h264_preset = parsed
        .get("h264_preset")
        .and_then(|v| v.as_str())
        .and_then(|v| H264Preset::from_str(v).ok())
        .unwrap_or(default.h264_preset);

    let repaired_config = TomlConfig {
        port,
        log_level,
        log_path,
        movie_folders,
        show_folders,
        resources,
        scan_max_concurrency,
        is_setup,
        h264_preset,
    };
    Ok(repaired_config)
}

impl ServerConfiguration {
    /// Into Json Config
    fn into_toml(&self) -> TomlConfig {
        TomlConfig {
            port: self.port,
            log_level: self.log_level.clone(),
            log_path: self.log_path.clone(),
            movie_folders: self.movie_folders.clone(),
            show_folders: self.show_folders.clone(),
            resources: self.resources.clone(),
            scan_max_concurrency: self.scan_max_concurrency,
            h264_preset: self.h264_preset,
            is_setup: self.is_setup,
        }
    }
    /// Tries to load config or creates default config file
    /// Errors when cant create or read file
    pub fn from_file(config_path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let file_config = ConfigFile::open(&config_path)?.read()?;

        let config = ServerConfiguration {
            resources: file_config.resources,
            port: file_config.port,
            log_level: file_config.log_level,
            capabilities: Capabilities::new()?,
            log_path: file_config.log_path,
            movie_folders: file_config.movie_folders,
            show_folders: file_config.show_folders,
            config_file: ConfigFile::open(config_path)?,
            scan_max_concurrency: file_config.scan_max_concurrency,
            h264_preset: H264Preset::default(),
            is_setup: file_config.is_setup,
        };
        Ok(config)
    }

    pub fn apply_args(&mut self, args: Args) {
        args.log_path.map(|x| self.log_path = x);
        args.port.map(|x| self.port = x);
        args.log_level.map(|x| self.log_level = x.into());
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

    pub fn set_log_level(&mut self, level: ConfigLogLevel) -> Result<(), anyhow::Error> {
        self.log_level = level;
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
    port: Option<u16>,
    /// Override log level
    #[arg(short, long)]
    log_level: Option<tracing::Level>,
    /// Override log location
    #[arg(long)]
    log_path: Option<PathBuf>,
    /// Provide custom config location
    #[arg(short, long)]
    config_path: Option<PathBuf>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub codecs: Vec<Codec>,
}

impl Capabilities {
    pub fn new() -> Result<Self, anyhow::Error> {
        let output = Command::new("ffmpeg")
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
    pub config_path: PathBuf,
    pub resources_path: PathBuf,
    pub cache_path: PathBuf,
}

impl AppResources {
    const APP_NAME: &'static str = "media-server";

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

    fn cache_storage() -> PathBuf {
        std::env::temp_dir().join(Self::APP_NAME)
    }

    pub fn initiate(&self) -> Result<(), anyhow::Error> {
        fs::create_dir_all(&self.resources_path)?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.database_path)?;
        Ok(())
    }
}

impl Default for AppResources {
    fn default() -> Self {
        let store_path = Self::data_storage();
        let config_path = Self::default_config_path();
        let db_folder = store_path.join("db");
        let resources_path = store_path.join("resources");
        let database_path = db_folder.join("database.sqlite");
        let cache_path = Self::cache_storage();

        Self {
            config_path,
            database_path,
            resources_path,
            cache_path,
        }
    }
}

#[test]
fn parse_capabilities() {
    let capabilities = Capabilities::new();
    assert!(capabilities.is_ok())
}

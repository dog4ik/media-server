use std::{
    fs::{self},
    io::{BufRead, ErrorKind, Write},
    path::{Path, PathBuf},
    process::Command,
};

use clap::Parser;
use serde::{Deserialize, Serialize};

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
    pub resources_folder: PathBuf,
    #[serde(skip_serializing)]
    pub config_file: ConfigFile,
    pub scan_max_concurrency: usize,
    pub is_setup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonConfig {
    port: u16,
    log_level: ConfigLogLevel,
    log_path: PathBuf,
    movie_folders: Vec<PathBuf>,
    show_folders: Vec<PathBuf>,
    resources_folder: PathBuf,
    scan_max_concurrency: usize,
    is_setup: bool,
}

impl Default for JsonConfig {
    fn default() -> Self {
        Self {
            show_folders: Vec::new(),
            movie_folders: Vec::new(),
            port: 6969,
            log_level: ConfigLogLevel::Trace,
            log_path: PathBuf::from("log.log"),
            resources_folder: PathBuf::from("resources"),
            scan_max_concurrency: 10,
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
                let default_config = JsonConfig::default();
                let mut file = fs::File::create_new(&config_path)?;
                let _ = file.write_all(&serde_json::to_vec_pretty(&default_config)?);
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
    fn read(&self) -> Result<JsonConfig, anyhow::Error> {
        tracing::info!("reading file");
        let buf = fs::read_to_string(&self.0)?;
        Ok(serde_json::from_str(&buf).unwrap_or_else(|e| {
            tracing::error!("Failed to read config: {}", e);
            tracing::info!("Resetting broken configc");
            if let Ok(repaired_config) = repair_config(&buf) {
                tracing::info!("Successfuly repaired config");
                let _ = fs::write(
                    &self.0,
                    &serde_json::to_vec_pretty(&repaired_config).unwrap(),
                );
                repaired_config
            } else {
                tracing::error!("Failed to repair config");
                let default_config = JsonConfig::default();
                let _ = fs::write(
                    &self.0,
                    &serde_json::to_vec_pretty(&default_config).unwrap(),
                );
                default_config
            }
        }))
    }

    fn flush(&self, json: JsonConfig) -> Result<(), anyhow::Error> {
        let json = serde_json::to_vec_pretty(&json)?;
        fs::write(&self.0, &json)?;
        Ok(())
    }
}

//NOTE: I hope to find solution to this mess
fn repair_config(raw: &str) -> Result<JsonConfig, anyhow::Error> {
    #[derive(Deserialize)]
    struct ShadowConfig {
        port: Option<u16>,
        log_level: Option<ConfigLogLevel>,
        log_path: Option<PathBuf>,
        movie_folders: Option<Vec<PathBuf>>,
        show_folders: Option<Vec<PathBuf>>,
        resources_folder: Option<PathBuf>,
        scan_max_concurrency: Option<usize>,
        is_setup: Option<bool>,
    }

    tracing::trace!("Trying to repair config");
    let json: ShadowConfig = serde_json::from_str(raw)?;
    let default = JsonConfig::default();

    let repaired_config = JsonConfig {
        port: json.port.unwrap_or(default.port),
        log_level: json.log_level.unwrap_or(default.log_level),
        log_path: json.log_path.unwrap_or(default.log_path),
        movie_folders: json.movie_folders.unwrap_or(default.movie_folders),
        show_folders: json.show_folders.unwrap_or(default.show_folders),
        resources_folder: json.resources_folder.unwrap_or(default.resources_folder),
        scan_max_concurrency: json
            .scan_max_concurrency
            .unwrap_or(default.scan_max_concurrency),
        is_setup: json.is_setup.unwrap_or(default.is_setup),
        ..Default::default()
    };
    Ok(repaired_config)
}

impl ServerConfiguration {
    /// Into Json Config
    fn into_json(&self) -> JsonConfig {
        JsonConfig {
            port: self.port,
            log_level: self.log_level.clone(),
            log_path: self.log_path.clone(),
            movie_folders: self.movie_folders.clone(),
            show_folders: self.show_folders.clone(),
            resources_folder: self.resources_folder.clone(),
            scan_max_concurrency: self.scan_max_concurrency,
            is_setup: self.is_setup,
        }
    }
    /// Tries to load config or creates default config file
    /// Errors when cant create or read file
    pub fn from_file(config_path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let json_config = ConfigFile::open(&config_path)?.read()?;

        let config = ServerConfiguration {
            resources_folder: json_config.resources_folder,
            port: json_config.port,
            log_level: json_config.log_level,
            capabilities: Capabilities::new()?,
            log_path: json_config.log_path,
            movie_folders: json_config.movie_folders,
            show_folders: json_config.show_folders,
            config_file: ConfigFile::open(config_path)?,
            scan_max_concurrency: json_config.scan_max_concurrency,
            is_setup: json_config.is_setup,
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

    pub fn set_resources_folder(
        &mut self,
        resources_folder: impl AsRef<Path>,
    ) -> Result<(), anyhow::Error> {
        self.resources_folder = resources_folder.as_ref().to_path_buf();
        self.flush()
    }

    pub fn set_port(&mut self, port: u16) -> Result<(), anyhow::Error> {
        self.port = port;
        self.flush()
    }

    pub fn set_log_level(&mut self, level: ConfigLogLevel) -> Result<(), anyhow::Error> {
        self.log_level = level;
        self.flush()
    }

    /// Flush current configuration in config file
    pub fn flush(&mut self) -> Result<(), anyhow::Error> {
        self.config_file.flush(self.into_json())?;
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

#[test]
fn parse_capabilities() {
    let capabilities = Capabilities::new();
    assert!(capabilities.is_ok())
}

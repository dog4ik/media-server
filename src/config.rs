use std::{
    any::{type_name, Any, TypeId},
    char,
    collections::HashMap,
    fmt::Display,
    io::BufRead,
    path::{Path, PathBuf},
    sync::{LazyLock, OnceLock},
};

use anyhow::Context;
use clap::Parser;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
};
use utoipa::openapi::RefOr;

fn camel_to_snake_case(input: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in input.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

#[derive(Debug)]
pub enum ValidationError {
    Bounds,
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ValidationError::Bounds => "bounds",
        };
        write!(f, "{}", msg)
    }
}

impl std::error::Error for ValidationError {}

// TODO: derive macro
pub trait ConfigValue:
    'static + Send + Sync + Default + Clone + Serialize + DeserializeOwned + utoipa::ToSchema<'static>
{
    const KEY: Option<&str> = None;
    const ENV_KEY: Option<&str> = None;
    const REQUIRE_RESTART: bool = false;

    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SettingValue<T> {
    default: T,
    config: Option<T>,
    cli: Option<T>,
    env: Option<T>,
}

#[derive(Debug, Serialize)]
pub struct SerializedSetting {
    require_restart: bool,
    key: String,
    default_value: serde_json::Value,
    config_value: serde_json::Value,
    cli_value: serde_json::Value,
    env_value: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConfigurationApplyError {
    pub message: String,
    pub key: String,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct ConfigurationApplyResult {
    pub require_restart: bool,
    pub errors: Vec<ConfigurationApplyError>,
}

impl<T: ConfigValue> SettingValue<T> {
    pub fn new(val: T) -> Self {
        use std::env::var;
        let env = match T::ENV_KEY {
            Some(key) => Some(key.to_string()),
            None => Some(T::KEY.map(str::to_uppercase).unwrap_or_else(|| {
                let (name, _) = T::schema();
                camel_to_snake_case(name).to_uppercase()
            })),
        }
        .and_then(|env_key| {
            let val = var(env_key).ok()?;
            match serde_plain::from_str(&val) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        found = val,
                        "Found env value but could not parse it as {}. {e}",
                        type_name::<T>()
                    );
                    None
                }
            }
        });
        Self {
            default: val,
            config: None,
            cli: None,
            env,
        }
    }

    /// Setting value with respect to it's source priority
    pub fn customized(&self) -> &T {
        self.cli
            .as_ref()
            .or(self.env.as_ref())
            .or(self.config.as_ref())
            .unwrap_or(&self.default)
    }
}

trait AnySettingValue: 'static + Send + Sync {
    fn key(&self) -> String;
    fn require_restart(&self) -> bool;
    fn type_name(&self) -> &'static str;

    fn customized_value(&self) -> &dyn Any;
    fn config_mut(&mut self) -> &mut dyn Any;
    fn cli_mut(&mut self) -> &mut dyn Any;
    fn reset_config_value(&mut self);

    fn serialize_config(&self) -> Option<toml::Value>;
    fn serialize_response(&self) -> SerializedSetting;

    fn deserialize_toml(&mut self, from: toml::Value) -> Result<(), toml::de::Error>;
    fn deserialize_json(&mut self, from: serde_json::Value) -> Result<(), serde_json::Error>;
}

impl<T: ConfigValue> AnySettingValue for SettingValue<T> {
    fn key(&self) -> String {
        T::KEY
            .map(|k| k.to_string())
            .unwrap_or_else(|| camel_to_snake_case(self.type_name()))
    }

    fn require_restart(&self) -> bool {
        T::REQUIRE_RESTART
    }

    fn type_name(&self) -> &'static str {
        T::schema().0
    }

    fn deserialize_toml(&mut self, from: toml::Value) -> Result<(), toml::de::Error> {
        let value = T::deserialize(from)?;
        self.config = Some(value);
        Ok(())
    }

    fn deserialize_json(&mut self, json: serde_json::Value) -> Result<(), serde_json::Error> {
        let value = serde_json::from_value(json)?;
        self.config = Some(value);
        Ok(())
    }

    fn serialize_config(&self) -> Option<toml::Value> {
        let value = self.config.clone();
        Some(toml::Value::try_from(value?).unwrap())
    }

    fn serialize_response(&self) -> SerializedSetting {
        let serialize = |t: Option<&T>| {
            let value = serde_json::to_value(t).unwrap();
            value
        };
        SerializedSetting {
            key: self.key(),
            require_restart: T::REQUIRE_RESTART,
            default_value: serialize(Some(&self.default)),
            config_value: serialize(self.config.as_ref()),
            cli_value: serialize(self.cli.as_ref()),
            env_value: serialize(self.env.as_ref()),
        }
    }

    fn customized_value(&self) -> &dyn Any {
        self.customized()
    }

    fn config_mut(&mut self) -> &mut dyn Any {
        &mut self.config
    }

    fn cli_mut(&mut self) -> &mut dyn Any {
        &mut self.cli
    }

    fn reset_config_value(&mut self) {
        self.config = None;
    }
}

pub static CONFIG: LazyLock<ConfigStore> = LazyLock::new(ConfigStore::construct);

#[derive(Clone)]
pub struct ConfigStore {
    settings: watch::Sender<HashMap<TypeId, Box<dyn AnySettingValue>>>,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore").finish()
    }
}

impl ConfigStore {
    pub fn construct() -> Self {
        let store = Self::new();

        store.register_value::<Port>();
        store.register_value::<HwAccel>();
        store.register_value::<ShowFolders>();
        store.register_value::<MovieFolders>();
        store.register_value::<FFmpegPath>();
        store.register_value::<FFprobePath>();
        store.register_value::<TmdbKey>();
        store.register_value::<TvdbKey>();
        store.register_value::<IntroMinDuration>();
        store.register_value::<IntroDetectionFfmpegBuild>();
        store.register_value::<WebUiPath>();

        store
    }

    pub fn new() -> Self {
        let (settings_tx, _) = watch::channel(HashMap::new());
        Self {
            settings: settings_tx,
        }
    }

    pub fn register_value<T: ConfigValue>(&self) {
        let default = T::default();
        self.settings.send_modify(|setting| {
            setting.insert(TypeId::of::<T>(), Box::new(SettingValue::new(default)));
        });
    }

    pub fn get_value<T: ConfigValue>(&self) -> T {
        let settings = self.settings.borrow();
        let setting = settings
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()));
        let t: &T = setting.customized_value().downcast_ref().unwrap();
        t.clone()
    }

    pub fn update_value<T: ConfigValue>(&self, new: T) {
        self.settings.send_modify(|settings| {
            let setting = settings
                .get_mut(&TypeId::of::<T>())
                .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()));
            let value = setting.config_mut();
            let value = value.downcast_mut().unwrap();
            *value = Some(new);
        });
    }

    pub fn construct_table(&self) -> toml::Table {
        let mut table = toml::Table::new();
        let settings = self.settings.borrow();
        for setting in settings.values() {
            let Some(value) = setting.serialize_config() else {
                continue;
            };
            table.insert(setting.key(), value);
        }
        table
    }

    pub fn json(&self) -> Vec<SerializedSetting> {
        let settings = self.settings.borrow();
        let mut out = Vec::with_capacity(settings.len());
        for setting in settings.values() {
            // CHANGE FROM ARRAY TO SETTINGS OBJECT?
            let value = setting.serialize_response();
            out.push(value);
        }
        out
    }

    pub fn apply_toml_settings(&self, table: toml::Table) {
        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                let key = setting.key();
                if let Some(val) = table.get(&key).cloned() {
                    if let Err(err) = setting.deserialize_toml(val) {
                        tracing::warn!(
                            "Failed to deserialize toml value for {}: {err}",
                            setting.type_name()
                        )
                    };
                }
            }
        });
    }

    pub fn apply_json(&self, value: serde_json::Value) -> anyhow::Result<ConfigurationApplyResult> {
        let mut result = ConfigurationApplyResult::default();
        let value = serde_json::to_value(&value)?;
        let obj = match value {
            serde_json::Value::Object(obj) => obj,
            _ => return Err(anyhow::anyhow!("Provided json must be object")),
        };

        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                if let Some(val) = obj.get(&setting.key()).cloned() {
                    match setting.deserialize_json(val) {
                        Ok(_) if setting.require_restart() => result.require_restart = true,
                        Err(err) => {
                            tracing::warn!(
                                "Failed to deserialize json value for {}: {err}",
                                setting.type_name()
                            );
                            result.errors.push(ConfigurationApplyError {
                                key: setting.key(),
                                message: err.to_string(),
                            });
                        }
                        _ => (),
                    };
                }
            }
        });
        Ok(result)
    }

    pub fn apply_config_value<T: ConfigValue>(&self, value: T) {
        self.settings.send_modify(|settings| {
            let setting = settings.get_mut(&value.type_id()).unwrap();
            let setting = setting.config_mut();
            let val = setting.downcast_mut().unwrap();
            *val = Some(value);
        });
    }

    pub fn apply_cli_value<T: ConfigValue>(&self, value: T) {
        self.settings.send_modify(|settings| {
            let setting = settings.get_mut(&value.type_id()).unwrap();
            let setting = setting.cli_mut();
            let val = setting.downcast_mut().unwrap();
            *val = Some(value);
        });
    }

    pub fn reset_config_values(&self) {
        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                setting.reset_config_value();
            }
        });
    }

    pub fn watch_value<T: ConfigValue>(&self) -> ConfigValueWatcher<T> {
        let rx = self.settings.subscribe();
        let current_value = self.get_value::<T>();
        ConfigValueWatcher {
            current_value,
            t_id: std::any::TypeId::of::<T>(),
            rx,
        }
    }
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ConfigValueWatcher<T> {
    rx: watch::Receiver<HashMap<TypeId, Box<dyn AnySettingValue>>>,
    current_value: T,
    t_id: std::any::TypeId,
}

impl<T: ConfigValue + PartialEq> ConfigValueWatcher<T> {
    /// Future resolves with the new value when it changes
    /// Cancellation safe
    pub async fn watch_change(&mut self) -> T {
        let changed_config = self
            .rx
            .wait_for(|map| {
                let val = map.get(&self.t_id).expect("config values be registered");
                let new = val.customized_value().downcast_ref::<T>().unwrap();
                *new != self.current_value
            })
            .await
            .expect("config is static so channel is never dropped");
        let new_value = changed_config
            .get(&self.t_id)
            .unwrap()
            .customized_value()
            .downcast_ref::<T>()
            .unwrap()
            .clone();
        self.current_value = new_value.clone();
        new_value
    }
}

// Shady utoipa manual implementation

impl<T: ConfigValue> utoipa::ToSchema<'static> for UtoipaConfigValue<T> {
    fn schema() -> (
        &'static str,
        utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
    ) {
        use utoipa::openapi::schema;
        use utoipa::PartialSchema;
        let (name, inner_schema) = T::schema();
        let snake_name = camel_to_snake_case(name);
        let optional: RefOr<utoipa::openapi::Schema> = match &inner_schema {
            RefOr::T(schema::Schema::Object(obj)) => {
                let mut obj = obj.clone();
                obj.nullable = true;
                obj.into()
            }
            RefOr::T(schema::Schema::Array(obj)) => {
                let mut obj = obj.clone();
                obj.nullable = true;
                obj.into()
            }
            RefOr::T(schema) => match schema {
                schema::Schema::Array(_) => panic!("Can't handle array schema type"),
                schema::Schema::Object(_) => panic!("Can't handle object schema type"),
                schema::Schema::OneOf(_) => panic!("Can't handle one_of schema type"),
                schema::Schema::AllOf(_) => panic!("Can't handle all_of schema type"),
                schema::Schema::AnyOf(_) => panic!("Can't handle any_of schema type"),
                _ => panic!("Can't handle other schema type"),
            },
            RefOr::Ref(r) => panic!("Can't ref type: {}", r.ref_location),
        };
        let key = T::KEY.unwrap_or(&snake_name);
        let key_schema = schema::ObjectBuilder::new()
            .nullable(false)
            .schema_type(schema::SchemaType::String)
            .enum_values(Some([key]));

        let out = schema::ObjectBuilder::new()
            .schema_type(schema::SchemaType::Object)
            .nullable(false)
            .property("require_restart", bool::schema())
            .required("require_restart")
            .property("key", key_schema)
            .required("key")
            .property("default_value", inner_schema.clone())
            .required("default_value")
            .property("config_value", optional.clone())
            .required("config_value")
            .property("cli_value", optional.clone())
            .required("cli_value")
            .property("env_value", optional)
            .required("env_value")
            .into();
        (name, out)
    }
}

impl utoipa::ToSchema<'static> for UtoipaConfigSchema {
    fn schema() -> (
        &'static str,
        utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
    ) {
        use utoipa::openapi::schema;
        let schema = schema::OneOfBuilder::new()
            .item(UtoipaConfigValue::<Port>::schema().1)
            .item(UtoipaConfigValue::<ShowFolders>::schema().1)
            .item(UtoipaConfigValue::<MovieFolders>::schema().1)
            .item(UtoipaConfigValue::<TmdbKey>::schema().1)
            .item(UtoipaConfigValue::<TvdbKey>::schema().1)
            .item(UtoipaConfigValue::<FFmpegPath>::schema().1)
            .item(UtoipaConfigValue::<FFprobePath>::schema().1)
            .item(UtoipaConfigValue::<HwAccel>::schema().1)
            .item(UtoipaConfigValue::<IntroMinDuration>::schema().1)
            .item(UtoipaConfigValue::<IntroDetectionFfmpegBuild>::schema().1)
            .item(UtoipaConfigValue::<WebUiPath>::schema().1);
        let array = schema::ArrayBuilder::new().items(schema).build();
        ("UtoipaConfigSchema", array.into())
    }
}

#[derive(Debug)]
#[allow(unused)]
pub struct UtoipaConfigValue<T> {
    _t: std::marker::PhantomData<T>,
}

#[derive(Debug)]
pub struct UtoipaConfigSchema;

// Settings

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct Port(pub u16);

impl AsRef<u16> for Port {
    fn as_ref(&self) -> &u16 {
        &self.0
    }
}

impl Default for Port {
    fn default() -> Self {
        Self(6969)
    }
}

impl ConfigValue for Port {
    const REQUIRE_RESTART: bool = true;
}

#[derive(Deserialize, Clone, Copy, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct HwAccel(pub bool);
impl ConfigValue for HwAccel {}

impl AsRef<bool> for HwAccel {
    fn as_ref(&self) -> &bool {
        &self.0
    }
}

#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = Vec<String>)]
pub struct MovieFolders(pub Vec<PathBuf>);
impl ConfigValue for MovieFolders {}

impl AsRef<[PathBuf]> for MovieFolders {
    fn as_ref(&self) -> &[PathBuf] {
        &self.0
    }
}

impl MovieFolders {
    pub fn add(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        if !self.0.contains(&path) {
            self.0.push(path);
        }
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.0.retain(|p| p != path)
    }

    pub fn first(&self) -> Option<&PathBuf> {
        self.0.first()
    }

    pub fn existing(&self) -> Vec<&PathBuf> {
        self.0
            .iter()
            .filter(|path| {
                let exists = path.try_exists().unwrap_or(false);
                if !exists {
                    tracing::warn!(
                        "Failed to check existence for movie directory: {}",
                        path.display()
                    );
                }
                exists
            })
            .collect()
    }
}

#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = Vec<String>)]
pub struct ShowFolders(pub Vec<PathBuf>);
impl ConfigValue for ShowFolders {}

impl AsRef<[PathBuf]> for ShowFolders {
    fn as_ref(&self) -> &[PathBuf] {
        &self.0
    }
}
impl ShowFolders {
    pub fn add(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        if !self.0.contains(&path) {
            self.0.push(path);
        }
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.0.retain(|p| p != path)
    }

    pub fn first(&self) -> Option<&PathBuf> {
        self.0.first()
    }

    pub fn existing(&self) -> Vec<&PathBuf> {
        self.0
            .iter()
            .filter(|path| {
                let exists = path.try_exists().unwrap_or(false);
                if !exists {
                    tracing::warn!(
                        "Failed to check existence for show directory: {}",
                        path.display()
                    );
                }
                exists
            })
            .collect()
    }
}

#[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct FFmpegPath(PathBuf);
impl ConfigValue for FFmpegPath {
    const KEY: Option<&str> = Some("ffmpeg_path");
}

impl Default for FFmpegPath {
    fn default() -> Self {
        Self(PathBuf::from("ffmpeg"))
    }
}

impl AsRef<Path> for FFmpegPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct FFprobePath(PathBuf);
impl ConfigValue for FFprobePath {
    const KEY: Option<&str> = Some("ffprobe_path");
}

impl Default for FFprobePath {
    fn default() -> Self {
        Self(PathBuf::from("ffprobe"))
    }
}

impl AsRef<Path> for FFprobePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct TmdbKey(pub Option<String>);
impl ConfigValue for TmdbKey {
    const ENV_KEY: Option<&str> = Some("TMDB_TOKEN");
}

impl AsRef<Option<String>> for TmdbKey {
    fn as_ref(&self) -> &Option<String> {
        &self.0
    }
}

#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct TvdbKey(pub Option<String>);
impl ConfigValue for TvdbKey {
    const ENV_KEY: Option<&str> = Some("TVDB_TOKEN");
}

impl AsRef<Option<String>> for TvdbKey {
    fn as_ref(&self) -> &Option<String> {
        &self.0
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
/// Minimal intro duration from seconds
pub struct IntroMinDuration(pub usize);
impl ConfigValue for IntroMinDuration {}
impl Default for IntroMinDuration {
    fn default() -> Self {
        Self(20)
    }
}

/// Path to FFmpeg build that supports chromparint audio fingerprinting
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct IntroDetectionFfmpegBuild(pub PathBuf);
impl ConfigValue for IntroDetectionFfmpegBuild {}
impl Default for IntroDetectionFfmpegBuild {
    fn default() -> Self {
        Self(PathBuf::from("ffmpeg"))
    }
}

/// Path to Web UI dist directory
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct WebUiPath(pub PathBuf);
impl ConfigValue for WebUiPath {
    const REQUIRE_RESTART: bool = true;
}
impl Default for WebUiPath {
    fn default() -> Self {
        let base_path = &APP_RESOURCES
            .get()
            .expect("APP_RESOURCES initiated before first CONFIG access")
            .base_path;
        Self(base_path.join("dist"))
    }
}

#[cfg(test)]
mod tests {

    use super::{ConfigStore, HwAccel, Port};

    const TEST_TOML_CONFIG: &str = r#"
port = 8000
hw_accel = true
    "#;

    #[test]
    fn setting_store() {
        let store = ConfigStore::construct();
        let mut port = Port::default();
        let stored_port: Port = store.get_value();
        assert_eq!(port, stored_port);
        port = Port(8000);
        store.update_value(port);
        let stored_port: Port = store.get_value();
        assert_eq!(port, stored_port);
    }

    #[test]
    fn apply_settings() {
        let store = ConfigStore::construct();
        let port: Port = store.get_value();
        let hw_accel: HwAccel = store.get_value();
        assert_eq!(port.0, Port::default().0);
        assert_eq!(hw_accel.0, HwAccel::default().0);
        let toml = toml::from_str(TEST_TOML_CONFIG).unwrap();
        store.apply_toml_settings(toml);
        let port: Port = store.get_value();
        let hw_accel: HwAccel = store.get_value();
        assert_eq!(port.0, 8000);
        assert_eq!(hw_accel.0, true);
    }
}

#[derive(Debug)]
pub struct ConfigFile(pub fs::File);

impl ConfigFile {
    pub async fn open(config_path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        if let Some(parent) = config_path.as_ref().parent() {
            fs::create_dir_all(parent).await?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&config_path)
            .await?;
        tracing::debug!("Opened config file {}", config_path.as_ref().display());
        Ok(Self(file))
    }

    /// Read config file
    pub async fn read(&mut self) -> Result<toml::Table, anyhow::Error> {
        let mut raw = String::new();
        let read = self.0.read_to_string(&mut raw).await?;
        tracing::debug!("Read {read} bytes from config file");
        let table: toml::Table = toml::from_str(&raw)?;
        Ok(table)
    }

    /// Write config file
    pub async fn write_toml(&mut self, table: toml::Table) -> Result<(), anyhow::Error> {
        self.0.set_len(0).await?;
        let raw = toml::to_string_pretty(&table)?;
        self.0.write_all(raw.as_bytes()).await?;
        Ok(())
    }
}

#[derive(Debug, Parser, Deserialize, Serialize)]
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

impl Args {
    pub fn apply_configuration(&self) {
        if let Some(port) = self.port {
            CONFIG.apply_cli_value(Port(port));
        }
        if let Some(token) = self.tmdb_token.clone() {
            CONFIG.apply_cli_value(TmdbKey(Some(token)));
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Codec {
    pub codec_type: CodecType,
    pub name: String,
    pub long_name: String,
    pub decode_supported: bool,
    pub encode_supported: bool,
}

impl Codec {
    pub fn from_capability_line(line: String) -> Self {
        let mut split = line.split_terminator(' ').filter(|chunk| !chunk.is_empty());
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct Capabilities {
    pub codecs: Vec<Codec>,
    pub ffmpeg_version: String,
    pub chromaprint_enabled: bool,
}

impl Capabilities {
    pub async fn parse() -> Result<Self, anyhow::Error> {
        let ffmpeg: FFmpegPath = CONFIG.get_value();
        let chromaprint_ffmpeg: IntroDetectionFfmpegBuild = CONFIG.get_value();
        let output = Command::new(ffmpeg.as_ref())
            .args(["-codecs"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("ffmpeg -codces command failed"));
        }

        let lines = output.stdout.lines();

        // skip description header
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

        let mut lines = output.stderr.lines();
        let version_line = lines.next().context("version line")??;
        let _build_line = lines.next();
        let configuration_line = lines.next().context("configuration line")??;

        let version = version_line.split_ascii_whitespace().nth(2).unwrap();
        let chromaprint_enabled = if ffmpeg.0 == chromaprint_ffmpeg.0 {
            configuration_line
                .split_ascii_whitespace()
                .skip(1)
                .any(|flag| flag == "--enable-chromaprint")
        } else {
            let out = Command::new(chromaprint_ffmpeg.0)
                .arg("-version")
                .output()
                .await?;
            let mut lines = out.stdout.lines();
            let _ = lines.next().context("version line")??;
            let _ = lines.next();
            let configuration_line = lines.next().context("configuration line")??;
            configuration_line
                .split_ascii_whitespace()
                .skip(1)
                .any(|flag| flag == "--enable-chromaprint")
        };
        Ok(Self {
            codecs,
            ffmpeg_version: version.to_string(),
            chromaprint_enabled,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AppResources {
    #[schema(value_type = String)]
    pub database_path: PathBuf,
    #[schema(value_type = String)]
    #[serde(skip)]
    pub config_path: PathBuf,
    #[schema(value_type = String)]
    pub resources_path: PathBuf,
    #[schema(value_type = String)]
    pub temp_path: PathBuf,
    #[schema(value_type = String)]
    pub cache_path: PathBuf,
    #[schema(value_type = Option<String>)]
    pub binary_path: Option<PathBuf>,
    #[schema(value_type = String)]
    pub base_path: PathBuf,
    #[schema(value_type = String)]
    pub log_path: PathBuf,
}

pub static APP_RESOURCES: OnceLock<AppResources> = OnceLock::new();

impl AppResources {
    pub const APP_NAME: &'static str = "media-server";

    fn prod_storage() -> PathBuf {
        dirs::data_local_dir()
            .expect("target to have data directory")
            .join(Self::APP_NAME)
    }

    fn debug_storage() -> PathBuf {
        PathBuf::from(".").canonicalize().unwrap()
    }

    fn data_storage() -> PathBuf {
        if Self::is_prod() {
            Self::prod_storage()
        } else {
            Self::debug_storage()
        }
    }

    pub fn is_prod() -> bool {
        !cfg!(debug_assertions)
    }

    pub fn default_config_path() -> PathBuf {
        Self::data_storage().join("configuration.toml")
    }

    fn temp_storage() -> PathBuf {
        Self::data_storage().join("tmp")
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

    pub fn log() -> PathBuf {
        Self::data_storage().join("log.log")
    }

    pub fn initiate() -> Result<(), std::io::Error> {
        use std::fs;
        fs::create_dir_all(Self::resources())?;
        fs::create_dir_all(Self::database_directory())?;
        fs::create_dir_all(Self::temp_storage())?;
        fs::create_dir_all(Self::cache_storage())?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::database())?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::log())?;
        Ok(())
    }

    pub fn new(config_path: PathBuf) -> Self {
        let resources_path = Self::resources();
        let database_path = Self::database();
        let temp_path = Self::temp_storage();
        let cache_path = Self::cache_storage();
        let log_path = Self::log();
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
            binary_path,
            base_path,
            log_path,
        }
    }
}

impl Default for AppResources {
    fn default() -> Self {
        let config_path = Self::default_config_path();
        Self::new(config_path)
    }
}

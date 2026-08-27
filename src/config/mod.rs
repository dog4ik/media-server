use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    metadata::{self, MetadataProvider},
    torrent_index::TorrentIndexIdentifier,
};

mod capabilities;
mod cli_args;
mod config_file;
mod resources;
/// Storage, layering and serialization machinery behind every setting below
mod store;

pub use capabilities::Capabilities;
pub use cli_args::Args;
pub use config_file::ConfigFile;
pub use resources::{APP_RESOURCES, AppResources};
pub use store::{
    CONFIG, ConfigurationApplyError, ConfigurationApplyResult, SerializedSetting,
    UtoipaConfigSchema,
};

use store::{ConfigStore, ConfigValue, UtoipaConfigValue};

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
        store.register_value::<ProvodKey>();
        store.register_value::<ProvodUrl>();
        store.register_value::<OtelEndpoint>();
        store.register_value::<IntroMinDuration>();
        store.register_value::<IntroDetectionFfmpegBuild>();
        store.register_value::<WebUiPath>();
        store.register_value::<ShowProvidersOrder>();
        store.register_value::<MovieProvidersOrder>();
        store.register_value::<DiscoverProvidersOrder>();
        store.register_value::<TorrentIndexesOrder>();
        store.register_value::<UpnpEnabled>();
        store.register_value::<UpnpTtl>();
        store.register_value::<MetadataLanguage>();
        store.register_value::<scan::MaxMovieConcurrency>();
        store.register_value::<scan::MaxShowConcurrency>();
        store.register_value::<scan::MaxAssetConcurrency>();
        store.register_value::<scan::UseSeasonEpisodes>();

        store
    }
}

impl utoipa::PartialSchema for UtoipaConfigSchema {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema;
        let schema = schema::OneOfBuilder::new()
            .item(UtoipaConfigValue::<Port>::schema())
            .item(UtoipaConfigValue::<ShowFolders>::schema())
            .item(UtoipaConfigValue::<MovieFolders>::schema())
            .item(UtoipaConfigValue::<TmdbKey>::schema())
            .item(UtoipaConfigValue::<TvdbKey>::schema())
            .item(UtoipaConfigValue::<ProvodUrl>::schema())
            .item(UtoipaConfigValue::<OtelEndpoint>::schema())
            .item(UtoipaConfigValue::<ProvodKey>::schema())
            .item(UtoipaConfigValue::<FFmpegPath>::schema())
            .item(UtoipaConfigValue::<FFprobePath>::schema())
            .item(UtoipaConfigValue::<HwAccel>::schema())
            .item(UtoipaConfigValue::<IntroMinDuration>::schema())
            .item(UtoipaConfigValue::<IntroDetectionFfmpegBuild>::schema())
            .item(UtoipaConfigValue::<WebUiPath>::schema())
            .item(UtoipaConfigValue::<UpnpEnabled>::schema())
            .item(UtoipaConfigValue::<UpnpTtl>::schema())
            .item(UtoipaConfigValue::<scan::MaxMovieConcurrency>::schema())
            .item(UtoipaConfigValue::<scan::MaxShowConcurrency>::schema())
            .item(UtoipaConfigValue::<scan::MaxAssetConcurrency>::schema())
            .item(UtoipaConfigValue::<scan::UseSeasonEpisodes>::schema())
            .item(UtoipaConfigValue::<MetadataLanguage>::schema());
        let array = schema::ArrayBuilder::new().items(schema).build();
        array.into()
    }
}

// Settings

/// The network port on which the server listens for incoming connections
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

/// Enable hardware acceleration to significantly improve transcoding performance, if supported by the system
#[derive(Deserialize, Clone, Copy, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct HwAccel(pub bool);
impl ConfigValue for HwAccel {}

impl AsRef<bool> for HwAccel {
    fn as_ref(&self) -> &bool {
        &self.0
    }
}

/// List of directories that contain movie files. All movie files from these directories will show up in the library
#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = Vec<String>)]
pub struct MovieFolders(pub Vec<PathBuf>);
impl ConfigValue for MovieFolders {}

impl AsRef<[PathBuf]> for MovieFolders {
    fn as_ref(&self) -> &[PathBuf] {
        &self.0
    }
}

/// List of directories that contain show files. All episode files from these directories will show up in the library
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
}

/// Path to ffmpeg binary. This ffmpeg binary will be used for media transcoding tasks
#[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct FFmpegPath(pub PathBuf);
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

/// Path to ffprobe binary. This setting will be deprecated in favor of ffmpeg abi
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

/// API key for TMDB. Allows server to authenticate with TMDB metadata provider
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

/// API key for Provod agent. Allows server to authenticate with Provod proxy server
#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct ProvodKey(pub Option<String>);
impl ConfigValue for ProvodKey {
    const ENV_KEY: Option<&str> = Some("PROVOD_TOKEN");
}

/// Url of Provod agent.
#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct ProvodUrl(pub Option<String>);
impl ConfigValue for ProvodUrl {}

/// OTLP endpoint for OpenTelemetry export (traces + metrics), e.g.
/// `http://localhost:4317`. When unset, OpenTelemetry is disabled.
#[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
pub struct OtelEndpoint(pub Option<String>);
impl ConfigValue for OtelEndpoint {
    const ENV_KEY: Option<&str> = Some("OTEL_EXPORTER_OTLP_ENDPOINT");
    const REQUIRE_RESTART: bool = true;
}

/// Settings related to the library scanning process
pub mod scan {
    use super::*;
    use serde::{Deserialize, Serialize};

    /// Try to use episodes metadata from fetched season.
    /// It will speed up metadata fetch for newly added season, but episodes will end up with potentially incomplete metadata
    ///
    /// Not recommended unless you have a huge show library you scan at once
    #[derive(Deserialize, Clone, Default, Serialize, Debug, utoipa::ToSchema)]
    pub struct UseSeasonEpisodes(pub bool);
    impl ConfigValue for UseSeasonEpisodes {}

    /// The amount of movies allowed to be fetched concurrently
    #[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
    pub struct MaxMovieConcurrency(pub usize);
    impl Default for MaxMovieConcurrency {
        fn default() -> Self {
            Self(8)
        }
    }
    impl ConfigValue for MaxMovieConcurrency {}

    /// The amount of shows allowed to be fetched concurrently
    #[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
    pub struct MaxShowConcurrency(pub usize);
    impl Default for MaxShowConcurrency {
        fn default() -> Self {
            Self(4)
        }
    }
    impl ConfigValue for MaxShowConcurrency {}

    /// The amount of assets allowed to be fetched concurrently
    #[derive(Deserialize, Clone, Serialize, Debug, utoipa::ToSchema)]
    pub struct MaxAssetConcurrency(pub usize);
    impl Default for MaxAssetConcurrency {
        fn default() -> Self {
            Self(32)
        }
    }
    impl ConfigValue for MaxAssetConcurrency {}
}

/// API key for TVDB. Allows server to authenticate with TVDB metadata provider
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

/// Minimal intro duration in seconds. With very low values things like netflix logo will be considered as intro
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
pub struct IntroMinDuration(pub usize);
impl ConfigValue for IntroMinDuration {}
impl Default for IntroMinDuration {
    fn default() -> Self {
        Self(20)
    }
}

/// Path to the FFmpeg build that supports Chromaprint. Required for intro detection feature to work
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct IntroDetectionFfmpegBuild(pub PathBuf);
impl ConfigValue for IntroDetectionFfmpegBuild {}
impl Default for IntroDetectionFfmpegBuild {
    fn default() -> Self {
        Self(PathBuf::from("ffmpeg"))
    }
}

/// Path to Web UI assets, useful when Web UI located in a separate directory
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
#[schema(value_type = String)]
pub struct WebUiPath(pub PathBuf);
impl ConfigValue for WebUiPath {
    const REQUIRE_RESTART: bool = true;
}
impl Default for WebUiPath {
    fn default() -> Self {
        Self(APP_RESOURCES.statics_path.join("dist"))
    }
}

/// Enable SSDP (Simple Service Discovery Protocol) for UPnP. This allows the server to be discovered on the local network by compatible devices
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq, Debug, utoipa::ToSchema, Default)]
pub struct UpnpEnabled(pub bool);
impl ConfigValue for UpnpEnabled {}

/// Amount of ip routing "hops" for SSDP packet.
#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, utoipa::ToSchema)]
pub struct UpnpTtl(pub u32);
impl ConfigValue for UpnpTtl {}
impl Default for UpnpTtl {
    fn default() -> Self {
        Self(upnp::ssdp::DEFAULT_SSDP_TTL)
    }
}

/// Discover metadata providers order
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
pub struct DiscoverProvidersOrder(pub Vec<MetadataProvider>);
impl ConfigValue for DiscoverProvidersOrder {}
impl Default for DiscoverProvidersOrder {
    fn default() -> Self {
        Self(vec![
            MetadataProvider::Local,
            MetadataProvider::Tmdb,
            MetadataProvider::Tvdb,
        ])
    }
}

/// Show metadata providers order
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
pub struct ShowProvidersOrder(pub Vec<MetadataProvider>);
impl ConfigValue for ShowProvidersOrder {}
impl Default for ShowProvidersOrder {
    fn default() -> Self {
        Self(vec![
            MetadataProvider::Local,
            MetadataProvider::Tmdb,
            MetadataProvider::Tvdb,
        ])
    }
}

/// Movie metadata providers order
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
pub struct MovieProvidersOrder(pub Vec<MetadataProvider>);
impl ConfigValue for MovieProvidersOrder {}
impl Default for MovieProvidersOrder {
    fn default() -> Self {
        Self(vec![
            MetadataProvider::Local,
            MetadataProvider::Tmdb,
            MetadataProvider::Tvdb,
        ])
    }
}

/// Torrent indexes providers order
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema)]
pub struct TorrentIndexesOrder(pub Vec<TorrentIndexIdentifier>);
impl ConfigValue for TorrentIndexesOrder {}
impl Default for TorrentIndexesOrder {
    fn default() -> Self {
        Self(vec![TorrentIndexIdentifier::Tpb])
    }
}

/// Language to fetch metadata in. Selected language will be used in names, plots and posters
#[derive(Deserialize, Serialize, Clone, Debug, utoipa::ToSchema, Default)]
pub struct MetadataLanguage(pub metadata::Language);
impl ConfigValue for MetadataLanguage {}

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
        assert!(hw_accel.0);
    }

    #[test]
    fn unset_setting() {
        let store = ConfigStore::construct();
        let port: Port = store.get_value();
        assert_eq!(port.0, Port::default().0);
        let config_set = serde_json::json!({ "port": 7355 });
        store.apply_json(config_set).unwrap();
        let port: Port = store.get_value();
        assert_eq!(port.0, 7355);
        let config_unset = serde_json::json!({"port": null });
        store.apply_json(config_unset).unwrap();
        let port: Port = store.get_value();
        assert_eq!(port.0, Port::default().0);
    }
}

use std::path::Path;

use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

use super::APP_RESOURCES;

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

    /// Open and read config file dropping file handle.
    pub async fn open_and_read() -> anyhow::Result<toml::Table> {
        let mut config = Self::open(&APP_RESOURCES.config_path).await?;
        config.read().await
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

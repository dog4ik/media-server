use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum DeviceType {
    MediaServer,
    MediaRenderer,
    Printer,
    Other(String),
}

impl From<&str> for DeviceType {
    fn from(value: &str) -> DeviceType {
        match value {
            "MediaServer" => DeviceType::MediaServer,
            "MediaRenderer" => DeviceType::MediaRenderer,
            "Printer" => DeviceType::Printer,
            _ => DeviceType::Other(value.to_string()),
        }
    }
}

impl Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            DeviceType::MediaServer => "MediaServer",
            DeviceType::MediaRenderer => "MediaRenderer",
            DeviceType::Printer => "Printer",
            DeviceType::Other(other) => other,
        };
        write!(f, "{name}")
    }
}

#[derive(Debug, Clone)]
pub enum ServiceType {
    ContentDirectory,
    AVTransport,
    RenderingControl,
    ConnectionManager,
    Printer,
    Other(String),
}

impl Display for ServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ServiceType::ContentDirectory => "ContentDirectory",
            ServiceType::AVTransport => "AVTransport",
            ServiceType::RenderingControl => "RenderingControl",
            ServiceType::ConnectionManager => "ConnectionManager",
            ServiceType::Printer => "Printer",
            ServiceType::Other(other) => other,
        };
        write!(f, "{name}")
    }
}

impl From<&str> for ServiceType {
    fn from(value: &str) -> ServiceType {
        match value {
            "ContentDirectory" => ServiceType::ContentDirectory,
            "AVTransport" => ServiceType::AVTransport,
            "RenderingControl" => ServiceType::RenderingControl,
            "ConnectionManager" => ServiceType::ConnectionManager,
            "Printer" => ServiceType::Printer,
            other => ServiceType::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum UrnType {
    Device(DeviceType),
    Service(ServiceType),
}

#[derive(Debug, Clone)]
/// Uniform Resource Name. Provides a unique and persistent identifier for a resource.
pub struct URN {
    pub version: u8,
    pub urn_type: UrnType,
}

impl Serialize for URN {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl URN {
    pub fn media_server() -> Self {
        Self {
            version: 1,
            urn_type: UrnType::Device(DeviceType::MediaServer),
        }
    }
}

impl Display for URN {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (urn_type, name) = match &self.urn_type {
            UrnType::Device(device) => ("device", device.to_string()),
            UrnType::Service(service) => ("service", service.to_string()),
        };

        write!(
            f,
            "urn:schemas-upnp-org:{urn_type}:{name}:{version}",
            version = self.version
        )
    }
}

impl FromStr for URN {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(5, ':');
        let urn = parts.next().context("urn prefix")?;
        anyhow::ensure!(urn == "urn");
        let _schema = parts.next().context("schema")?;
        let schema_type = parts.next().context("schema_type")?;
        let name = parts.next().context("service/device name")?;
        let version = parts.next().context("service/device version")?.parse()?;
        let urn_type = match schema_type {
            "device" => UrnType::Device(DeviceType::from(name)),
            "service" => UrnType::Service(ServiceType::from(name)),
            rest => return Err(anyhow::anyhow!("unknown device type: {rest}")),
        };
        Ok(URN { version, urn_type })
    }
}

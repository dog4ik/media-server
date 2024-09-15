use std::fmt::Display;

use anyhow::Context;

#[derive(Debug, Clone, Copy)]
pub enum DeviceType<'a> {
    MediaServer,
    MediaRenderer,
    Printer,
    Other(&'a str),
}

impl<'a> From<&'a str> for DeviceType<'a> {
    fn from(value: &'a str) -> DeviceType<'a> {
        match value {
            "MediaServer" => DeviceType::MediaServer,
            "MediaRenderer" => DeviceType::MediaRenderer,
            "Printer" => DeviceType::Printer,
            other => DeviceType::Other(other),
        }
    }
}

impl Display for DeviceType<'_> {
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

#[derive(Debug, Clone, Copy)]
pub enum ServiceType<'a> {
    ContentDirectory,
    AVTransport,
    RenderingControl,
    ConnectionManager,
    Printer,
    Other(&'a str),
}

impl Display for ServiceType<'_> {
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

impl<'a> From<&'a str> for ServiceType<'a> {
    fn from(value: &'a str) -> ServiceType<'a> {
        match value {
            "ContentDirectory" => ServiceType::ContentDirectory,
            "AVTransport" => ServiceType::AVTransport,
            "RenderingControl" => ServiceType::RenderingControl,
            "ConnectionManager" => ServiceType::ConnectionManager,
            "Printer" => ServiceType::Printer,
            other => ServiceType::Other(other),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum UrnType<'a> {
    Device(DeviceType<'a>),
    Service(ServiceType<'a>),
}

#[derive(Debug, Clone, Copy)]
/// Uniform Resource Name. Provides a unique and persistent identifier for a resource.
pub struct URN<'a> {
    pub version: u8,
    pub urn_type: UrnType<'a>,
}

impl<'a> URN<'a> {
    pub fn media_server() -> Self {
        Self {
            version: 1,
            urn_type: UrnType::Device(DeviceType::MediaServer),
        }
    }
}

impl Display for URN<'_> {
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

impl<'a> TryFrom<&'a str> for URN<'a> {
    type Error = anyhow::Error;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
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

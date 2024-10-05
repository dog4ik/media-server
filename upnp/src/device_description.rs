use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::{templates::SpecVersion, SERVER_UUID};

#[derive(Debug, Serialize)]
pub struct DeviceDescription<'a> {
    #[serde(rename = "@xmlns")]
    pub xmlns: &'a str,
    #[serde(rename = "@xmlns:dlna")]
    pub xmlns_dlna: &'a str,
    #[serde(rename = "@configId")]
    pub config_id: &'a str,
    #[serde(rename = "specVersion")]
    pub spec_version: SpecVersion,
    pub device: Device<'a>,
}

impl DeviceDescription<'_> {
    pub fn new(friendly_name: String) -> Self {
        Self {
            xmlns: "urn:schemas-upnp-org:device-1-0",
            xmlns_dlna: "urn:schemas-dlna-org:device-1-0",
            config_id: "1",
            spec_version: SpecVersion::upnp_v2(),
            device: Device {
                device_type: "urn:schemas-upnp-org:device:MediaServer:1",
                friendly_name,
                manufacturer: "media-server",
                manufacturer_url: Some("https://github.com/dog4ik"),
                model_description: Some("The media server"),
                model_name: "Media server",
                model_number: Some("1.0"),
                model_url: Some("https://github.com/dog4ik/media-server"),
                serial_number: None,
                udn: UDN::new(SERVER_UUID),
                dlna_x_dlnadoc: "urn:schemas-dlna-org:device-1-0",
                icon_list: IconList {
                    icon: vec![Icon {
                        mimetype: "image/webp",
                        width: 25,
                        height: 25,
                        depth: 25,
                        url: "/logo.webp",
                    }],
                },
                service_list: ServiceList {
                    service: vec![Service::content_directory()],
                },
            },
        }
    }
}

impl Default for DeviceDescription<'_> {
    fn default() -> Self {
        Self::new("Media server".into())
    }
}

#[derive(Debug, Serialize)]
//TODO: use types that can be serialized here
pub struct Device<'a> {
    #[serde(rename = "deviceType")]
    pub device_type: &'a str,
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    /// Manufacturer name. Should be < 64 characters.
    pub manufacturer: &'a str,
    #[serde(rename = "manufacturerURL")]
    pub manufacturer_url: Option<&'a str>,
    #[serde(rename = "modelDescription")]
    /// Should be < 128 characters
    pub model_description: Option<&'a str>,
    #[serde(rename = "modelName")]
    pub model_name: &'a str,
    #[serde(rename = "modelNumber")]
    pub model_number: Option<&'a str>,
    #[serde(rename = "modelURL")]
    pub model_url: Option<&'a str>,
    #[serde(rename = "serialNumber")]
    pub serial_number: Option<&'a str>,
    #[serde(rename = "UDN")]
    pub udn: UDN,
    #[serde(rename = "X_DLNADOC")]
    pub dlna_x_dlnadoc: &'a str,
    #[serde(rename = "iconList")]
    pub icon_list: IconList<'a>,
    #[serde(rename = "serviceList")]
    pub service_list: ServiceList<'a>,
}

/// Unique Device Name. Universally-unique identifier for the device, whether root or
/// embedded. shall be the same over time for a specific device instance (i.e., shall survive
/// reboots).
#[derive(Debug, Clone, Serialize)]
pub struct UDN(String);

impl UDN {
    pub fn new(uuid: uuid::Uuid) -> Self {
        Self(format!("uuid:{uuid}"))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Display for UDN {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for UDN {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = s
            .strip_prefix("uuid:")
            .context("udn should start with uuid:")?
            .parse()
            .context("parse uuid")?;
        Ok(Self::new(uuid))
    }
}

#[derive(Debug, Serialize)]
pub struct DlnaXDlnadoc {
    #[serde(rename = "@xmlns:dlna")]
    pub xmlns_dlna: String,
}

#[derive(Debug, Serialize)]
pub struct IconList<'a> {
    icon: Vec<Icon<'a>>,
}

#[derive(Debug, Serialize)]
pub struct ServiceList<'a> {
    service: Vec<Service<'a>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Icon<'a> {
    pub mimetype: &'a str,
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub url: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Service<'a> {
    #[serde(rename = "serviceType")]
    pub service_type: &'a str,
    /// URL for service description. Shall be relative to the URL at which the device description
    #[serde(rename = "serviceId")]
    pub service_id: &'a str,
    #[serde(rename = "SCPDURL")]
    pub scpdurl: &'a str,
    #[serde(rename = "controlURL")]
    pub control_url: &'a str,
    #[serde(rename = "eventSubURL")]
    pub event_sub_url: &'a str,
}

impl Service<'_> {
    const fn content_directory() -> Self {
        Service {
            service_type: "urn:schemas-upnp-org:service:ContentDirectory:1",
            service_id: "urn:upnp-org:serviceId:ContentDirectory",
            scpdurl: "/upnp/content_directory/scpd.xml",
            control_url: "/upnp/content_directory/control.xml",
            event_sub_url: "/upnp/content_directory/event.xml",
        }
    }
}

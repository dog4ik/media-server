use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};
use serde::{Deserialize, Serialize};

use crate::IntoXml;

use super::{templates::SpecVersion, SERVER_UUID};

#[derive(Debug)]
pub struct DeviceDescription<'a> {
    pub config_id: &'a str,
    pub spec_version: SpecVersion,
    pub device: Device<'a>,
}

impl DeviceDescription<'_> {
    pub fn into_xml(&self) -> anyhow::Result<String> {
        use quick_xml::Writer;
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", None, None)))?;
        let root = BytesStart::new("root").with_attributes([
            ("xmlns", "urn:schemas-upnp-org:device-1-0"),
            ("xmlns:dlna", "urn:schemas-dlna-org:device-1-0"),
            ("configId", self.config_id),
        ]);
        let root_end = root.to_end().into_owned();
        w.write_event(Event::Start(root))?;

        w.write_serializable("specVersion", &self.spec_version)?;
        self.device.write_xml(&mut w)?;

        w.write_event(Event::End(root_end))?;
        Ok(String::from_utf8(w.into_inner())?)
    }
}

impl DeviceDescription<'_> {
    pub fn new(friendly_name: String) -> Self {
        Self {
            config_id: "9999",
            spec_version: SpecVersion::upnp_v1_1(),
            device: Device {
                device_type: "urn:schemas-upnp-org:device:MediaServer:1",
                friendly_name,
                manufacturer: "media-server",
                serial_number: None,
                manufacturer_url: Some("https://github.com/dog4ik"),
                model_description: Some("The media server"),
                model_name: "Media server",
                model_number: Some("1.0"),
                model_url: Some("https://github.com/dog4ik/media-server"),
                udn: UDN::new(SERVER_UUID),
                icon_list: vec![
                    Icon {
                        mimetype: "image/webp",
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.webp",
                    },
                    Icon {
                        mimetype: "image/png",
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.png",
                    },
                    Icon {
                        mimetype: "image/jpeg",
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.jpeg",
                    },
                ],
                service_list: vec![Service::content_directory(), Service::connection_manager()],
            },
        }
    }
}

impl Default for DeviceDescription<'_> {
    fn default() -> Self {
        Self::new("Media server".into())
    }
}

#[derive(Debug)]
//TODO: use types that can be serialized here
pub struct Device<'a> {
    pub device_type: &'a str,
    pub friendly_name: String,
    /// Manufacturer name. Should be < 64 characters.
    pub manufacturer: &'a str,
    pub manufacturer_url: Option<&'a str>,
    /// Should be < 128 characters
    pub model_description: Option<&'a str>,
    pub model_name: &'a str,
    pub model_number: Option<&'a str>,
    pub model_url: Option<&'a str>,
    pub serial_number: Option<&'a str>,
    pub udn: UDN,
    pub icon_list: Vec<Icon<'a>>,
    pub service_list: Vec<Service<'a>>,
}

impl IntoXml for Device<'_> {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> quick_xml::Result<()> {
        let device = BytesStart::new("device");
        let device_end = device.to_end().into_owned();
        w.write_event(Event::Start(device))?;
        w.create_element("deviceType")
            .write_text_content(BytesText::new(&self.device_type))?;
        w.create_element("friendlyName")
            .write_text_content(BytesText::new(&self.friendly_name))?;
        w.create_element("manufacturer")
            .write_text_content(BytesText::new(&self.manufacturer))?;
        if let Some(manufacturer_url) = &self.manufacturer_url {
            w.create_element("manufacturerURL")
                .write_text_content(BytesText::new(manufacturer_url))?;
        }
        if let Some(model_description) = self.model_description {
            w.create_element("modelDescription")
                .write_text_content(BytesText::new(model_description))?;
        }
        w.create_element("modelName")
            .write_text_content(BytesText::new(self.model_name))?;
        if let Some(model_number) = &self.model_number {
            w.create_element("modelNumber")
                .write_text_content(BytesText::new(model_number))?;
        }
        if let Some(model_url) = self.model_url {
            w.create_element("modelURL")
                .write_text_content(BytesText::new(model_url))?;
        }
        let udn = self.udn.to_string();
        w.create_element("UDN")
            .write_text_content(BytesText::new(&udn))?;
        w.create_element("dlna:X_DLNADOC")
            .write_text_content(BytesText::new("DMS-1.50"))?;
        w.create_element("iconList")
            .write_inner_content::<_, quick_xml::Error>(|w| {
                for icon in &self.icon_list {
                    w.write_serializable("icon", icon)
                        .expect("serialization not fail");
                }
                Ok(())
            })?;
        w.create_element("serviceList")
            .write_inner_content::<_, quick_xml::Error>(|w| {
                for service in &self.service_list {
                    w.write_serializable("service", service)
                        .expect("serialization not fail");
                }
                Ok(())
            })?;
        w.write_event(Event::End(device_end))
    }
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
    const fn connection_manager() -> Self {
        Service {
            service_type: "urn:schemas-upnp-org:service:ConnectionManager:1",
            service_id: "urn:upnp-org:serviceId:ConnectionManager",
            scpdurl: "/upnp/connection_manager/scpd.xml",
            control_url: "/upnp/connection_manager/control.xml",
            event_sub_url: "/upnp/connection_manager/event.xml",
        }
    }
}

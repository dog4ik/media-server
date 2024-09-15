use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
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
                udn: "uuid:6a05839a-8906-4077-9169-ae7ca73f3832",
                dlna_x_dlnadoc: "urn:schemas-dlna-org:device-1-0",
                icon_list: vec![Icon {
                    mimetype: "image/webp",
                    width: "120",
                    height: "120",
                    depth: "120",
                    url: "/logo.webp",
                }],
                service_list: vec![Service {
                    service_type: "urn:schemas-unpn-org:service:ContentDirectory:1",
                    service_id: "uuid:6a05839a-8906-4077-9169-ae7ca73f3834",
                    scpdurl: "/upnp/content_directory",
                    control_url: "/upnp/content_directory_control/control",
                    event_sub_url: "/upnp/content_directory/event",
                }],
            },
        }
    }
}

impl Default for DeviceDescription<'_> {
    fn default() -> Self {
        Self::new("Media server".into())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpecVersion {
    pub major: usize,
    pub minor: usize,
}

impl SpecVersion {
    /// Construct UPnP2.0 spec version
    pub fn upnp_v2() -> Self {
        Self { major: 2, minor: 0 }
    }
}

#[derive(Debug, Serialize, Deserialize)]
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
    pub udn: &'a str,
    #[serde(rename = "X_DLNADOC")]
    pub dlna_x_dlnadoc: &'a str,
    #[serde(rename = "iconList")]
    pub icon_list: Vec<Icon<'a>>,
    #[serde(rename = "serviceList")]
    pub service_list: Vec<Service<'a>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DlnaXDlnadoc {
    #[serde(rename = "@xmlns:dlna")]
    pub xmlns_dlna: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Icon<'a> {
    pub mimetype: &'a str,
    pub width: &'a str,
    pub height: &'a str,
    pub depth: &'a str,
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

use std::{borrow::Cow, fmt::Display, str::FromStr};

use anyhow::Context;
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};
use serde::{Deserialize, Serialize};

use crate::{FromXml, IntoXml, XmlReaderExt};

use super::templates::SpecVersion;

#[derive(Debug)]
pub struct DeviceDescription<'a> {
    pub config_id: Option<String>,
    pub spec_version: SpecVersion,
    pub device: Device<'a>,
}

impl DeviceDescription<'_> {
    pub fn into_xml(&self) -> anyhow::Result<String> {
        use quick_xml::Writer;
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", None, None)))?;
        let root = BytesStart::new("root").with_attributes(
            [
                ("xmlns", "urn:schemas-upnp-org:device-1-0"),
                ("xmlns:dlna", "urn:schemas-dlna-org:device-1-0"),
            ]
            .into_iter()
            .chain(self.config_id.as_ref().map(|id| ("configId", id.as_str()))),
        );
        let root_end = root.to_end().into_owned();
        w.write_event(Event::Start(root))?;
        self.spec_version.write_xml(&mut w)?;
        self.device.write_xml(&mut w)?;

        w.write_event(Event::End(root_end))?;
        Ok(String::from_utf8(w.into_inner())?)
    }
}

impl<'a> FromXml<'a> for DeviceDescription<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let root = r.read_to_start()?;
        anyhow::ensure!(root.local_name().as_ref() == b"root");

        let config_id = root
            .attributes()
            .flatten()
            .find_map(|attr| {
                (attr.key.local_name().as_ref() == b"configId")
                    .then(|| attr.unescape_value().map(|v| v.to_string()))
            })
            .transpose()
            .context("unescape config id")?;

        let root = root.to_owned();

        let spec_version = SpecVersion::read_xml(r)?;

        let device_start = r.read_to_start()?;
        anyhow::ensure!(device_start.local_name().as_ref() == b"device");
        let device = Device::read_xml(r)?;

        r.read_to_end(root.to_end().name())?;

        Ok(Self {
            config_id,
            spec_version,
            device,
        })
    }
}

impl<'a> DeviceDescription<'a> {
    pub fn new(friendly_name: impl Into<Cow<'a, str>>, uuid: uuid::Uuid) -> Self {
        Self {
            config_id: Some("9999".to_string()),
            spec_version: SpecVersion::upnp_v1_1(),
            device: Device {
                device_type: "urn:schemas-upnp-org:device:MediaServer:1".into(),
                friendly_name: friendly_name.into(),
                manufacturer: "media-server".into(),
                serial_number: None,
                manufacturer_url: Some("https://github.com/dog4ik".into()),
                model_description: Some("The media server".into()),
                model_name: "Media server".into(),
                model_number: Some("1.0".into()),
                model_url: Some("https://github.com/dog4ik/media-server".into()),
                udn: Udn::new(uuid),
                icon_list: vec![
                    Icon {
                        mimetype: "image/webp".into(),
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.webp".into(),
                    },
                    Icon {
                        mimetype: "image/png".into(),
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.png".into(),
                    },
                    Icon {
                        mimetype: "image/jpeg".into(),
                        width: 32,
                        height: 32,
                        depth: 25,
                        url: "/logo.jpeg".into(),
                    },
                ],
                service_list: vec![Service::content_directory(), Service::connection_manager()],
                device_list: vec![],
                presentation_url: None,
            },
        }
    }
}

#[derive(Debug)]
pub struct Device<'a> {
    pub device_type: Cow<'a, str>,
    pub friendly_name: Cow<'a, str>,
    /// Manufacturer name. Should be < 64 characters.
    pub manufacturer: Cow<'a, str>,
    pub manufacturer_url: Option<Cow<'a, str>>,
    /// Should be < 128 characters
    pub model_description: Option<Cow<'a, str>>,
    pub model_name: Cow<'a, str>,
    pub model_number: Option<Cow<'a, str>>,
    pub model_url: Option<Cow<'a, str>>,
    pub serial_number: Option<Cow<'a, str>>,
    pub udn: Udn,
    pub icon_list: Vec<Icon<'a>>,
    pub service_list: Vec<Service<'a>>,
    pub device_list: Vec<Device<'a>>,
    pub presentation_url: Option<Cow<'a, str>>,
}

impl<'a> Device<'a> {
    pub fn all_services(&'a self) -> Box<dyn Iterator<Item = &'a Service<'a>> + 'a> {
        let self_services = self.service_list.iter();
        let nested_services = self.device_list.iter().flat_map(|d| d.all_services());
        Box::new(self_services.chain(nested_services))
    }
}

impl IntoXml for Device<'_> {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
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
        if let Some(model_description) = &self.model_description {
            w.create_element("modelDescription")
                .write_text_content(BytesText::new(model_description))?;
        }
        w.create_element("modelName")
            .write_text_content(BytesText::new(&self.model_name))?;
        if let Some(model_number) = &self.model_number {
            w.create_element("modelNumber")
                .write_text_content(BytesText::new(model_number))?;
        }
        if let Some(model_url) = &self.model_url {
            w.create_element("modelURL")
                .write_text_content(BytesText::new(model_url))?;
        }
        if let Some(serial_number) = &self.serial_number {
            w.create_element("serialNumber")
                .write_text_content(BytesText::new(serial_number))?;
        }
        let udn = self.udn.to_string();
        w.create_element("UDN")
            .write_text_content(BytesText::new(&udn))?;
        w.create_element("dlna:X_DLNADOC")
            .write_text_content(BytesText::new("DMS-1.50"))?;
        w.create_element("iconList").write_inner_content(|w| {
            for icon in &self.icon_list {
                w.write_serializable("icon", icon)
                    .expect("serialization not fail");
            }
            Ok(())
        })?;
        w.create_element("serviceList").write_inner_content(|w| {
            for service in &self.service_list {
                w.write_serializable("service", service)
                    .expect("serialization not fail");
            }
            Ok(())
        })?;

        if let Some(presentation_url) = &self.presentation_url {
            w.create_element("presentationURL")
                .write_text_content(BytesText::new(presentation_url))?;
        }
        w.write_event(Event::End(device_end))
    }
}

impl<'a> FromXml<'a> for Device<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let mut device_type = None;
        let mut friendly_name = None;
        let mut manufacturer = None;
        let mut manufacturer_url = None;
        let mut model_description = None;
        let mut model_name = None;
        let mut model_number = None;
        let mut model_url = None;
        let mut serial_number = None;
        let mut udn = None;
        let mut icon_list = Vec::new();
        let mut service_list = Vec::new();
        let mut device_list = Vec::new();
        let mut presentation_url = None;

        while let Ok(event) = r.read_event_err_eof() {
            match event {
                Event::Start(start) => {
                    let start = start.to_owned();
                    let end_name = start.name();
                    match start.local_name().as_ref() {
                        b"deviceType" => {
                            let text = r.read_text(end_name)?;
                            device_type = Some(text);
                        }
                        b"friendlyName" => {
                            let text = r.read_text(end_name)?;
                            friendly_name = Some(text);
                        }
                        b"manufacturer" => {
                            let text = r.read_text(end_name)?;
                            manufacturer = Some(text);
                        }
                        b"manufacturerURL" => {
                            let text = r.read_text(end_name)?;
                            manufacturer_url = Some(text);
                        }
                        b"modelDescription" => {
                            let text = r.read_text(end_name)?;
                            model_description = Some(text);
                        }
                        b"modelName" => {
                            let text = r.read_text(end_name)?;
                            model_name = Some(text);
                        }
                        b"modelNumber" => {
                            let text = r.read_text(end_name)?;
                            model_number = Some(text);
                        }
                        b"modelURL" => {
                            let text = r.read_text(end_name)?;
                            model_url = Some(text);
                        }
                        b"serialNumber" => {
                            let text = r.read_text(end_name)?;
                            serial_number = Some(text);
                        }
                        b"UDN" => {
                            let text = r.read_text(end_name)?;
                            udn = Some(Udn::from_str(&text)?);
                        }
                        b"UPC" => {
                            r.read_to_end(end_name)?;
                        }
                        b"iconList" => {
                            while let Ok(e) = r.read_event() {
                                match e {
                                    Event::Start(start) => {
                                        anyhow::ensure!(start.local_name().as_ref() == b"icon");
                                        icon_list.push(Icon::read_xml(r)?);
                                    }
                                    Event::End(end) => {
                                        anyhow::ensure!(end.local_name().as_ref() == b"iconList");
                                        break;
                                    }
                                    Event::Text(_) => {}
                                    r => Err(anyhow::anyhow!(
                                        "Expected icon start or list end, got {:?}",
                                        r
                                    ))?,
                                }
                            }
                        }
                        b"serviceList" => {
                            while let Ok(e) = r.read_event() {
                                match e {
                                    Event::Start(start) => {
                                        anyhow::ensure!(start.local_name().as_ref() == b"service");
                                        service_list.push(Service::read_xml(r)?);
                                    }
                                    Event::End(end) => {
                                        anyhow::ensure!(
                                            end.local_name().as_ref() == b"serviceList"
                                        );
                                        break;
                                    }
                                    Event::Text(_) => {}
                                    r => Err(anyhow::anyhow!(
                                        "Expected service start or service end, got {:?}",
                                        r
                                    ))?,
                                }
                            }
                        }
                        b"deviceList" => {
                            while let Ok(e) = r.read_event() {
                                match e {
                                    Event::Start(start) => {
                                        anyhow::ensure!(start.local_name().as_ref() == b"device");
                                        device_list.push(Device::read_xml(r)?);
                                    }
                                    Event::End(end) => {
                                        anyhow::ensure!(end.local_name().as_ref() == b"deviceList");
                                        break;
                                    }
                                    Event::Text(_) => {}
                                    r => Err(anyhow::anyhow!(
                                        "Expected device start or device end, got {:?}",
                                        r
                                    ))?,
                                }
                            }
                        }
                        b"presentationURL" => {
                            let text = r.read_text(end_name)?;
                            presentation_url = Some(text);
                        }
                        _ => {
                            r.read_to_end(end_name)?;
                        }
                    }
                }
                Event::End(end) => {
                    anyhow::ensure!(
                        end.local_name().as_ref() == b"device",
                        "expected device end, got {:?}",
                        end
                    );
                    break;
                }
                _ => {}
            }
        }

        let device_type = device_type.context("device type")?;
        let friendly_name = friendly_name.context("friendly name")?;
        let manufacturer = manufacturer.context("manufacturer name")?;
        let model_name = model_name.context("model name")?;
        let udn = udn.context("udn")?;

        Ok(Self {
            device_type,
            friendly_name,
            manufacturer,
            manufacturer_url,
            model_description,
            model_name,
            model_number,
            model_url,
            serial_number,
            udn,
            icon_list,
            service_list,
            device_list,
            presentation_url,
        })
    }
}

/// Unique Device Name. Universally-unique identifier for the device, whether root or
/// embedded. shall be the same over time for a specific device instance (i.e., shall survive
/// reboots).
#[derive(Debug, Clone, Serialize)]
pub struct Udn(String);

impl Udn {
    pub fn new(uuid: uuid::Uuid) -> Self {
        Self(format!("uuid:{uuid}"))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Display for Udn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Udn {
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Icon<'a> {
    pub mimetype: Cow<'a, str>,
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub url: Cow<'a, str>,
}

impl IntoXml for Icon<'_> {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        w.write_serializable("icon", self)
            .expect("serialization not fail");
        Ok(())
    }
}

impl<'a> FromXml<'a> for Icon<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let mut mimetype = None;
        let mut width = None;
        let mut height = None;
        let mut depth = None;
        let mut url = None;

        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    let end = start.name();
                    match start.local_name().as_ref() {
                        b"mimetype" => {
                            let text = r.read_text(end)?;
                            mimetype = Some(text);
                        }
                        b"width" => {
                            let text = r.read_text(end)?;
                            width = Some(text.parse()?);
                        }
                        b"height" => {
                            let text = r.read_text(end)?;
                            height = Some(text.parse()?);
                        }
                        b"depth" => {
                            let text = r.read_text(end)?;
                            depth = Some(text.parse()?);
                        }
                        b"url" => {
                            let text = r.read_text(end)?;
                            url = Some(text);
                        }
                        _ => {
                            // skip unknown tags
                            r.read_to_end(end)?;
                        }
                    }
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"icon");
                    break;
                }
                _ => {}
            }
        }

        let mimetype = mimetype.context("get mimetype")?;
        let width = width.context("get width")?;
        let height = height.context("get height")?;
        let depth = depth.context("get depth")?;
        let url = url.context("get url")?;

        Ok(Self {
            mimetype,
            width,
            height,
            depth,
            url,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Service<'a> {
    #[serde(rename = "serviceType")]
    pub service_type: Cow<'a, str>,
    /// URL for service description. Shall be relative to the URL at which the device description
    #[serde(rename = "serviceId")]
    pub service_id: Cow<'a, str>,
    #[serde(rename = "SCPDURL")]
    pub scpd_url: Cow<'a, str>,
    #[serde(rename = "controlURL")]
    pub control_url: Cow<'a, str>,
    #[serde(rename = "eventSubURL")]
    pub event_sub_url: Cow<'a, str>,
}

impl IntoXml for Service<'_> {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        w.write_serializable("service", self)
            .expect("serialization not fail");
        Ok(())
    }
}

impl<'a> FromXml<'a> for Service<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let mut service_type = None;
        let mut service_id = None;
        let mut scpdurl = None;
        let mut control_url = None;
        let mut event_sub_url = None;

        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    let end = start.name();
                    match start.local_name().as_ref() {
                        b"serviceType" => {
                            let text = r.read_text(end)?;
                            service_type = Some(text);
                        }
                        b"serviceId" => {
                            let text = r.read_text(end)?;
                            service_id = Some(text);
                        }
                        b"SCPDURL" => {
                            let text = r.read_text(end)?;
                            scpdurl = Some(text);
                        }
                        b"controlURL" => {
                            let text = r.read_text(end)?;
                            control_url = Some(text);
                        }
                        b"eventSubURL" => {
                            let text = r.read_text(end)?;
                            event_sub_url = Some(text);
                        }
                        _ => {
                            // skip unknown tags
                            r.read_to_end(end)?;
                        }
                    }
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"service");
                    break;
                }
                _ => {}
            }
        }

        let service_type = service_type.context("service type")?;
        let service_id = service_id.context("service id")?;
        let scpdurl = scpdurl.context("scpdurl")?;
        let control_url = control_url.context("control url")?;
        let event_sub_url = event_sub_url.context("event sub url")?;

        Ok(Self {
            service_type,
            service_id,
            scpd_url: scpdurl,
            control_url,
            event_sub_url,
        })
    }
}

impl Service<'_> {
    const fn content_directory() -> Self {
        Service {
            service_type: Cow::Borrowed("urn:schemas-upnp-org:service:ContentDirectory:1"),
            service_id: Cow::Borrowed("urn:upnp-org:serviceId:ContentDirectory"),
            scpd_url: Cow::Borrowed("/upnp/content_directory/scpd.xml"),
            control_url: Cow::Borrowed("/upnp/content_directory/control.xml"),
            event_sub_url: Cow::Borrowed("/upnp/content_directory/event.xml"),
        }
    }
    const fn connection_manager() -> Self {
        Service {
            service_type: Cow::Borrowed("urn:schemas-upnp-org:service:ConnectionManager:1"),
            service_id: Cow::Borrowed("urn:upnp-org:serviceId:ConnectionManager"),
            scpd_url: Cow::Borrowed("/upnp/connection_manager/scpd.xml"),
            control_url: Cow::Borrowed("/upnp/connection_manager/control.xml"),
            event_sub_url: Cow::Borrowed("/upnp/connection_manager/event.xml"),
        }
    }
}

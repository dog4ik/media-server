use render_control::RenderControlService;
use rupnp::ssdp::{SearchTarget, URN};
use serde::Serialize;
use std::{fmt::Display, time::Duration};
use tokio_stream::StreamExt;

const RENDERING_CONTROL: URN = URN::service("schemas-upnp-org", "RenderingControl", 1);

pub struct DeviceDiscoverer {}

mod action;
mod av_transport;
mod service_variables;
mod connection_manager;
mod content_directory;
mod device_description;
mod render_control;
pub mod router;
pub mod ssdp;
mod types;
mod urn;

impl DeviceDiscoverer {
    pub async fn discover() -> anyhow::Result<()> {
        let devices = rupnp::discover(&SearchTarget::All, Duration::from_secs(3)).await?;
        tokio::pin!(devices);

        while let Some(device) = devices.try_next().await? {
            println!(
                "{} - {} @ {}",
                device.device_type(),
                device.friendly_name(),
                device.url()
            );
        }
        println!("finished discovering devices");
        Ok(())
    }

    pub async fn service_discover() -> anyhow::Result<()> {
        let devices = rupnp::discover(&SearchTarget::All, Duration::from_secs(3)).await?;
        tokio::pin!(devices);

        while let Some(device) = devices.try_next().await? {
            let Some(service) = device.find_service(&RENDERING_CONTROL) else {
                continue;
            };

            let args = "<InstanceID>0</InstanceID><Channel>Master</Channel>";
            let response = service.action(device.url(), "GetVolume", args).await?;

            let volume = response.get("CurrentVolume").unwrap();

            println!("'{}' is at volume {}", device.friendly_name(), volume);

            let args = "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>20</DesiredVolume>";

            let _ = service.action(device.url(), "SetVolume", args).await?;
            let response = service.action(device.url(), "GetVolume", args).await?;
            let volume = response.get("CurrentVolume").unwrap();
            println!(
                "'{}' is at volume {} after setVolume action",
                device.friendly_name(),
                volume
            );
            debug_assert_eq!(volume, "20");

            break;
        }

        println!("finished service discover");
        Ok(())
    }
}

#[derive(Debug)]
struct Action<T> {
    service_type: String,
    payload: T,
    action: String,
}

impl<T: Serialize> Serialize for Action<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let payload = quick_xml::se::to_string(&self.payload).unwrap();
        let body = format!(
            r#"
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
                s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:{action} xmlns:u="{service}">
                        {payload}
                    </u:{action}>
                </s:Body>
            </s:Envelope>"#,
            service = &self.service_type,
            action = self.action,
            payload = payload
        );
        serializer.serialize_str(&body)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UpnpAgent<'a> {
    os: &'a str,
    os_version: &'a str,
    upnp_version: &'a str,
    product: &'a str,
    product_version: &'a str,
}

impl<'a> TryFrom<&'a str> for UpnpAgent<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        todo!()
    }
}

impl Display for UpnpAgent<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{os}/{os_version} UPnp/{upnp_version} {product}/{product_version}",
            os = self.os,
            os_version = self.os_version,
            upnp_version = self.upnp_version,
            product = self.product,
            product_version = self.product_version
        )
    }
}

#[derive(Debug)]
pub struct Device {
    pub url: String,
    pub name: String,
    pub render_control: Option<RenderControlService>,
}

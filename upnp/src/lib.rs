#![doc = include_str!("../README.md")]
pub mod action;
/// [AVTransport:3](https://www.upnp.org/specs/av/UPnP-av-AVTransport-v3-Service-20101231.pdf) service implementation
///
/// This service type enables control over the transport of audio and video streams. The service type defines a
/// common model for A/V transport control suitable for a generic user interface. It can be used to control a
/// wide variety of Smart TVs, disc, tape and solid-state based media devices such as CD players, VCRs and MP3 players.
///
/// A minimal implementation of this service can be used to control tuners.
/// The service type is related to the ConnectionManager service type, which describes A/V connection setup
/// procedures, and the ContentDirectory service, which offers meta-information about the resource stored on
/// the media. AVTransport also offers an action to retrieve any metadata embedded in the resource itself.
pub mod av_transport;
/// [ConnectionManager:3](https://www.upnp.org/specs/av/UPnP-av-ConnectionManager-v3-Service-20101231.pdf) service implementation
///
/// This service-type enables modeling of streaming capabilities of A/V devices, and binding of those
/// capabilities between devices. Each device that is able to send or receive a stream according to the UPnP
/// AV Architecture will have 1 instance of the ConnectionManager service.
///
/// This service provides a
/// mechanism for control points to:
/// 1. Perform capability matching between source/server devices and sink/renderer devices,
/// 2. Find information about currently ongoing transfers in the network,
/// 3. Setup and teardown connections between devices (when required by the streaming protocol).
///
/// The ConnectionManager service is generic enough to properly abstract different kinds of streaming
/// mechanisms, such as HTTP-based streaming, RTSP/RTP-based and 1394-based streaming.
///
/// The ConnectionManager enables control points to abstract from physical media interconnect technology
/// when making connections. The term ‘stream’ used in this service template refers to both analog and
/// digital data transfer.
#[allow(unused, unused_variables)]
pub mod connection_manager;
/// [ContentDirectory:4](https://upnp.org/specs/av/UPnP-av-ContentDirectory-v4-Service.pdf) service implementation
///
/// Many devices within the home network contain various types of content that other devices
/// would like to access (for example, music, videos, still images, etc). As an example, a
/// MediaServer device might contain a significant portion of the homeowner’s audio, video, and
/// still-image library. In order for the homeowner to enjoy this content, the homeowner needs to
/// be able to browse the objects stored on the MediaServer, select a specific one, and cause it
/// to be played on an appropriate rendering device (for example, an au dio player for music
/// objects, a TV for video content, an Electronic Picture Frame for still -images, etc).
///
/// For maximum convenience, it is highly desirable to let the homeowner to initiate these
/// operations from a variety of UI devices. In most cases, these UI devices will either be a UI
/// built into the rendering device, or it will be a stand -alone UI device such as a wireless PDA or
/// tablet. In any case, it is unlikely that the homeowner will interact directly with the device
/// containing the content (that is: the homeowner won’t have to walk over to the server device).
/// In order to enable this capability, the server device needs to provide a uniform mechanism for
/// UI devices to browse the content on the server and to obtain detailed information about
/// individual content objects. This is the purpose of the ContentDirectory service.
///
/// The ContentDirectory service additionally provides a lookup/storage service that enables
/// clients (for example, UI devices) to locate (and possibly store) individual objects (for example,
/// songs, movies, pictures, etc) that the (server) device is capable of providing. For example,
/// this service can be used to enumerate a list of songs stored on an MP3 player, a list of still -
/// images comprising various slide-shows, a list of movies stored in a DVD-Jukebox, a list of TV
/// shows currently being broadcast (a.k.a an EPG), a list of songs stored in a CD -Jukebox, a list
/// of programs stored on a PVR (Personal Video Recorder) device, etc. Nearly any type of
/// content can be enumerated via this ContentDirectory service. For devices that contain
/// multiple types of content (for example, MP3, MPEG2, JPEG, etc.), a single instance of the
/// ContentDirectory service can be used to enumerate all objects, regardless of their type.
pub mod content_directory;
mod device_description;
#[allow(unused)]
mod eventing;
/// This service-type enables a UPnP control point to configure and control IP connections on the WAN
/// interface of a UPnP compliant `InternetGatewayDevice1`. Any type of WAN interface (e.g., DSL or cable)
/// that can support an IP connection can use this service.
pub mod internet_gateway;
/// Axum router used to setup control, description endpoints
pub mod router;
/// UPnP service SSDP search client
pub mod search_client;
mod service;
pub mod service_client;
mod service_variables;
/// Simple Service Discovery Protocol ([SSDP](https://en.wikipedia.org/wiki/Simple_Service_Discovery_Protocol)) implementation
pub mod ssdp;
pub mod templates;
pub mod urn;

/// Useful unitily functions for [Reader](quick_xml::Reader)
pub trait XmlReaderExt<'a> {
    fn read_event_err_eof(&mut self) -> anyhow::Result<quick_xml::events::Event<'a>>;
    fn read_to_start(&mut self) -> anyhow::Result<quick_xml::events::BytesStart<'a>>;
    fn read_to_start_or_empty(
        &mut self,
    ) -> anyhow::Result<(bool, quick_xml::events::BytesStart<'a>)>;
    fn read_end(&mut self) -> anyhow::Result<quick_xml::events::BytesEnd<'a>>;
    fn read_text(&mut self) -> anyhow::Result<quick_xml::events::BytesText<'a>>;
}

impl<'a> XmlReaderExt<'a> for quick_xml::Reader<&'a [u8]> {
    fn read_event_err_eof(&mut self) -> anyhow::Result<quick_xml::events::Event<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::Eof => Err(anyhow::anyhow!("early eof")),
            _ => Ok(event),
        }
    }
    fn read_to_start(&mut self) -> anyhow::Result<quick_xml::events::BytesStart<'a>> {
        loop {
            let event = self.read_event_err_eof()?.into_owned();
            if let quick_xml::events::Event::Start(e) = event {
                break Ok(e);
            }
        }
    }
    fn read_to_start_or_empty(
        &mut self,
    ) -> anyhow::Result<(bool, quick_xml::events::BytesStart<'a>)> {
        loop {
            let event = self.read_event_err_eof()?.into_owned();
            match event {
                quick_xml::events::Event::Start(e) => break Ok((false, e)),
                quick_xml::events::Event::Empty(e) => break Ok((true, e)),
                _ => (),
            }
        }
    }
    fn read_end(&mut self) -> anyhow::Result<quick_xml::events::BytesEnd<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::End(e) => Ok(e),
            e => anyhow::bail!("expected end, got {:?}", e),
        }
    }
    fn read_text(&mut self) -> anyhow::Result<quick_xml::events::BytesText<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::Text(e) => Ok(e),
            e => anyhow::bail!("expected text, got {:?}", e),
        }
    }
}

pub type XmlWriter = quick_xml::Writer<Vec<u8>>;

/// Allows structs to serialize themselves into xml fragments
pub trait IntoXml {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()>;

    fn into_string(&self) -> std::io::Result<String> {
        let mut w = quick_xml::Writer::new(Vec::new());
        self.write_xml(&mut w)?;
        Ok(String::from_utf8(w.into_inner()).expect("produced value to be utf-8"))
    }
}

impl std::fmt::Debug for Box<dyn IntoXml> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.into_string().unwrap())
    }
}

/// Allows structs to deserialize themselves from xml reader
pub trait FromXml<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl<T: IntoXml> IntoXml for Vec<T> {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        for el in self {
            el.write_xml(w)?;
        }
        Ok(())
    }
}

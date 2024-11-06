use quick_xml::{
    events::{BytesDecl, BytesStart, BytesText, Event},
    Writer,
};

use crate::{IntoXml, XmlWriter};

use super::{
    filter::Filter,
    property_name::{PropertyValue, ValueType},
    Container, ContainerProperty, Item, ItemProperty, ObjectProperty,
};

const UPNP_NS: &str = "upnp";
const DC_NS: &str = "dc";

macro_rules! impl_basic_property {
    ($name:literal for $type:ident) => {
        impl ObjectProperty for $type {
            const NAME: &str = $name;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
    ($name:literal for multivalue $type:ident) => {
        impl ObjectProperty for $type {
            const NAME: &str = $name;
            const MULTIVALUE: bool = true;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
    (container only $name:literal for $type:ident) => {
        impl ContainerProperty for $type {
            const NAME: &str = $name;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
    (container only $name:literal for multivalue $type:ident) => {
        impl ContainerProperty for $type {
            const NAME: &str = $name;
            const MULTIVALUE: bool = true;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
    (item only $name:literal for $type:ident) => {
        impl ItemProperty for $type {
            const NAME: &str = $name;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
    (item only $name:literal for multivalue $type:ident) => {
        impl ItemProperty for $type {
            const NAME: &str = $name;
            const MULTIVALUE: bool = true;
        }
        impl Into<PropertyValue> for $type {
            fn into(self) -> PropertyValue {
                let (ns, name) = $name
                    .split_once(':')
                    .map_or((None, $name), |(ns, name)| (Some(ns), name));
                PropertyValue {
                    ns,
                    name,
                    is_allowed: false,
                    value: ValueType::basic(self.0),
                    dependant_properties: vec![],
                }
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct AlbumArtUri(pub String);
impl_basic_property!("upnp:albumArtURI" for multivalue AlbumArtUri);

/// The upnp:episodeCount property contains the total number of episodes in the
/// series to which this content belongs.
#[derive(Debug, Clone, Copy)]
pub struct EpisodeCount(pub u32);
impl_basic_property!("upnp:episodeCount" for EpisodeCount);

/// The upnp:episodeCount property contains the episode number of this recorded
/// content within the series to which this content belongs.
#[derive(Debug, Clone, Copy)]
pub struct EpisodeNumber(pub u32);
impl_basic_property!("upnp:episodeNumber" for EpisodeNumber);

/// The upnp:episodeSeason property indicates the season of the episode
#[derive(Debug, Clone, Copy)]
pub struct EpisodeSeason(pub u32);
impl_basic_property!("upnp:episodeSeason" for EpisodeSeason);

/// The upnp:programTitle property contains the name of the program. This is most
/// likely obtained from a database that contains program -related information, such as an
/// Electronic Program Guide.
/// Example: “Friends Series Finale”.
/// Note: To be precise, this is different from the dc:title property which indicates a friendly name
/// for the ContentDirectory service object. However, in many cases, the dc:title property will be
/// set to the same value as the upnp:programTitle property.
#[derive(Debug, Clone)]
pub struct ProgramTitle(pub String);
impl_basic_property!("upnp:programTitle" for ProgramTitle);

/// The upnp:seriesTitle property contains the name of the series.
#[derive(Debug, Clone)]
pub struct SeriesTitle(pub String);
impl_basic_property!("upnp:seriesTitle" for SeriesTitle);

/// Contains a brief description of the content item
#[derive(Debug, Clone)]
pub struct Description(pub String);
impl_basic_property!("dc:description" for Description);

/// The upnp:longDescription property contains a few lines of description of the
/// content item (longer than the dc:description property).
#[derive(Debug, Clone)]
pub struct LongDescription(pub String);
impl_basic_property!("dc:long_description" for LongDescription);

/// The dc:date property contains the primary date of the content.
/// Examples:
/// - `2004-05-14`
/// - `2004-05-14T14:30:05`
/// - `2004-05-14T14:30:05+09:00`
#[derive(Debug)]
pub struct Date {
    date: time::PrimitiveDateTime,
}
impl Date {
    pub const FORMAT: time::format_description::well_known::Rfc3339 =
        time::format_description::well_known::Rfc3339;
}

impl ObjectProperty for Date {
    const NAME: &str = "dc:date";
}
impl IntoXml for Date {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> quick_xml::Result<()> {
        let formatted = self.date.format(&Self::FORMAT).expect("infallible");
        w.create_element("dc:date")
            .write_text_content(BytesText::new(&formatted))?;
        Ok(())
    }
}

/// The upnp:longDescription property contains a few lines of description of the
/// content item (longer than the dc:description property).
#[derive(Debug, Clone)]
pub struct Language(pub String);
impl_basic_property!("dc:language" for Language);

/// The read-only upnp:playbackCount property contains the number of times the
/// content has been played. The special value -1 means that the content has been played bu
#[derive(Debug, Clone)]
pub struct PlaybackCount(pub String);
impl_basic_property!("upnp:playbackCount" for PlaybackCount);

/// The upnp:recordedDuration property contains the duration of the recorded content
#[derive(Debug, Clone)]
pub struct RecordedDuration(pub std::time::Duration);
impl ObjectProperty for RecordedDuration {
    const NAME: &str = "upnp:recordedDuration";
}
impl IntoXml for RecordedDuration {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> quick_xml::Result<()> {
        let upnp_duration = super::UpnpDuration::new(self.0);
        w.create_element("upnp:recordedDuration")
            .write_text_content(BytesText::new(&upnp_duration.to_string()))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct SearchClass {
    class: String,
    name: Option<String>,
    include_derived: bool,
}

impl IntoXml for SearchClass {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let mut attributes = Vec::new();
        attributes.push((
            "includeDerived",
            if self.include_derived { "1" } else { "0" },
        ));

        if let Some(name) = &self.name {
            attributes.push(("name", name));
        };

        w.create_element("upnp:searchClass")
            .with_attributes(attributes)
            .write_text_content(BytesText::new(&self.class))?;
        Ok(())
    }
}

pub mod res {
    use std::{fmt::Display, str::FromStr};

    use anyhow::Context;
    use quick_xml::events::BytesText;

    use crate::{
        content_directory::{
            property_name::{DependantProperty, PropertyValue, ValueType},
            ObjectProperty, UpnpDuration, UpnpFramerate, UpnpResolution,
        },
        IntoXml, XmlWriter,
    };

    #[derive(Debug)]
    pub struct Resource {
        uri: String,
        protocol_info: ProtocolInfo,
        import_uri: Option<String>,
        /// The size in bytes of the resource.
        size: Option<u64>,
        duration: Option<UpnpDuration>,
        protection: Option<String>,
        bitrate: Option<usize>,
        bits_per_sample: Option<usize>,
        sample_frequency: Option<usize>,
        nr_audio_channels: Option<usize>,
        resolution: Option<UpnpResolution>,
        color_depth: Option<usize>,
        tspec: Option<String>,
        allowed_use: Option<String>,
        validity_start: Option<String>,
        validity_end: Option<String>,
        remaining_time: Option<String>,
        usage_info: Option<String>,
        rights_info_uri: Option<String>,
        content_info_uri: Option<String>,
        record_quality: Option<String>,
        daylight_saving: Option<String>,
        framerate: Option<UpnpFramerate>,
    }

    impl ObjectProperty for Resource {
        const NAME: &str = "res";
        const MULTIVALUE: bool = true;
    }

    impl Resource {
        pub fn new(uri: String, protocol_info: ProtocolInfo) -> Self {
            Self {
                uri,
                protocol_info,
                import_uri: None,
                size: None,
                duration: None,
                protection: None,
                bitrate: None,
                bits_per_sample: None,
                sample_frequency: None,
                nr_audio_channels: None,
                resolution: None,
                color_depth: None,
                tspec: None,
                allowed_use: None,
                validity_start: None,
                validity_end: None,
                remaining_time: None,
                usage_info: None,
                rights_info_uri: None,
                content_info_uri: None,
                record_quality: None,
                daylight_saving: None,
                framerate: None,
            }
        }
    }

    #[derive(Debug)]
    pub struct ProtocolInfo {
        protocol: String,
        network: String,
        content_format: String,
        additional_info: String,
    }

    impl ProtocolInfo {
        pub fn http_get(mime: String) -> Self {
            Self {
                protocol: "http-get".into(),
                network: "*".into(),
                content_format: mime,
                additional_info: "*".into(),
            }
        }
    }

    impl Display for ProtocolInfo {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "{protocol}:{network}:{content_format}:{additional_info}",
                protocol = self.protocol,
                network = self.network,
                content_format = self.content_format,
                additional_info = self.additional_info,
            )
        }
    }

    impl FromStr for ProtocolInfo {
        type Err = anyhow::Error;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let mut split = s.splitn(4, ':');
            let protocol = split.next().context("get protocol part")?;
            let network = split.next().context("get network part")?;
            let content_format = split.next().context("get content format part")?;
            let additional_info = split.next().context("get additional info part")?;
            anyhow::ensure!(split.next().is_none());
            Ok(Self {
                protocol: protocol.to_owned(),
                network: network.to_owned(),
                content_format: content_format.to_owned(),
                additional_info: additional_info.to_owned(),
            })
        }
    }

    impl Into<PropertyValue> for Resource {
        fn into(self) -> PropertyValue {
            let mut attributes = Vec::new();
            attributes.push(DependantProperty::new_required(
                "protocolInfo",
                self.protocol_info.to_string(),
            ));

            let mut add = |name: &'static str, value: String| {
                attributes.push(DependantProperty::new_optional(name, value));
            };

            if let Some(import_uri) = &self.import_uri {
                add("importUri", import_uri.to_owned());
            }
            if let Some(size) = self.size.map(|s| s.to_string()) {
                add("size", size);
            }
            if let Some(duration) = &self.duration {
                add("duration", duration.to_string());
            }
            if let Some(protection) = &self.protection {
                add("protection", protection.to_owned());
            }
            if let Some(bitrate) = self.bitrate {
                add("bitrate", bitrate.to_string());
            }
            if let Some(bits_per_sample) = self.bits_per_sample {
                add("bitsPerSample", bits_per_sample.to_string());
            }
            if let Some(sample_frequency) = self.sample_frequency {
                add("sampleFrequency", sample_frequency.to_string());
            }
            if let Some(nr_audio_channels) = self.nr_audio_channels {
                add("nrAudioChannels", nr_audio_channels.to_string());
            }
            if let Some(resolution) = &self.resolution {
                add("resolution", resolution.to_string());
            }
            if let Some(color_depth) = self.color_depth {
                add("colorDepth", color_depth.to_string());
            }
            if let Some(tspec) = &self.tspec {
                add("tspec", tspec.to_owned());
            }
            if let Some(allowed_use) = &self.allowed_use {
                add("allowedUse", allowed_use.to_owned());
            }
            if let Some(validity_start) = &self.validity_start {
                add("validityStart", validity_start.to_owned());
            }
            if let Some(validity_end) = &self.validity_end {
                add("validityEnd", validity_end.to_owned());
            }
            if let Some(remaining_time) = &self.remaining_time {
                add("remainingTime", remaining_time.to_owned());
            }
            if let Some(usage_info) = &self.usage_info {
                add("usageInfo", usage_info.to_owned());
            }
            if let Some(rights_info_uri) = &self.rights_info_uri {
                add("rightsInfoUri", rights_info_uri.to_owned());
            }
            if let Some(content_info_uri) = &self.content_info_uri {
                add("contentInfoUri", content_info_uri.to_owned());
            }
            if let Some(record_quality) = &self.record_quality {
                add("recordQuality", record_quality.to_owned());
            }
            if let Some(daylight_saving) = &self.daylight_saving {
                add("daylightSaving", daylight_saving.to_owned());
            }
            if let Some(framerate) = &self.framerate {
                add("framerate", framerate.to_string());
            }

            PropertyValue {
                ns: None,
                name: "res",
                is_allowed: false,
                value: ValueType::Value(Box::new(self.uri)),
                dependant_properties: attributes,
            }
        }
    }

    impl IntoXml for Resource {
        fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
            let mut attributes = Vec::new();
            attributes.push(("protocolInfo", self.protocol_info.to_string()));
            if let Some(import_uri) = &self.import_uri {
                attributes.push(("importUri", import_uri.to_owned()));
            }
            if let Some(size) = self.size.map(|s| s.to_string()) {
                attributes.push(("size", size));
            }
            if let Some(duration) = &self.duration {
                attributes.push(("duration", duration.to_string()));
            }
            if let Some(protection) = &self.protection {
                attributes.push(("protection", protection.to_owned()));
            }
            if let Some(bitrate) = self.bitrate {
                attributes.push(("bitrate", bitrate.to_string()));
            }
            if let Some(bits_per_sample) = self.bits_per_sample {
                attributes.push(("bitsPerSample", bits_per_sample.to_string()));
            }
            if let Some(sample_frequency) = self.sample_frequency {
                attributes.push(("sampleFrequency", sample_frequency.to_string()));
            }
            if let Some(nr_audio_channels) = self.nr_audio_channels {
                attributes.push(("nrAudioChannels", nr_audio_channels.to_string()));
            }
            if let Some(resolution) = &self.resolution {
                attributes.push(("resolution", resolution.to_string()));
            }
            if let Some(color_depth) = self.color_depth {
                attributes.push(("colorDepth", color_depth.to_string()));
            }
            if let Some(tspec) = &self.tspec {
                attributes.push(("tspec", tspec.to_owned()));
            }
            if let Some(allowed_use) = &self.allowed_use {
                attributes.push(("allowedUse", allowed_use.to_owned()));
            }
            if let Some(validity_start) = &self.validity_start {
                attributes.push(("validityStart", validity_start.to_owned()));
            }
            if let Some(validity_end) = &self.validity_end {
                attributes.push(("validityEnd", validity_end.to_owned()));
            }
            if let Some(remaining_time) = &self.remaining_time {
                attributes.push(("remainingTime", remaining_time.to_owned()));
            }
            if let Some(usage_info) = &self.usage_info {
                attributes.push(("usageInfo", usage_info.to_owned()));
            }
            if let Some(rights_info_uri) = &self.rights_info_uri {
                attributes.push(("rightsInfoUri", rights_info_uri.to_owned()));
            }
            if let Some(content_info_uri) = &self.content_info_uri {
                attributes.push(("contentInfoUri", content_info_uri.to_owned()));
            }
            if let Some(record_quality) = &self.record_quality {
                attributes.push(("recordQuality", record_quality.to_owned()));
            }
            if let Some(daylight_saving) = &self.daylight_saving {
                attributes.push(("daylightSaving", daylight_saving.to_owned()));
            }
            if let Some(framerate) = &self.framerate {
                attributes.push(("framerate", framerate.to_string()));
            }
            w.create_element("res")
                .with_attributes(attributes.iter().map(|(k, v)| (*k, v.as_str())))
                .write_text_content(BytesText::new(&self.uri))?;
            Ok(())
        }
    }
}

#[derive(Default, Debug)]
pub struct DidlResponse {
    pub containers: Vec<Container>,
    pub items: Vec<Item>,
}

impl DidlResponse {
    pub fn len(&self) -> usize {
        self.items.len() + self.containers.len()
    }

    pub fn into_xml(&self) -> anyhow::Result<String> {
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        let didl = BytesStart::new("DIDL-Lite").with_attributes([
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
            ("xmlns", "urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"),
            ("xmlns:upnp", "urn:schemas-upnp-org:metadata-1-0/upnp/"),
            ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
            (
                "xsi:schemaLocation",
                r#"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/ http://www.upnp.org/schemas/av/didl-lite.xsd urn:schemas-upnp-org:metadata-1-0/upnp/ http://www.upnp.org/schemas/av/upnp.xsd"#,
            ),
        ]);
        let didl_end = didl.to_end().into_owned();
        w.write_event(Event::Start(didl))?;

        for object in &self.containers {
            object.write_xml(&mut w)?;
        }

        for object in &self.items {
            object.write_xml(&mut w)?;
        }

        w.write_event(Event::End(didl_end))?;

        Ok(String::from_utf8(w.into_inner())?)
    }

    pub fn apply_filter(&mut self, filter: Filter) {
        match filter {
            Filter::Wildcard => {
                println!("Applying wildcard filter");
                for container in &mut self.containers {
                    for property in container.properties.values_mut() {
                        property.allow_all();
                    }
                    for property in container.multivalue_properties.values_mut().flatten() {
                        property.allow_all();
                    }
                }
                for item in &mut self.items {
                    for property in item.properties.values_mut() {
                        property.allow_all();
                    }
                    for property in item.multivalue_properties.values_mut().flatten() {
                        property.allow_all();
                    }
                }
            }
            Filter::Allowed(filters) => {
                for container in &mut self.containers {
                    for property in container.properties.values_mut() {
                        for filter in &filters {
                            property.apply_filter(filter, 0);
                        }
                    }
                    for property in container.multivalue_properties.values_mut().flatten() {
                        for filter in &filters {
                            property.apply_filter(filter, 0);
                        }
                    }
                }
                for item in &mut self.items {
                    for property in item.properties.values_mut() {
                        for filter in &filters {
                            property.apply_filter(filter, 0);
                        }
                    }
                    for property in item.multivalue_properties.values_mut().flatten() {
                        for filter in &filters {
                            property.apply_filter(filter, 0);
                        }
                    }
                }
            }
        }
    }
}

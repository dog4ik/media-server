use quick_xml::{
    Writer,
    events::{BytesDecl, BytesStart, BytesText, Event},
};

use crate::{IntoXml, XmlWriter};

use super::{
    Container, Item, ObjectProperty,
    filter::Filter,
    property_name::{PropertyValue, ValueType},
};

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

/// The upnp:album property indicates the title of the album to which the content item belongs.
#[derive(Debug, Clone)]
pub struct Album(pub String);
impl_basic_property!("upnp:album" for multivalue Album);

/// The upnp:playlist property indicates the name of a playlist (the dc:title of a
/// playlistItem) to which the content item belongs
#[derive(Debug, Clone)]
pub struct Playlist(pub String);
impl_basic_property!("upnp:playlist" for multivalue Playlist);

/// The upnp:albumArtURI property contains a reference to album art.
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

/// The upnp:programTitle property contains the name of the program.
///
/// This is most likely obtained from a database that contains program -related information, such as an
/// Electronic Program Guide.
///
/// Example: "Friends Series Finale".
///
/// Note: To be precise, this is different from the `dc:title` property which indicates a friendly name
/// for the `ContentDirectory` service object. However, in many cases, the `dc:title` property will be
/// set to the same value as the `upnp:programTitle` property.
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
///
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

impl std::str::FromStr for Date {
    type Err = time::error::Parse;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Date {
            date: time::PrimitiveDateTime::parse(s, &Self::FORMAT)?,
        })
    }
}

impl ObjectProperty for Date {
    const NAME: &str = "dc:date";
}
impl IntoXml for Date {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let formatted = self.date.format(&Self::FORMAT).expect("infallible");
        w.create_element("dc:date")
            .write_text_content(BytesText::new(&formatted))?;
        Ok(())
    }
}

impl From<Date> for PropertyValue {
    fn from(val: Date) -> Self {
        PropertyValue {
            ns: Some("dc"),
            name: "date",
            is_allowed: false,
            value: ValueType::Value(Box::new(val)),
            dependant_properties: vec![],
        }
    }
}

/// The dc:language property indicates one of the languages used in the content as
/// defined by RFC 3066.
///
/// For example: `en-US`
#[derive(Debug, Clone)]
pub struct Language(pub String);
impl_basic_property!("dc:language" for multivalue Language);

/// The read-only upnp:playbackCount property contains the number of times the
/// content has been played. The special value -1 means that the content has been played bu
#[derive(Debug, Clone)]
pub struct PlaybackCount(pub i32);
impl_basic_property!("upnp:playbackCount" for PlaybackCount);

/// The upnp:recordedDuration property contains the duration of the recorded content
#[derive(Debug, Clone)]
pub struct RecordedDuration(pub std::time::Duration);
impl ObjectProperty for RecordedDuration {
    const NAME: &str = "upnp:recordedDuration";
}
impl IntoXml for RecordedDuration {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let upnp_duration = super::UpnpDuration::new(self.0);
        w.create_element("upnp:recordedDuration")
            .write_text_content(BytesText::new(&upnp_duration.to_string()))?;
        Ok(())
    }
}

impl From<RecordedDuration> for PropertyValue {
    fn from(val: RecordedDuration) -> Self {
        PropertyValue {
            ns: Some("upnp"),
            name: "recordedDuration",
            is_allowed: false,
            value: ValueType::Value(Box::new(val)),
            dependant_properties: vec![],
        }
    }
}

/// This property contains a class for which the container object can be searched.
///
/// The read-only upnp:searchClass property is only applicable to container objects.
///
/// If @searchable (property on container class) = true, then
/// - If no upnp:searchClass properties are specified, then the `Search` action can return any
/// match.
/// - If upnp:searchClass properties are specified, then the `Search` action shall only return
/// matches from the classes specified in the upnp:searchClass properties.
/// - upnp:searchClass is allowed.
/// - upnp:searchClass is always determined by the `ContentDirectory` service.
/// - upnp:searchClass semantics are per container, there is no parent-child relationship, they
/// only apply to searches started from that container.
/// else
/// - The container and its subtrees are not searchable.
/// - The values of the upnp:searchClass properties are meaningless and therefore the
/// upnp:searchClass properties should not be included.
///
/// Default Value: If @searchable (property on container class) = true, then all classes can be searched.
#[derive(Debug)]
pub struct SearchClass {
    class: String,
    /// Indicates a friendly name for the class
    name: Option<String>,
    /// Indicates whether the class specified shall also include derived classes.
    include_derived: bool,
}

impl ObjectProperty for SearchClass {
    const NAME: &str = "upnp:searchClass";
    const MULTIVALUE: bool = true;
}

impl IntoXml for SearchClass {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
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
        IntoXml, XmlWriter,
        content_directory::{
            ObjectProperty, UpnpDuration, UpnpFramerate, UpnpResolution,
            property_name::{DependantProperty, PropertyValue, ValueType},
        },
    };

    /// The res property indicates a resource, typically a media file, associated with the object.
    ///
    /// If the value of the res property is not present, then the content has not yet been fully
    /// imported by the ContentDirectory service and is not yet accessible for playback purposes
    #[derive(Debug)]
    pub struct Resource {
        uri: String,
        /// This required property identifies the protocol that shall be used to transmit the resource
        protocol_info: ProtocolInfo,
        /// The read-only res@importUri property indicates the URI via which the resource
        /// can be imported to the ContentDirectory service via the ImportResource() action or HTTP POST.
        ///
        /// The res@importUri property identifies a download portal for the associated res
        /// property of a specific target object.
        ///
        /// It is used to create a local copy of the external content.
        ///
        /// After the transfer finishes successfully, the local content is then associated with the target
        /// object by setting the target object’s res property value to a URI for that content, which may or
        /// may not be the same URI as the one specified in the res@importUri property, depending on
        /// the ContentDirectory service implementation.
        import_uri: Option<String>,
        /// The size in bytes of the resource.
        size: Option<u64>,
        /// The res@duration property indicates the time duration of the playback of the resource, at normal speed.
        duration: Option<UpnpDuration>,
        /// The res@protection property contains some identification of a protection system used for the resource.
        protection: Option<String>,
        /// The res@bitrate property indicates the bitrate in bytes/second of the encoding of the resource.
        ///
        /// Note that there exists an inconsistency with a res@bitrate property name and its value being
        /// expressed in bytes/sec.
        ///
        /// In case the resource has been encoded using variable bitrate (VBR), it is recommen dead that
        /// the `res@bitrate` value represents the average bitrate, calculated over the entire duration of
        /// the resource (total number of bytes divided by the total duration of the resource).
        bitrate: Option<usize>,
        /// The res@bitsPerSample property indicates the number of bits used to represent one sample of the resource.
        bits_per_sample: Option<usize>,
        /// The res@sampleFrequency property indicates the sample frequency used to digitize the audio resource.
        ///
        /// Expressed in Hz.
        sample_frequency: Option<usize>,
        /// The res@nrAudioChannels property indicates the number of audio channels
        /// present in the audio resource, for example, 1 for mono, 2 for stereo, 6 for Dolby Surround.
        nr_audio_channels: Option<usize>,
        /// The res@resolution property indicates the XxY resolution, in pixels, of the resource
        resolution: Option<UpnpResolution>,
        /// The res@colorDepth property indicates the number of bits per pixel used to represent the video or image resource
        color_depth: Option<usize>,
        /// The res@tspec property identifies the content’s QoS (quality of service) characteristics.
        ///
        /// It has a maximum length of 256 characters.
        ///
        /// The details about this property, including its components and formatting constraints,
        /// are defined in the QoS Manager service definition document.
        tspec: Option<String>,
        /// The res@allowedUse property is composed of a comma-separated list of value pairs.
        ///
        /// Each value pair is composed of an enumerated string value, followed by a colon (":"),
        /// followed by an integer.
        ///
        /// For example, "PLAY:5,COPY:1".
        ///
        ///
        /// In each pair, the first value corresponds to an allowed us e for the resource referenced by the
        /// associated res property. Recommended enumerated values are: "PLAY", "COPY", "MOVE" and “UNKNOWN”.
        /// Vendors may extend this list.
        ///
        /// The “UNKNOWN” value is the default value when new resources are created.
        ///
        /// A value of “UNKNOWN” indicates that allowed uses for this
        /// resource might exist, but have not been reflected in the ContentDirectory service.
        ///
        ///
        /// The second quantity is the number of times the specified use is allowed to occur.
        /// A value of `-1` indicates that there is no limit on the number of times this use may occur.
        /// This value should be updated when the number of allowed uses changes.
        ///
        /// For example, a resource with the res@allowedUse property initially set to "COPY:1" should be updated to
        /// "COPY:0" after a copy has been successfully completed.
        allowed_use: Option<String>,
        /// The res@validityStart property defines the beginning date&time when the
        /// corresponding uses described in the res@allowedUse property become valid
        ///
        /// Example value designates May 30, 2004, 1:20pm, as a validity interval
        /// beginning value: `2004-05-30T13:20:00-05:00`.
        ///
        /// When the res@validityStart property is not present, the beginning of the validity interval is
        /// assumed to have already started.
        validity_start: Option<String>,
        /// The res@validityEnd property defines the ending date&time when the
        /// corresponding uses described in the res@allowedUse property become invalid.
        ///
        /// When the res@validityEnd property is not present, there correspondingly is no end to the validity interval.
        validity_end: Option<String>,
        /// The res@remainingTime property is used to indicate the amount of time
        /// remaining until the use specified in the res@allowedUse property is revoked.
        ///
        /// The remaining time is an aggregate amount of time that the resource may be used either continuously or in
        /// discrete intervals.
        ///
        /// When both res@remainingTime and res@validityEnd are specified, the use
        /// is revoked either when res@remainingTime reaches zero, or when the res@validityEnd time
        /// is reached, whichever occurs first.
        remaining_time: Option<String>,
        /// The res@usageInfo property contains a user-friendly string with additional
        /// information about the allowed use of the resource.
        ///
        /// Example: `Playing of the movie is allowed in high-definition mode. One copy is allowed to be made,
        /// but only the standard definition version may be copied`.
        usage_info: Option<String>,
        /// The res@rightsInfoURI property references an html page and a web site associated with the rights vendor for the resource
        rights_info_uri: Option<String>,
        /// Each res@contentInfoURI property contains a URI employed to assist the user
        /// interface in providing additional information to the user about the content referenced by the
        /// resource.
        content_info_uri: Option<String>,
        /// When the resource referenced by the res property was created by recording, the
        /// res@recordQuality property can be specified to indicate the quality level(s) used to make the
        /// recording.
        ///
        /// The res@recordQuality property is a CSV list of \<type\> ":" \<recording quality\>
        /// pairs. The type and quality in each pair are separated by a colon character (":").
        ///
        /// The type portion indicates what kind of value system is used in the recording quality portion.
        ///
        /// The recording quality portion is the actual recording quality v value used.
        ///
        /// When there is more than one pair of colon-separated values in the list, all pairs shall represent the same
        /// quality level in different type systems
        record_quality: Option<String>,
        /// The res@daylightSaving property indicates whether the time values used in
        /// other res-dependent properties, such as the res@validityStart property and the
        /// res@validityEnd property, are expressed using as a reference either Daylight Saving Time or
        /// Standard Time.
        ///
        /// This property is only applicable when the time values in other res-dependent
        /// properties are expressed in local time
        daylight_saving: Option<String>,
        /// The res@framerate property indicates the frame rate in frames/second of the
        /// encoding of the resource including a trailing indication of progressive or interlaced scanning
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

        pub fn set_duartion<T: Into<UpnpDuration>>(&mut self, duration: impl Into<Option<T>>) {
            let duration = duration.into();
            self.duration = duration.map(Into::into);
        }

        pub fn set_resoulution(&mut self, resolution: impl Into<Option<UpnpResolution>>) {
            self.resolution = resolution.into()
        }

        pub fn set_bitrate(&mut self, bitrate: impl Into<Option<usize>>) {
            self.bitrate = bitrate.into()
        }

        pub fn set_size(&mut self, size: impl Into<Option<u64>>) {
            self.size = size.into()
        }

        pub fn set_audio_channels(&mut self, audio_channels: impl Into<Option<usize>>) {
            self.nr_audio_channels = audio_channels.into()
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

    impl From<Resource> for PropertyValue {
        fn from(val: Resource) -> Self {
            let mut attributes = Vec::new();
            attributes.push(DependantProperty::new_required(
                "protocolInfo",
                val.protocol_info.to_string(),
            ));

            let mut add = |name: &'static str, value: String| {
                attributes.push(DependantProperty::new_optional(name, value));
            };

            if let Some(import_uri) = val.import_uri {
                add("importUri", import_uri);
            }
            if let Some(size) = val.size {
                add("size", size.to_string());
            }
            if let Some(duration) = &val.duration {
                add("duration", duration.to_string());
            }
            if let Some(protection) = val.protection {
                add("protection", protection);
            }
            if let Some(bitrate) = val.bitrate {
                add("bitrate", bitrate.to_string());
            }
            if let Some(bits_per_sample) = val.bits_per_sample {
                add("bitsPerSample", bits_per_sample.to_string());
            }
            if let Some(sample_frequency) = val.sample_frequency {
                add("sampleFrequency", sample_frequency.to_string());
            }
            if let Some(nr_audio_channels) = val.nr_audio_channels {
                add("nrAudioChannels", nr_audio_channels.to_string());
            }
            if let Some(resolution) = &val.resolution {
                add("resolution", resolution.to_string());
            }
            if let Some(color_depth) = val.color_depth {
                add("colorDepth", color_depth.to_string());
            }
            if let Some(tspec) = val.tspec {
                add("tspec", tspec);
            }
            if let Some(allowed_use) = val.allowed_use {
                add("allowedUse", allowed_use);
            }
            if let Some(validity_start) = val.validity_start {
                add("validityStart", validity_start);
            }
            if let Some(validity_end) = val.validity_end {
                add("validityEnd", validity_end);
            }
            if let Some(remaining_time) = val.remaining_time {
                add("remainingTime", remaining_time);
            }
            if let Some(usage_info) = val.usage_info {
                add("usageInfo", usage_info);
            }
            if let Some(rights_info_uri) = val.rights_info_uri {
                add("rightsInfoUri", rights_info_uri);
            }
            if let Some(content_info_uri) = val.content_info_uri {
                add("contentInfoUri", content_info_uri);
            }
            if let Some(record_quality) = val.record_quality {
                add("recordQuality", record_quality);
            }
            if let Some(daylight_saving) = val.daylight_saving {
                add("daylightSaving", daylight_saving);
            }
            if let Some(framerate) = &val.framerate {
                add("framerate", framerate.to_string());
            }

            PropertyValue {
                ns: None,
                name: "res",
                is_allowed: false,
                value: ValueType::Value(Box::new(val.uri)),
                dependant_properties: attributes,
            }
        }
    }

    impl IntoXml for Resource {
        fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
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

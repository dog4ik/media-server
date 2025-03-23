use std::str::FromStr;

use crate::{
    IntoXml,
    content_directory::UpnpDuration,
    device_description::Udn,
    service_client::{ActionCallError, ScpdClient, ScpdService},
    service_variables::{IntoUpnpValue, SVariable},
    urn::{URN, UrnType},
};

/// This REQUIRED state variable forms the core of the AVTransport service. It defines the conceptually top-
/// level state of the transport, for example, whether it is playing, recording, etc.
///
/// Device vendors do not need to implement all allowed values of this variable,
/// for example, non-recordable media will not implement the "RECORDING" state.
///
/// Note that dubbing of media at various speeds is not supported in this version of the `AVTransport`, mainly
/// because there are no standards for cross-device dubbing speeds.
///
/// Device vendors are allowed to implement additional vendor-defined transport states.
///
/// However, since the semantic meaning of these transport states is not specified,
/// control points that find a `AVTransport` service in a transport state that they do not understand
/// are encouraged to refrain from interacting with that `AVTransport` service (for example, forcing the service into the "STOPPED" state).
/// Rather, they are encouraged to wait until the service transits back into a transport state that they understand.
#[derive(Debug)]
pub enum TransportState {
    Stopped,
    Playing,
    Transitioning,
    /// The `PAUSED_PLAYBACK` state is different from the `PAUSED_RECORDING` state in the sense that in case the media contains
    /// video, it indicates output of a still image.
    PausedPlayback,
    /// The `PAUSED_RECORDING` state is different from the `STOPPED` state in the sense that the transport
    /// is already prepared for recording and can respond faster or more accurate.
    PausedRecording,
    Recording,
    NoMediaPresent,
}

impl SVariable for TransportState {
    type VarType = Self;

    const VAR_NAME: &str = "TransportState";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "STOPPED",
        "PLAYING",
        "TRANSITIONING",
        "PAUSED_PLAYBACK",
        "PAUSED_RECORDING",
        "RECORDING",
        "NO_MEDIA_PRESENT",
    ]);
}

impl IntoUpnpValue for TransportState {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "STOPPED" => Self::Stopped,
            "PLAYING" => Self::Playing,
            "TRANSITIONING" => Self::Transitioning,
            "PAUSED_PLAYBACK" => Self::PausedPlayback,
            "PAUSED_RECORDING" => Self::PausedRecording,
            "RECORDING" => Self::Recording,
            "NO_MEDIA_PRESENT" => Self::NoMediaPresent,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for TransportState {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            TransportState::Stopped => "STOPPED",
            TransportState::Playing => "PLAYING",
            TransportState::Transitioning => "TRANSITIONING",
            TransportState::PausedPlayback => "PAUSED_PLAYBACK",
            TransportState::PausedRecording => "PAUSED_RECORDING",
            TransportState::Recording => "RECORDING",
            TransportState::NoMediaPresent => "NO_MEDIA_PRESENT",
        };
        msg.write_xml(w)
    }
}

/// This REQUIRED state variable is used to indicate if asynchronous errors have occurred, during operation
/// of the AVTransport service, that cannot be returned by a normal action.
///
/// For example, some time after
/// playback of a stream has been started (via SetAVTransportURI() and [ScpdClient::play] actions), there can be network
/// congestion or server problems causing hiccups in the rendered media.
///
/// These types of situations can be signaled to control points by setting this state variable to value "ERROR_OCCURRED".
///
/// More specific error descriptions MAY also be used as vendor extensions. The value of TransportState after an error has
/// occurred is implementation-dependent; some implementations MAY go to "STOPPED" while other
/// implementations MAY be able to continue playing after an error.
///
/// The time at which this state variable returns to "OK" after an error situation is also implementation dependent.
#[derive(Debug)]
pub enum TransportStatus {
    Ok,
    ErrorOccurred,
}

impl SVariable for TransportStatus {
    type VarType = Self;

    const VAR_NAME: &str = "TransportStatus";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["OK", "ERROR_OCCURRED"]);
}

impl IntoUpnpValue for TransportStatus {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "OK" => Self::Ok,
            "ERROR_OCCURRED" => Self::ErrorOccurred,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for TransportStatus {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            TransportStatus::Ok => "OK",
            TransportStatus::ErrorOccurred => "ERROR_OCCURRED",
        };
        msg.write_xml(w)
    }
}

/// This REQUIRED state variable indicates whether the current media is track-aware (both single and multi-
/// track) or track-unaware (e.g. VHS-tape).
#[derive(Debug)]
pub enum CurrentMediaCategory {
    NoMedia,
    TrackAware,
    TrackUnAware,
}

impl SVariable for CurrentMediaCategory {
    type VarType = Self;

    const VAR_NAME: &str = "CurrentMediaCategory";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["NO_MEDIA", "TRACK_AWARE", "TRACK_UNAWARE"]);
}

impl IntoUpnpValue for CurrentMediaCategory {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "NO_MEDIA" => Self::NoMedia,
            "TRACK_AWARE" => Self::TrackAware,
            "TRACK_UNAWARE" => Self::TrackUnAware,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for CurrentMediaCategory {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            CurrentMediaCategory::NoMedia => "NO_MEDIA",
            CurrentMediaCategory::TrackAware => "TRACK_AWARE",
            CurrentMediaCategory::TrackUnAware => "TRACK_UNAWARE",
        };
        msg.write_xml(w)
    }
}

/// This REQUIRED state variable indicates the storage medium of the resource specified by [AVTransportURI].
///
/// If no resource is specified, then the state variable is set to [PlaybackStorageMedium::None]. If [AVTransportURI]
/// refers to a resource received from the UPnP network, the state variable is set to [PlaybackStorageMedium::Network].
///
/// Device vendors MAY extend the specified allowed value list of this variable.
///
/// For example, various types of solid- state media formats can be added in a vendor-specific way.
///
/// Note that this variable is not intended for signal- or content-formats such as MPEG2. Such type of
/// information is exposed by the ConnectionManager service associated with this service.
#[derive(Debug)]
pub enum PlaybackStorageMedium {
    /// Unknown medium
    Unknown,
    /// Digital Video Tape medium
    Dv,
    /// Mini Digital Video Tape medium
    MiniDv,
    /// VHS Tape medium
    Vhs,
    /// W-VHS Tape medium
    WVhs,
    /// Super VHS Tape medium
    SVhs,
    /// Digital VHS Tape medium
    DVhs,
    /// Compact VHS medium
    Vhsc,
    /// 8 mm Video Tape medium
    Video8,
    /// High resolution 8 mm Video Tape medium
    Hi8,
    /// Compact Disc-Read Only Memory medium
    CdRom,
    /// Compact Disc-Digital Audio medium
    CdDa,
    /// Compact Disc-Recordable medium
    CdR,
    /// Compact Disc-Rewritable medium
    CdRw,
    /// Video Compact Disc medium
    VideoCd,
    /// Super Audio Compact Disc medium
    Sacd,
    /// Mini Disc Audio medium
    MdAudio,
    /// Mini Disc Picture medium
    MdPicture,
    /// DVD Read Only medium
    DvdRom,
    /// DVD Video medium
    DvdVideo,
    /// DVD Recordable medium
    DvdAndR,
    /// DVD Recordable medium
    DvdR,
    /// DVD Rewritable medium
    DvdAndRw,
    /// DVD Rewritable medium
    DvdRw,
    /// DVD RAM medium
    DvdRam,
    /// DVD Audio medium
    DvdAudio,
    /// Digital Audio Tape medium
    Dat,
    /// Laser Disk medium
    Ld,
    /// Hard Disk Drive medium
    Hdd,
    /// Micro MV Tape medium
    MicroMv,
    /// Network Interface medium
    Network,
    /// No medium present
    None,
    /// Medium type discovery is not implemented
    NotImplemented,
    /// SD (Secure Digital) Memory Card medium
    Sd,
    /// PC Card medium
    PcCard,
    /// MultimediaCard medium
    Mmc,
    /// Compact Flash medium
    Cf,
    /// Blu-ray Disc medium
    Bd,
    /// Memory Stick medium
    Ms,
    /// HD DVD medium
    HdDvd,
}

impl SVariable for PlaybackStorageMedium {
    type VarType = Self;

    const VAR_NAME: &str = "PlaybackStorageMedium";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "UNKNOWN",
        "DV",
        "MINI-DV",
        "VHS",
        "W-VHS",
        "S-VHS",
        "D-VHS",
        "VHSC",
        "VIDEO8",
        "HI8",
        "CD-ROM",
        "CD-DA",
        "CD-R",
        "CD-RW",
        "VIDEO-CD",
        "SACD",
        "MD-AUDIO",
        "MD-PICTURE",
        "DVD-ROM",
        "DVD-VIDEO",
        "DVD+R",
        "DVD-R",
        "DVD+RW",
        "DVD-RW",
        "DVD-RAM",
        "DVD-AUDIO",
        "DAT",
        "LD",
        "HDD",
        "MICRO-MV",
        "NETWORK",
        "NONE",
        "NOT_IMPLEMENTED",
        "SD",
        "PC-CARD",
        "MMC",
        "CF",
        "BD",
        "MS",
        "HD_DVD",
    ]);
}

impl IntoUpnpValue for PlaybackStorageMedium {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "UNKNOWN" => Self::Unknown,
            "DV" => Self::Dv,
            "MINI-DV" => Self::MiniDv,
            "VHS" => Self::Vhs,
            "W-VHS" => Self::WVhs,
            "S-VHS" => Self::SVhs,
            "D-VHS" => Self::DVhs,
            "VHSC" => Self::Vhsc,
            "VIDEO8" => Self::Video8,
            "HI8" => Self::Hi8,
            "CD-ROM" => Self::CdRom,
            "CD-DA" => Self::CdDa,
            "CD-R" => Self::CdR,
            "CD-RW" => Self::CdRw,
            "VIDEO-CD" => Self::VideoCd,
            "SACD" => Self::Sacd,
            "MD-AUDIO" => Self::MdAudio,
            "MD-PICTURE" => Self::MdPicture,
            "DVD-ROM" => Self::DvdRom,
            "DVD-VIDEO" => Self::DvdVideo,
            "DVD+R" => Self::DvdAndR,
            "DVD-R" => Self::DvdR,
            "DVD+RW" => Self::DvdAndRw,
            "DVD-RW" => Self::DvdRw,
            "DVD-RAM" => Self::DvdRam,
            "DVD-AUDIO" => Self::DvdAudio,
            "DAT" => Self::Dat,
            "LD" => Self::Ld,
            "HDD" => Self::Hdd,
            "MICRO-MV" => Self::MicroMv,
            "NETWORK" => Self::Network,
            "NONE" => Self::None,
            "NOT_IMPLEMENTED" => Self::NotImplemented,
            "SD" => Self::Sd,
            "PC-CARD" => Self::PcCard,
            "MMC" => Self::Mmc,
            "CF" => Self::Cf,
            "BD" => Self::Bd,
            "MS" => Self::Ms,
            "HD_DVD" => Self::HdDvd,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for PlaybackStorageMedium {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Unknown => "UNKNOWN",
            Self::Dv => "DV",
            Self::MiniDv => "MINI-DV",
            Self::Vhs => "VHS",
            Self::WVhs => "W-VHS",
            Self::SVhs => "S-VHS",
            Self::DVhs => "D-VHS",
            Self::Vhsc => "VHSC",
            Self::Video8 => "VIDEO8",
            Self::Hi8 => "HI8",
            Self::CdRom => "CD-ROM",
            Self::CdDa => "CD-DA",
            Self::CdR => "CD-R",
            Self::CdRw => "CD-RW",
            Self::VideoCd => "VIDEO-CD",
            Self::Sacd => "SACD",
            Self::MdAudio => "MD-AUDIO",
            Self::MdPicture => "MD-PICTURE",
            Self::DvdRom => "DVD-ROM",
            Self::DvdVideo => "DVD-VIDEO",
            Self::DvdAndR => "DVD+R",
            Self::DvdR => "DVD-R",
            Self::DvdAndRw => "DVD+RW",
            Self::DvdRw => "DVD-RW",
            Self::DvdRam => "DVD-RAM",
            Self::DvdAudio => "DVD-AUDIO",
            Self::Dat => "DAT",
            Self::Ld => "LD",
            Self::Hdd => "HDD",
            Self::MicroMv => "MICRO-MV",
            Self::Network => "NETWORK",
            Self::None => "NONE",
            Self::NotImplemented => "NOT_IMPLEMENTED",
            Self::Sd => "SD",
            Self::PcCard => "PC-CARD",
            Self::Mmc => "MMC",
            Self::Cf => "CF",
            Self::Bd => "BD",
            Self::Ms => "MS",
            Self::HdDvd => "HD_DVD",
        };
        msg.write_xml(w)
    }
}

/// This REQUIRED state variable indicates the storage medium where the resource specified by
/// AVTransportURI will be recorded when a Record action is issued.
///
/// If no resource is specified, then the state
/// variable is set to "NONE". Device vendors MAY extend the allowed value list of this variable. For
/// example, various types of solid-state media formats can be added in a vendor-specific way.
///
/// Note that this variable is not intended for signal- or content-formats such as MPEG2. Such type of
/// information is exposed by the ConnectionManager service associated with this service. If the service
/// implementation does not support recording, then this state variable MUST be set to
/// "NOT_IMPLEMENTED".
///
/// The allowed values for this state variable are the same as the [PlaybackStorageMedium] state variable.
#[derive(Debug)]
pub struct RecordStorageMedium;

impl SVariable for RecordStorageMedium {
    type VarType = PlaybackStorageMedium;

    const VAR_NAME: &str = "RecordStorageMedium";
}

/// This REQUIRED state variable contains a static, comma-separated list of storage media that the device can
/// play.
///
/// RECOMMENDED values are defined in the allowed value list for state variable
#[derive(Debug)]
pub struct PossiblePlaybackStorageMedia;

impl SVariable for PossiblePlaybackStorageMedia {
    type VarType = String;

    const VAR_NAME: &str = "PossiblePlaybackStorageMedia";
}

/// This REQUIRED state variable contains a static, comma-separated list of storage media onto which the
/// device can record.
///
/// RECOMMENDED values are defined in the allowed value list for state variable RecordStorageMedium.
///
/// If the service implementation does not support recording, then this state variable
/// MUST be set to "NOT_IMPLEMENTED"
#[derive(Debug)]
pub struct PossibleRecordStorageMedia;

impl SVariable for PossibleRecordStorageMedia {
    type VarType = String;

    const VAR_NAME: &str = "PossibleRecordStorageMedia";
}

/// This REQUIRED state variable indicates the current play mode (for example, random play, repeated play,
/// etc.).
///
/// This notion is typical for CD-based audio media, but is generally not supported by tape-based media.
#[derive(Debug)]
pub enum CurrentPlayMode {
    Normal,
    Shuffle,
    RepeatOne,
    RepeatAll,
    Random,
    /// Value "DIRECT_1" indicates playing a single track and then stop (don’t play the next track).
    Direct1,
    /// Value "INTRO" indicates playing a short sample (typically 10 seconds or so) of each track on the media.
    Intro,
}

impl SVariable for CurrentPlayMode {
    type VarType = Self;

    const VAR_NAME: &str = "CurrentPlayMode";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "NORMAL",
        "SHUFFLE",
        "REPEAT_ONE",
        "REPEAT_ALL",
        "RANDOM",
        "DIRECT_1",
        "INTRO",
    ]);
}

impl IntoUpnpValue for CurrentPlayMode {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "NORMAL" => Self::Normal,
            "SHUFFLE" => Self::Shuffle,
            "REPEAT_ONE" => Self::RepeatOne,
            "REPEAT_ALL" => Self::RepeatAll,
            "RANDOM" => Self::Random,
            "DIRECT_1" => Self::Direct1,
            "INTRO" => Self::Intro,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for CurrentPlayMode {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Normal => "NORMAL",
            Self::Shuffle => "SHUFFLE",
            Self::RepeatOne => "REPEAT_ONE",
            Self::RepeatAll => "REPEAT_ALL",
            Self::Random => "RANDOM",
            Self::Direct1 => "DIRECT_1",
            Self::Intro => "INTRO",
        };
        msg.write_xml(w)
    }
}

/// A string representation of a rational fraction that indicates the speed
/// relative to normal speed.
///
/// Example values are `1`, `1/2`, `2`, `-1`, `1/10`, etc.
///
/// Actually supported speeds can be retrieved from the `AllowedValueList` of this state variable in the AVTransport service description.
/// Value "1" is REQUIRED, value "0" is not allowed. Negative values indicate reverse playback
#[derive(Debug)]
pub struct TransportPlaySpeed;

impl SVariable for TransportPlaySpeed {
    type VarType = String;

    const VAR_NAME: &str = "TransportPlaySpeed";
}

/// This REQUIRED state variable reflects the write protection status of the currently loaded media
#[derive(Debug)]
pub enum RecordMediumWriteStatus {
    Writable,
    /// indicates a writable media that is currently write-protected (for example, a protected VHS tape)
    Protected,
    /// Indicates an inherent read-only media (for example, a DVD-ROM disc) or the device doesn’t support recording on the current media
    NotWritable,
    /// If no media is loaded
    Unknown,
    NotImplemented,
}

impl IntoUpnpValue for RecordMediumWriteStatus {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "WRITABLE" => Self::Writable,
            "PROTECTED" => Self::Protected,
            "NOT_WRITABLE" => Self::NotWritable,
            "UNKNOWN" => Self::Unknown,
            "NOT_IMPLEMENTED" => Self::NotImplemented,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for RecordMediumWriteStatus {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Writable => "WRITABLE",
            Self::Protected => "PROTECTED",
            Self::NotWritable => "NOT_WRITABLE",
            Self::Unknown => "UNKNOWN",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        };
        msg.write_xml(w)
    }
}

impl SVariable for RecordMediumWriteStatus {
    type VarType = Self;

    const VAR_NAME: &str = "RecordMediumWriteStatus";
}

/// This REQUIRED state variable indicates the currently set record quality mode.
///
/// Such a setting takes the form of "Quality Ordinal:label".
///
/// The Quality Ordinal indicates a particular relative quality level available
/// in the device, from 0 (lowest quality) to n (highest quality).
///
/// The label associated with the ordinal provides a
/// human-readable indication of the ordinal’s meaning.
///
/// If the service implementation does not support
/// recording, then this state variable MUST be set to “NOT_IMPLEMENTED
#[derive(Debug)]
pub enum CurrentRecordQualityMode {
    Ep,
    Lp,
    Sp,
    Basic,
    Medium,
    High,
    NotImplemented,
}

impl IntoUpnpValue for CurrentRecordQualityMode {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "0:EP" => Self::Ep,
            "1:LP" => Self::Lp,
            "2:SP" => Self::Sp,
            "3:BASIC" => Self::Basic,
            "4:MEDIUM" => Self::Medium,
            "5:HIGH" => Self::High,
            "NOT_IMPLEMENTED" => Self::NotImplemented,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for CurrentRecordQualityMode {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Ep => "0:EP",
            Self::Lp => "1:LP",
            Self::Sp => "2:SP",
            Self::Basic => "3:BASIC",
            Self::Medium => "4:MEDIUM",
            Self::High => "5:HIGH",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        };
        msg.write_xml(w)
    }
}

impl SVariable for CurrentRecordQualityMode {
    type VarType = Self;

    const VAR_NAME: &str = "CurrentRecordQualityMode";
}

/// This REQUIRED state variable contains a static, comma-separated list of recording quality modes that the
/// device supports.
///
/// For example, for an analog VHS recorder the string would be “0:EP,1:LP,2:SP”, while for
/// a PVR the string would be “0:BASIC,1:MEDIUM,2:HIGH”.
/// The string specifics depend on the type of device containing the AVTransport.
///
/// Note that record quality modes are independent of the content-format that MAY be exposed to the network through a `ConnectionManager` service.
///
/// If the service implementation does not support recording, then this state variable MUST be set to "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct PossibleRecordQualityModes;

impl SVariable for PossibleRecordQualityModes {
    type VarType = String;

    const VAR_NAME: &str = "PossibleRecordQualityModes";
}

/// This REQUIRED state variable contains the number of tracks controlled by the AVTransport instance.
///
/// If no resource is associated with the AVTransport instance (via SetAVTransportURI()), and there is no default
/// resource (for example, a loaded disc) then NumberOfTracks MUST be 0.
///
/// Also, if the implementation is
/// never able to determine the number of tracks in the currently selected media, NumberOfTracks MUST be
/// set to 0. Otherwise, it MUST be 1 or higher.
///
/// In some cases, for example, large playlist, it can take a long
/// time to determine the exact number of tracks. Until the exact number is determined, the value of the state
/// variable is implementation dependent, for example, keeping it to 1 until determined or updating the value
/// periodically. Note that in any case, the AVTransport service MUST generate a LastChange event with
/// defined moderation period when the exposed value is updated.
///
/// For track-unaware media, this state variable will always be set to 1. For LD and DVD media, a track is
/// defined as a chapter number. For Tuners that provide an indexed list of channels, a track is defined as an
/// index number in such a list. This state variable has to be consistent with the resource identified by
/// [AVTransportURI].
///
/// For example, if [AVTransportURI] points to a single MP3 file, then `NumberOfTracks`
/// MUST be set to 1.
/// However, if [AVTransportURI] points to a playlist file, then `NumberOfTracks` MUST be
/// equal to the number of entries in the playlist.
#[derive(Debug)]
pub struct NumberOfTracks;

impl SVariable for NumberOfTracks {
    type VarType = u32;
    const VAR_NAME: &str = "NumberOfTracks";
}

/// Current track
///
/// If [NumberOfTracks] is 0, then `CurrentTrack` will be 0. Otherwise, this
/// state variable contains the sequence number of the currently selected track, starting at value 1, up to and
/// including [NumberOfTracks].
///
/// For track-unaware media, this state variable is always 1.
///
/// For LD and DVD media, the notion of track equals the notion of chapter number.
///
/// For Tuners that provide an indexed list of
/// channels, the current track is defined as the current index number in such a list.
#[derive(Debug)]
pub struct CurrentTrack;

impl SVariable for CurrentTrack {
    type VarType = u32;
    const VAR_NAME: &str = "CurrentTrack";
}

/// This REQUIRED state variable contains the duration of the current track
#[derive(Debug)]
pub struct CurrentTrackDuration;

impl SVariable for CurrentTrackDuration {
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "CurrentTrackDuration";
}

/// Media duration
///
/// This REQUIRED state variable contains the duration of the media, as identified by state variable [AVTransportURI].
///
/// In case the [AVTransportURI] represents only 1 track, this state variable is equal to [CurrentTrackDuration]
///
/// If the service implementation does not support media duration
/// information, then this state variable MUST be set to "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct CurrentMediaDuration;

impl SVariable for CurrentMediaDuration {
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "CurrentMediaDuration";
}

/// This REQUIRED state variable contains the metadata, in the form of a DIDL-Lite XML Fragment (defined
/// in the ContentDirectory service template), associated with the resource pointed to by state variable
/// [CurrentTrackURI].
///
/// The metadata could have been extracted from state variable [AVTransportURIMetaData],
/// or extracted from the resource binary itself (for example, embedded ID3 tags for MP3 audio).
///
/// This is implementation dependent.
///
/// If the service implementation does not support this feature, then this state variable MUST be set to "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct CurrentTrackMetaData;

impl SVariable for CurrentTrackMetaData {
    type VarType = String;
    const VAR_NAME: &str = "CurrentTrackMetaData";
}

/// This REQUIRED state variable contains a reference, in the form of a URI, to the current track.
///
/// The URI can enable a control point to retrieve any meta-data associated with the current track, such as title and
/// author information, via the [ContentDirectory service](crate::content_directory)
/// [Browse()](crate::content_directory::ContentDirectoryService::browse)
/// and/or `Search` action. In case the media
/// does contain multi-track content, but there is no separate URI associated with each track,
/// `CurrentTrackURI` MUST be set equal to [AVTransportURI].
#[derive(Debug)]
pub struct CurrentTrackURI;

impl SVariable for CurrentTrackURI {
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "CurrentTrackURI";
}

/// This REQUIRED state variable contains a reference, in the form of a URI, to the resource controlled by the
/// AVTransport instance.
///
/// This URI can refer to a single item (for example, a song) or to a collection of items
/// (for example, a playlist).
///
/// In the single item case, the `AVTransport` will have 1 track and [AVTransportURI] is
/// equal to [CurrentTrackURI].
///
/// In the collection of items case, the `AVTransport` will have multiple tracks, and
/// [AVTransportURI] will remain constant during track changes.
///
/// The URI enables a control point to retrieve
/// any meta-data associated with the `AVTransport` instance, such as title and author information, via the
/// [ContentDirectory service](crate::content_directory).
#[derive(Debug)]
pub struct AVTransportURI;

impl SVariable for AVTransportURI {
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "AVTransportURI";
}

/// This REQUIRED state variable contains the meta-data, in the form of a DIDL-Lite XML Fragment,
/// associated with the resource pointed to by state variable [AVTransportURI].
///
/// See the [ContentDirectory service specification](https://upnp.org/specs/av/UPnP-av-ContentDirectory-v4-Service.pdf)
/// for details.
///
/// If the service implementation
/// does not support this feature, then this state variable MUST be set to "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct AVTransportURIMetaData;

impl SVariable for AVTransportURIMetaData {
    type VarType = String;
    const VAR_NAME: &str = "AVTransportURIMetaData";
}

/// his REQUIRED state variable contains the [AVTransportURI] value to be played when the playback of the
/// current [AVTransportURI] finishes.
///
/// Setting this variable ahead of time (via action `SetNextAVTransportURI()`) enables a device
/// to provide seamless transitions between resources for certain
/// data transfer protocols that need buffering (for example, HTTP).
///
/// If the service implementation does not
/// support this feature, then this state variable MUST be set to "NOT_IMPLEMENTED".
///
/// Do not confuse transitions between [AVTransportURI] and `NextAVTransportURI` with track transitions.
/// When [AVTransportURI] is set to a playlist, `NextAVTransportURI` will be played when the whole playlist
/// finishes, not when the current playlist entry ([CurrentTrackURI]) finishes.
#[derive(Debug)]
pub struct NextAVTransportURI;

impl SVariable for NextAVTransportURI {
    // TODO: Handle NOT_IMPLEMENTED value
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "NextAVTransportURI";
}

/// This REQUIRED state variable contains the meta-data, in the form of a DIDL-Lite XML Fragment,
/// associated with the resource pointed to by state variable [NextAVTransportURI].
///
/// See the [ContentDirectory service specification](https://upnp.org/specs/av/UPnP-av-ContentDirectory-v4-Service.pdf)
/// for details.
///
/// If the service implementation does not support this feature then this state variable MUST be set to
/// "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct NextAVTransportURIMetaData;

impl SVariable for NextAVTransportURIMetaData {
    type VarType = String;
    const VAR_NAME: &str = "NextAVTransportURIMetaData";
}

/// For track-aware media, this REQUIRED state variable contains the current position in the current track, in
/// terms of time, measured from the beginning of the current track.
///
/// The range for this state variable is from
/// "00:00:00" to the duration of the current track as indicated by the CurrentTrackDuration state variable.
///
/// For track-aware media, this state variable always contains a positive value.
///
/// For track-unaware media (e.g. a single tape), this state variable contains the position, in terms of time,
/// measured from a zero reference point on the media. The range for this state variable is from the beginning
/// of the media, measured from the zero reference point to the end of the media, also measured from the zero
/// reference point. For track-unaware media, this state variable can be negative. Indeed, when the zero
/// reference point does not coincide with the beginning of the media, all positions before the zero reference
/// point are expressed as negative values.
///
/// The time format used for the `RelativeTimePosition` state variable is the same as for state variable
/// [CurrentTrackDuration].
///
/// If the service implementation does not support relative time-based position
/// information, then this state variable MUST be set to "NOT_IMPLEMENTED".
#[derive(Debug)]
pub struct RelativeTimePosition;

impl SVariable for RelativeTimePosition {
    // TODO: Handle NOT_IMPLEMENTED value
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "RelativeTimePosition";
}

/// This REQUIRED state variable contains the current position, in terms of time, measured from the
/// beginning of the media.
///
/// The time format used for the [AbsoluteTimePosition] state variable is the same as for
/// state variable [CurrentTrackDuration].
///
/// The range for this state variable is from "00:00:00" to the duration of
/// the current media as indicated by the [CurrentMediaDuration] state variable.
///
/// This state variable always contains a positive value.
///
/// If the service implementation does not support any kind of position information, then this state variable
/// MUST be set to "NOT_IMPLEMENTED".
///
/// Devices that do not have time position information, but are able
/// to detect whether they are at the end of the media MUST use special value "END_OF_MEDIA" when
/// actually at the end, and the value "NOT_IMPLEMENTED" otherwise.
#[derive(Debug)]
pub struct AbsoluteTimePosition;

impl SVariable for AbsoluteTimePosition {
    // TODO: Handle NOT_IMPLEMENTED | END_OF_MEDIA values
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "AbsoluteTimePosition";
}

/// For track-aware media, this REQUIRED state variable contains the current position in the current track, in
/// terms of a dimensionless counter, measured from the beginning of the current track.
///
/// The range for this state
/// variable is from 0 to the counter value that corresponds to the end of the current track.
///
/// For track-aware media, this state variable always contains a positive value.
///
/// For track-unaware media (e.g. a single tape), this state variable contains the position, in terms of a
/// dimensionless counter, measured from a zero reference point on the media.
///
/// The range for this state variable
/// is from the counter value that corresponds to the beginning of the media, measured from the zero reference
/// point to the counter value that corresponds to the end of the media, also measured from the zero reference
/// point.
///
/// For track-unaware media, this state variable can be negative, Indeed, when the zero reference point
/// does not coincide with the beginning of the media, all positions before the zero reference point are
/// expressed as negative values.
///
/// For devices that support media with addressable ranges that equal or exceed the allowed range of this
/// counter, the `AVTransport` service MUST scale actual media addresses to counter values to fit within the
/// range allowed for this counter.
///
/// If the service implementation does not support relative count-based position information, then this state
/// variable MUST be set to the [i32::MAX].
#[derive(Debug)]
pub struct RelativeCounterPosition;

impl SVariable for RelativeCounterPosition {
    type VarType = i32;
    const VAR_NAME: &str = "RelativeCounterPosition";
}

/// This REQUIRED state variable contains the current position, in terms of a dimensionless counter,
/// measured from the beginning of the loaded media.
///
/// The allowed range for this variable is 0 - 2147483646.
/// For devices that support media with addressable ranges that equal or exceed the allowed range of this
/// counter, the AVTransport service MUST scale actual media addresses to counter values to fit within the
/// range allowed for this counter.
///
/// If the service implementation does not support absolute count-based
/// position information, then this state variable MUST be set to the value 2147483647.
///
/// Note: Although the data type for state variable AbsoluteCounterPosition is [u32], the range is restricted to
/// 0 - [i32::MAX] for backwards compatibility reasons
#[derive(Debug)]
pub struct AbsoluteCounterPosition;

impl SVariable for AbsoluteCounterPosition {
    type VarType = u32;
    const VAR_NAME: &str = "AbsoluteCounterPosition";
}

pub mod current_transport_actions {
    use crate::{
        IntoXml,
        service_variables::{IntoUpnpValue, SVariable},
    };

    /// This state variable contains a comma-separated list of transport-controlling actions
    /// that can be successfully invoked for the current resource at this specific point in time.
    ///
    /// This CONDITIONALLY REQUIRED state variable MUST be supported if the AVTransport service
    /// implements the GetCurrentTransportActions() action.
    ///
    /// The list MUST contain a subset (including the empty set) of the following action names:
    /// "Play", "Stop", "Pause", "Seek", "Next", "Previous" and "Record".
    ///
    /// In addition, the list MAY be augmented by a subset of vendor-defined transport-controlling action names.
    /// For example:
    /// When a live stream from the Internet is being controlled, the variable can be only “Play,Stop”. When a local audio CD
    /// is being controlled, the variable can be "Play,Stop,Pause,Seek,Next,Previous". This information can be
    /// used, for example, to dynamically enable or disable play, stop, and pause buttons, etc., on a user interface.
    #[derive(Debug)]
    pub struct CurrentTransportActions(pub Vec<CurrentTransportAction>);

    impl IntoXml for CurrentTransportActions {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            if self.0.is_empty() {
                return Ok(());
            }

            for action in &self.0[..self.0.len() - 1] {
                action.write_xml(w)?;
                ",".write_xml(w)?;
            }

            self.0.last().expect("not empty check above").write_xml(w)
        }
    }

    impl IntoUpnpValue for CurrentTransportActions {
        fn from_xml_value(value: &str) -> anyhow::Result<Self> {
            Ok(Self(
                value
                    .split(',')
                    .filter_map(|v| CurrentTransportAction::from_xml_value(v).ok())
                    .collect(),
            ))
        }
    }

    impl SVariable for CurrentTransportActions {
        type VarType = Self;
        const VAR_NAME: &str = "CurrentTransportActions";
    }

    #[derive(Debug)]
    pub enum CurrentTransportAction {
        Play,
        Stop,
        Pause,
        Seek,
        Next,
        Previous,
        Record,
    }

    impl IntoUpnpValue for CurrentTransportAction {
        fn from_xml_value(value: &str) -> anyhow::Result<Self> {
            let out = match value {
                "PLAY" => Self::Play,
                "STOP" => Self::Stop,
                "PAUSE" => Self::Pause,
                "SEEK" => Self::Seek,
                "NEXT" => Self::Next,
                "PREVIOUS" => Self::Previous,
                "RECORD" => Self::Record,
                _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
            };
            Ok(out)
        }
    }

    impl IntoXml for CurrentTransportAction {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let val = match self {
                Self::Play => "PLAY",
                Self::Stop => "STOP",
                Self::Pause => "PAUSE",
                Self::Seek => "SEEK",
                Self::Next => "NEXT",
                Self::Previous => "PREVIOUS",
                Self::Record => "RECORD",
            };
            val.write_xml(w)
        }
    }
}

#[derive(Debug)]
pub struct LastChange;

impl SVariable for LastChange {
    type VarType = String;

    const VAR_NAME: &str = "LastChange";
}

/// The [DRMState] state variable is used by instances of the `AVTransport` service to inform control points about
/// process failures and other `AVTransport` instance state changes that can occur independently of
/// `AVTransport` actions.
///
/// This CONDITIONALLY REQUIRED state variable MUST be supported if the `AVTransport` service
/// implements the `GetDRMState` action and the `AVTransport` service supports controlling of the transport
/// for DRM-controlled content.
#[derive(Debug)]
pub enum DRMState {
    /// This setting indicates that DRM related processing has completed successfully.
    /// This setting also applies, to items which do not have DRM protection applied.
    Ok,
    /// This setting indicates that the state of the DRM subsystem is not known.
    /// For example, this would be the case when the DRM system is first initialized
    /// and the content-binary location has not yet been specified.
    Unknown,
    /// This setting indicates that the DRM system is currently deriving a decryption
    /// key to decrypt a content-binary.
    ProcessingContentKey,
    /// This setting indicates that a content key needed to start or continue media
    /// transport was either not received or has failed verification.
    ContentKeyFailure,
    /// This setting indicates that the authentication process is currently in progress,
    /// but has not yet completed.
    AttemptingAuthentication,
    /// This setting indicates than an attempted authentication process has failed.
    FailedAuthentication,
    /// This setting indicates that authentication has not yet taken place or that a
    /// previously successful authentication has transitioned to a non- authenticated
    /// state for example due to a timeout or other condition.
    NotAuthenticated,
    /// This setting indicates that the DRM system has detected that this device has
    /// been revoked, i.e. the device has been explicitly prohibited from accessing
    /// any DRM protected content on this server.
    DeviceRevocation,
    /// This setting indicates that the device cannot decrypt the content-binary since
    /// it does not support the DRM technology used to encode this content.
    DrmSystemNotSupported,
    /// This setting indicates that this device is not able to obtain any license for
    /// this content-binary.
    LicenseDenied,
    /// This setting indicates that a previously valid license obtained by this device
    /// has expired.
    LicenseExpired,
    /// This setting indicates that a license granted to the device does not permit an
    /// attempted operation on the content-binary.
    LicenseInsufficient,
}

impl SVariable for DRMState {
    type VarType = Self;

    const VAR_NAME: &str = "DRMState";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "OK",
        "UNKNOWN",
        "PROCESSING_CONTENT_KEY",
        "CONTENT_KEY_FAILURE",
        "ATTEMPTING_AUTHENTICATION",
        "FAILED_AUTHENTICATION",
        "NOT_AUTHENTICATED",
        "DEVICE_REVOCATION",
        "DRM_SYSTEM_NOT_SUPPORTED",
        "LICENSE_DENIED",
        "LICENSE_EXPIRED",
        "LICENSE_INSUFFICIENT",
    ]);
}

impl IntoUpnpValue for DRMState {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "OK" => Self::Ok,
            "UNKNOWN" => Self::Unknown,
            "PROCESSING_CONTENT_KEY" => Self::ProcessingContentKey,
            "CONTENT_KEY_FAILURE" => Self::ContentKeyFailure,
            "ATTEMPTING_AUTHENTICATION" => Self::AttemptingAuthentication,
            "FAILED_AUTHENTICATION" => Self::FailedAuthentication,
            "NOT_AUTHENTICATED" => Self::NotAuthenticated,
            "DEVICE_REVOCATION" => Self::DeviceRevocation,
            "DRM_SYSTEM_NOT_SUPPORTED" => Self::DrmSystemNotSupported,
            "LICENSE_DENIED" => Self::LicenseDenied,
            "LICENSE_EXPIRED" => Self::LicenseExpired,
            "LICENSE_INSUFFICIENT" => Self::LicenseInsufficient,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for DRMState {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Ok => "OK",
            Self::Unknown => "UNKNOWN",
            Self::ProcessingContentKey => "PROCESSING_CONTENT_KEY",
            Self::ContentKeyFailure => "CONTENT_KEY_FAILURE",
            Self::AttemptingAuthentication => "ATTEMPTING_AUTHENTICATION",
            Self::FailedAuthentication => "FAILED_AUTHENTICATION",
            Self::NotAuthenticated => "NOT_AUTHENTICATED",
            Self::DeviceRevocation => "DEVICE_REVOCATION",
            Self::DrmSystemNotSupported => "DRM_SYSTEM_NOT_SUPPORTED",
            Self::LicenseDenied => "LICENSE_DENIED",
            Self::LicenseExpired => "LICENSE_EXPIRED",
            Self::LicenseInsufficient => "LICENSE_INSUFFICIENT",
        };
        msg.write_xml(w)
    }
}
/// This state variable indicates a high-precision time offset that is used to adjust the
/// actual timing of the ConnectionManager CLOCKSYNC feature for a specific instance.
///
/// This CONDITIONALLY REQUIRED state variable MUST be supported if the `AVTransport` service
/// implements `GetSyncOffset` and `SetSyncOffset` actions. Note that if either action is implemented, both
/// MUST be implemented.
///
/// Its value is used to
/// automatically and uniformly shift all of the presentation time values that are associated with the
/// [ConnectionManager](crate::connection_manager) CLOCKSYNC feature.
///
/// Some examples include the `RelativePresentationTime` input
/// argument of the `SyncPlay` action or the presentation timestamps associated with a content stream.
///
/// A positive value indicates that the relevant time-of-day value(s) MUST be increased by the specified
/// amount, thus, causing a slight delay.
///
/// Conversely, a negative value indicates that the relevant time-of-day
/// value(s) MUST be decreased by the specified amount, thus, causing the associated effect to occur sooner
/// than would have otherwise occurred.
///
/// Learn more about the format of this state variable in the specification.
#[derive(Debug)]
pub struct SyncOffset;

impl SVariable for SyncOffset {
    type VarType = String;

    const VAR_NAME: &str = "SyncOffset";
}

/// This REQUIRED state variable is introduced to provide type information for the Unit argument in action `Seek()`.
///
/// It indicates the allowed units in which the amount of seeking to be performed is specified. It can be
/// specified as a time (relative or absolute), a count (relative or absolute), a track number, a tape-index (for
/// example, for tapes with an indexing facility; relative or absolute) or even a video frame (relative or
/// absolute).
///
/// A device vendor is allowed to implement a subset of the allowed value list of this state variable.
///
/// Only the value “TRACK_NR” is REQUIRED.
#[derive(Debug, Clone, Copy)]
pub enum ArgSeekMode {
    TrackNr,
    AbsTime,
    RelTime,
    AbsCount,
    RelCount,
    ChannelFreq,
    Tape,
    RelTape,
    Frame,
    RelFrame,
}

impl SVariable for ArgSeekMode {
    type VarType = Self;

    const VAR_NAME: &str = "A_ARG_TYPE_SeekMode";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "TRACK_NR",
        "ABS_TIME",
        "REL_TIME",
        "ABS_COUNT",
        "REL_COUNT",
        "CHANNEL_FREQ",
        "TAPE",
        "REL_TAPE",
        "FRAME",
        "REL_FRAME",
    ]);
}

impl IntoUpnpValue for ArgSeekMode {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "TRACK_NR" => Self::TrackNr,
            "ABS_TIME" => Self::AbsTime,
            "REL_TIME" => Self::RelTime,
            "ABS_COUNT" => Self::AbsCount,
            "REL_COUNT" => Self::RelCount,
            "CHANNEL_FREQ" => Self::ChannelFreq,
            "TAPE" => Self::Tape,
            "REL_TAPE" => Self::RelTape,
            "FRAME" => Self::Frame,
            "REL_FRAME" => Self::RelFrame,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for ArgSeekMode {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::TrackNr => "TRACK_NR",
            Self::AbsTime => "ABS_TIME",
            Self::RelTime => "REL_TIME",
            Self::AbsCount => "ABS_COUNT",
            Self::RelCount => "REL_COUNT",
            Self::ChannelFreq => "CHANNEL_FREQ",
            Self::Tape => "TAPE",
            Self::RelTape => "REL_TAPE",
            Self::Frame => "FRAME",
            Self::RelFrame => "REL_FRAME",
        };
        msg.write_xml(w)
    }
}

/// This REQUIRED state variable is introduced to provide type information for the Target argument in action
/// `Seek`.
///
/// It indicates the target position of the `Seek` action, in terms of units defined by state variable
/// [ArgSeekMode]. The data type of this variable is string.
///
/// However, depending on the actual seek
/// mode used, it MUST contain string representations of values as defined in the following table:
///
/// | SeekMode                                 | SeekTarget              |
/// | ---------------------------------------- | ----------------------- |
/// | [TRACK_NR](ArgSeekMode::TrackNr)         | [u32]                   |
/// | [ABS_TIME](ArgSeekMode::AbsTime)         | [UpnpDuration]          |
/// | [REL_TIME](ArgSeekMode::RelTime)         | [UpnpDuration]          |
/// | [ABS_COUNT](ArgSeekMode::AbsCount)       | [u32]                   |
/// | [REL_COUNT](ArgSeekMode::RelCount)       | [i32]                   |
/// | [CHANNEL_FREQ](ArgSeekMode::ChannelFreq) | [f32], expressed in Hz. |
/// | [TAPEINDEX](ArgSeekMode::Tape)           | [u32]                   |
/// | [REL_TAPEINDEX](ArgSeekMode::RelTape)    | [i32]                   |
/// | [FRAME](ArgSeekMode::Frame)              | [u32]                   |
/// | [REL_FRAME](ArgSeekMode::RelFrame)       | [i32]                   |
#[derive(Debug)]
pub struct ArgSeekTarget;

impl SVariable for ArgSeekTarget {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_SeekTarget";
}

/// This REQUIRED state variable is introduced to provide type information for the `InstanceID` input
/// argument present in all `AVTransport` actions.
///
/// It identifies the virtual instance of the `AVTransport` service to
/// which the action applies.
///
/// A valid `InstanceID` is obtained from a factory method in the ConnectionManager
/// service: the [PrepareForConnection](crate::connection_manager::ConnectionManagerService::prepare_for_connection) action.
///
/// If the device’s ConnectionManager does not implement the optional
/// [PrepareForConnection](crate::connection_manager::ConnectionManagerService::prepare_for_connection) action,
/// special value "0" MUST be used for the `InstanceID` input argument.
///
/// In such a case, the device implements a single static `AVTransport` instance, and only one
/// stream can be controlled and sent (or received) at any time.
#[derive(Debug)]
pub struct ArgInstanceID;

impl SVariable for ArgInstanceID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_InstanceID";
}

/// The state variable is introduced to provide type information for
/// the AVTransportUDN argument in that action
#[derive(Debug)]
pub struct DeviceUDN(pub Udn);

impl IntoUpnpValue for DeviceUDN {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        Udn::from_str(value).map(Self)
    }
}

impl IntoXml for DeviceUDN {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        self.0.to_string().write_xml(w)
    }
}

impl SVariable for DeviceUDN {
    type VarType = Self;

    const VAR_NAME: &str = "A_ARG_TYPE_DeviceUDN";
}

/// The state variable is introduced to provide type information for
/// the `ServiceType` argument in `SetStateVariables` action
#[derive(Debug)]
pub struct ServiceType;

impl SVariable for ServiceType {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_ServiceType";
}

/// The state variable is introduced to provide type information for
/// the `ServiceId` argument in `SetStateVariables` action.
#[derive(Debug)]
pub struct ServiceID;

impl SVariable for ServiceID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_ServiceID";
}

pub mod statevariable_value_pairs {
    use quick_xml::events::{BytesStart, Event};

    use crate::{
        FromXml, IntoXml, XmlReaderExt,
        service_variables::{IntoUpnpValue, SVariable},
    };

    #[derive(Debug)]
    struct StateVariable {
        name: String,
        value: String,
    }

    impl IntoXml for StateVariable {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            w.create_element(&self.name)
                .write_inner_content(|w| self.value.write_xml(w))?;
            Ok(())
        }
    }

    impl StateVariable {
        fn read_xml(
            r: &mut quick_xml::Reader<&[u8]>,
            start: quick_xml::name::QName,
        ) -> anyhow::Result<Self> {
            let value = r.read_text(start)?.to_string();
            let name = String::from_utf8(start.local_name().as_ref().to_owned())?;
            Ok(Self { name, value })
        }
    }

    /// This state variable contains a list of state variable
    /// names and their values.
    ///
    /// The list of state variables whose name/value pair is requested is given by another
    /// argument to the action.
    ///
    /// This CONDITIONALLY REQUIRED state variable MUST be supported if the `AVTransport` service
    /// implements the `GetStateVariables` and `SetStateVariables` actions.
    ///
    /// Note that if either action is
    /// implemented, both MUST be implemented.
    ///
    /// The state variable is introduced to provide type information for
    /// the StateVariableValuePairs argument in that action.
    #[derive(Debug)]
    pub struct ArgStateVariableValuePairs {
        values: Vec<StateVariable>,
    }

    impl IntoXml for ArgStateVariableValuePairs {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let start = BytesStart::new("stateVariableValuePairs").with_attributes([
                ("xmlns", "urn:schemas-upnp-org:av:avs"),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                (
                    "xsi:schemaLocation",
                    "
urn:schemas-upnp-org:av:avs
http://www.upnp.org/schemas/av/avs.xsd",
                ),
            ]);

            let end = start.clone();
            let end = end.to_end();

            w.write_event(Event::Start(start))?;

            for value in &self.values {
                value.write_xml(w)?;
            }

            w.write_event(Event::End(end))
        }
    }

    impl<'a> FromXml<'a> for ArgStateVariableValuePairs {
        fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
            let (is_empty, start) = r.read_to_start_or_empty()?;
            anyhow::ensure!(start.local_name().as_ref() == b"stateVariableValuePairs");
            let mut values = Vec::new();
            if is_empty {
                return Ok(Self { values });
            }
            while let Ok(event) = r.read_event() {
                match event {
                    Event::Start(bytes_start) => {
                        values.push(StateVariable::read_xml(r, bytes_start.name())?);
                    }
                    Event::End(bytes_end) => {
                        anyhow::ensure!(bytes_end == start.to_end());
                        break;
                    }
                    Event::Text(_) => {}
                    r => Err(anyhow::anyhow!(
                        "expected variable or pairs end, got {:?}",
                        r
                    ))?,
                }
            }
            Ok(Self { values })
        }
    }

    impl IntoUpnpValue for ArgStateVariableValuePairs {
        fn from_xml_value(value: &str) -> anyhow::Result<Self> {
            Self::read_xml(&mut quick_xml::Reader::from_str(value))
        }
    }

    impl SVariable for ArgStateVariableValuePairs {
        type VarType = Self;

        const VAR_NAME: &str = "A_ARG_TYPE_StateVariableValuePairs";
    }
}
/// CSV list of state variable names.
///
/// The state variable is introduced to provide type information for
/// the StateVariableList argument in that action.
///
/// This variable MAY
/// contain one or more (as required) of the defined AVTransport state variable names except LastChange and
/// any A_ARG_TYPE_xxx state variable names.
///
/// The asterisk ("*") can be specified to indicate all relevant
/// variable names (excluding LastChange and any A_ARG_TYPE_xxx state variables.)
#[derive(Debug)]
pub struct ArgStateVariableList;

impl SVariable for ArgStateVariableList {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_StateVariableList";
}

/// This state variable is introduced to provide a chunk of a playlist document to the device
#[derive(Debug)]
pub struct ArgPlaylistData;

impl SVariable for ArgPlaylistData {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistData";
}

/// This state variable is introduced to indicate the chunk’s length of the playlist document
#[derive(Debug)]
pub struct ArgPlaylistDataLength;

impl SVariable for ArgPlaylistDataLength {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistDataLength";
}

/// This state variable is introduced to provide a zero-based offset into the playlist document being passed to the renderer.
#[derive(Debug)]
pub struct ArgPlaylistOffset;

impl SVariable for ArgPlaylistOffset {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistOffset";
}

/// This state variable is introduced to provide the total length of the entire playlist document.
#[derive(Debug)]
pub struct ArgPlaylistTotalLength;

impl SVariable for ArgPlaylistTotalLength {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistTotalLength";
}

/// This state variable is introduced to provide the `MIME` type of the playlist provided to the device
#[derive(Debug)]
pub struct ArgPlaylistMIMEType;

impl SVariable for ArgPlaylistMIMEType {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistMIMEType";
}

/// This state variable is introduced
/// to provide extended type information of the playlist provided to the device.
///
/// The value of this argument
/// corresponds to the contents of the `res@protocolInfo` property 4th field
///
/// See `Content Directory` service for more on `protocolInfo`
#[derive(Debug)]
pub struct ArgPlaylistExtendedType;

impl SVariable for ArgPlaylistExtendedType {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistExtendedType";
}

/// This state variable is introduced to provide step information
/// for a streaming playlist operation
#[derive(Debug)]
pub enum ArgPlaylistStep {
    ///  Indicates that this is the start of streaming playlist operation.
    Initial,
    /// Indicates that this is a continuation of a streaming playlist operation.
    Continue,
    /// Indicates that the current streaming playlist operation will end when all
    ///pending playlist data at the device is consumed.
    Stop,
    /// Indicates that processing of the current streaming playlist ends
    /// immediately. Any pending playlist data for the streaming playlist is
    /// discarded,
    Reset,
}

impl IntoUpnpValue for ArgPlaylistStep {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "Initial" => Self::Initial,
            "Continue" => Self::Continue,
            "Stop" => Self::Stop,
            "Reset" => Self::Reset,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for ArgPlaylistStep {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Initial => "Initial",
            Self::Continue => "Continue",
            Self::Stop => "Stop",
            Self::Reset => "Reset",
        };
        msg.write_xml(w)
    }
}

impl SVariable for ArgPlaylistStep {
    type VarType = Self;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistStep";

    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["Initial", "Continue", "Stop", "Reset"]);
}

/// This state variable describes the playlist types supported by the implementation
#[derive(Debug)]
pub enum ArgPlaylistType {
    Static,
    Streaming,
}

impl IntoUpnpValue for ArgPlaylistType {
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        let out = match value {
            "Static" => Self::Static,
            "Streaming" => Self::Streaming,
            _ => Err(anyhow::anyhow!("Unrecognized value: {value}"))?,
        };
        Ok(out)
    }
}

impl IntoXml for ArgPlaylistType {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Static => "Static",
            Self::Streaming => "Streaming",
        };
        msg.write_xml(w)
    }
}

impl SVariable for ArgPlaylistType {
    type VarType = Self;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistType";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["Static", "Streaming"]);
}

pub mod arg_playlist_info {
    /// This state variable is a document detailing whether the implementation can play the indicated item formats
    #[derive(Debug)]
    pub enum PlaylistInfo {
        Streaming(StreamingPlaylistInfo),
        Static(StaticPlaylistInfo),
    }

    #[derive(Debug)]
    pub struct StreamingPlaylistInfo {}

    #[derive(Debug)]
    pub struct StaticPlaylistInfo {}
}

/// This argument provides a starting object `@id` property value for playlists which employ object linking properties
///
/// For playlists that do not employ `objectLinking` properties this state variable SHOULD be set to "".
#[derive(Debug)]
pub struct ArgPlaylistStartObjID;

impl SVariable for ArgPlaylistStartObjID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistStartObjID";
}

/// This argument provides a starting target group ID
/// objectLink@groupID value for playlists which employ object linking properties
///
/// For playlists that do not employ `objectLinking` properties this state variable SHOULD be set to "".
#[derive(Debug)]
pub struct ArgPlaylistStartGroupID;

impl SVariable for ArgPlaylistStartGroupID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistStartGroupID";
}

/// This state variable indicates a high-precision time offset that is
/// used to adjust the actual timing of the ConnectionManager CLOCKSYNC feature for a specific instance.
#[derive(Debug)]
pub struct ArgSyncOffsetAdj;

impl SVariable for ArgSyncOffsetAdj {
    type VarType = UpnpDuration;

    const VAR_NAME: &str = "A_ARG_TYPE_SyncOffsetAdj";
}

/// This state variable is introduced to provide
/// type information for the `ReferencePresentationTime` and other similar input arguments for `AVTransport`
/// actions related to CLOCKSYNC feature.
///
/// It represents a high-precision point in time (corresponding to a
/// specific time on a specific day) that is used to designate the exact time when certain time-sensitive
/// operations are to be performed.
#[derive(Debug)]
pub struct ArgPresentationTime;

impl SVariable for ArgPresentationTime {
    type VarType = UpnpDuration;

    const VAR_NAME: &str = "A_ARG_TYPE_PresentationTime";
}

/// This state variable is introduced to provide
/// type information for the `ReferenceClockId` input argument for `AVTransport` actions related to
/// CLOCKSYNC feature.
#[derive(Debug)]
pub struct ArgClockId;

impl SVariable for ArgClockId {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_ClockId";
}

// Client

#[derive(Debug)]
pub struct AvTransportClient;

#[derive(Debug)]
pub struct PositionInfo {
    pub track: u32,
    pub duration: std::time::Duration,
    pub url: reqwest::Url,
    pub rel_time: std::time::Duration,
    pub abs_time: std::time::Duration,
}

impl ScpdService for AvTransportClient {
    const URN: crate::urn::URN = URN {
        version: 1,
        urn_type: UrnType::Service(crate::urn::ServiceType::AVTransport),
    };
}

impl ScpdClient<AvTransportClient> {
    /// Start playing the resource
    ///
    /// This REQUIRED action starts playing the resource of the specified instance, at the specified speed, starting
    /// at the current position, according to the current play mode.
    ///
    /// Playing MUST continue until the resource ends
    /// or the transport state is changed via actions `Stop` or [Pause](ScpdClient::pause).
    ///
    /// The device MUST do a best effort to match the specified play speed.
    ///
    /// Actually supported speeds can be retrieved from the `AllowedValueList` of the
    /// [TransportPlaySpeed] state variable in the `AVTransport` service description.
    ///
    /// If no [AVTransportURI] is set, the resource being played is device-dependent
    pub async fn play(&self, speed: &str) -> Result<(), ActionCallError> {
        let action = self.action("Play")?;
        let payload = action.av_play("0".into(), speed)?;
        () = self.run_action(action, payload).await.unwrap();
        Ok(())
    }

    /// Pause the playback
    ///
    /// This is an OPTIONAL action. While the device is in a playing state, that is: `TransportState` is [TransportState::Playing],
    /// this action halts the progression of the resource that is associated with the specified `InstanceID`.
    ///
    /// Any visual representation of the resource SHOULD remain displayed in a static manner (for example, the last frame of
    /// video remains displayed).
    ///
    /// Any audio representation of the resource SHOULD be muted.
    ///
    /// The difference between `Pause` and `Stop` actions is that `Pause` MUST remain at the current position within the resource and
    /// the current resource MUST persist as described above (for example, the current video resource continues to
    /// be transmitted/displayed).
    pub async fn pause(&self) -> Result<(), ActionCallError> {
        let action = self.action("Pause")?;
        let payload = action.av_pause("0".into())?;
        () = self.run_action(action, payload).await?;
        Ok(())
    }

    pub async fn seek(&self, duration: impl Into<UpnpDuration>) -> Result<(), ActionCallError> {
        let action = self.action("Seek")?;
        let upnp_duration: UpnpDuration = duration.into();
        let payload =
            action.av_seek("0".into(), ArgSeekMode::RelTime, upnp_duration.to_string())?;
        () = self.run_action(action, payload).await?;
        Ok(())
    }

    /// This REQUIRED action returns information associated with the current position of the transport of the
    /// specified instance.
    ///
    /// It has no effect on state.
    pub async fn position_info(&self) -> Result<PositionInfo, ActionCallError> {
        let action = self.action("GetPositionInfo")?;
        let payload = action.av_position_info("0".into())?;
        let (track, duration, _, url, rel_time, abs_time, _, _): (
            <CurrentTrack as SVariable>::VarType,
            <CurrentTrackDuration as SVariable>::VarType,
            <CurrentTrackMetaData as SVariable>::VarType,
            <CurrentTrackURI as SVariable>::VarType,
            <RelativeTimePosition as SVariable>::VarType,
            <AbsoluteTimePosition as SVariable>::VarType,
            <RelativeCounterPosition as SVariable>::VarType,
            <AbsoluteCounterPosition as SVariable>::VarType,
        ) = self.run_action(action, payload).await?;
        Ok(PositionInfo {
            track,
            duration: duration.into(),
            url,
            rel_time: rel_time.into(),
            abs_time: abs_time.into(),
        })
    }
}

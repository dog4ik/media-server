use std::str::FromStr;

use crate::{
    content_directory::UpnpDuration,
    device_description::Udn,
    service_client::{ActionCallError, ScpdClient, ScpdService},
    service_variables::{IntoUpnpValue, SVariable},
    urn::{UrnType, URN},
    IntoXml,
};

#[derive(Debug)]
pub enum TransportState {
    Stopped,
    Playing,
    Transitioning,
    PausedPlayback,
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

#[derive(Debug)]
pub struct RecordStorageMedium;

impl SVariable for RecordStorageMedium {
    type VarType = String;

    const VAR_NAME: &str = "RecordStorageMedium";
}

#[derive(Debug)]
pub struct PossiblePlaybackStorageMedia;

impl SVariable for PossiblePlaybackStorageMedia {
    type VarType = String;

    const VAR_NAME: &str = "PossiblePlaybackStorageMedia";
}

#[derive(Debug)]
pub struct PossibleRecordStorageMedia;

impl SVariable for PossibleRecordStorageMedia {
    type VarType = String;

    const VAR_NAME: &str = "PossibleRecordStorageMedia";
}

#[derive(Debug)]
pub enum CurrentPlayMode {
    Normal,
    Shuffle,
    RepeatOne,
    RepeatAll,
    Random,
    Direct1,
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
/// relative to normal speed. Example values are “1”, “1/2”, “2”, “-1”, “1/10”, etc.
#[derive(Debug)]
pub struct TransportPlaySpeed;

impl SVariable for TransportPlaySpeed {
    type VarType = String;

    const VAR_NAME: &str = "TransportPlaySpeed";
}

#[derive(Debug)]
pub enum RecordMediumWriteStatus {
    Writable,
    Protected,
    NotWritable,
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

#[derive(Debug)]
struct PossibleRecordQualityModes;

impl SVariable for PossibleRecordQualityModes {
    type VarType = String;

    const VAR_NAME: &str = "PossibleRecordQualityModes";
}

#[derive(Debug)]
struct NumberOfTracks;

impl SVariable for NumberOfTracks {
    type VarType = u32;
    const VAR_NAME: &str = "NumberOfTracks";
}

#[derive(Debug)]
struct CurrentTrack;

impl SVariable for CurrentTrack {
    type VarType = u32;
    const VAR_NAME: &str = "CurrentTrack";
}

#[derive(Debug)]
struct CurrentTrackDuration;

impl SVariable for CurrentTrackDuration {
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "CurrentTrackDuration";
}

#[derive(Debug)]
struct CurrentMediaDuration;

impl SVariable for CurrentMediaDuration {
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "CurrentMediaDuration";
}

#[derive(Debug)]
struct CurrentTrackMetaData;

impl SVariable for CurrentTrackMetaData {
    type VarType = String;
    const VAR_NAME: &str = "CurrentTrackMetaData";
}

#[derive(Debug)]
struct CurrentTrackURI;

impl SVariable for CurrentTrackURI {
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "CurrentTrackURI";
}

#[derive(Debug)]
struct AVTransportURI;

impl SVariable for AVTransportURI {
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "AVTransportURI";
}

#[derive(Debug)]
struct AVTransportURIMetaData;

impl SVariable for AVTransportURIMetaData {
    type VarType = String;
    const VAR_NAME: &str = "AVTransportURIMetaData";
}

#[derive(Debug)]
struct NextAVTransportURI;

impl SVariable for NextAVTransportURI {
    // TODO: Handle NOT_IMPLEMENTED value
    type VarType = reqwest::Url;
    const VAR_NAME: &str = "NextAVTransportURI";
}

#[derive(Debug)]
struct NextAVTransportURIMetaData;

impl SVariable for NextAVTransportURIMetaData {
    type VarType = String;
    const VAR_NAME: &str = "NextAVTransportURIMetaData";
}

#[derive(Debug)]
struct RelativeTimePosition;

impl SVariable for RelativeTimePosition {
    // TODO: Handle NOT_IMPLEMENTED value
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "RelativeTimePosition";
}

#[derive(Debug)]
struct AbsoluteTimePosition;

impl SVariable for AbsoluteTimePosition {
    // TODO: Handle NOT_IMPLEMENTED | END_OF_MEDIA values
    type VarType = UpnpDuration;
    const VAR_NAME: &str = "AbsoluteTimePosition";
}

#[derive(Debug)]
struct RelativeCounterPosition;

impl SVariable for RelativeCounterPosition {
    type VarType = i32;
    const VAR_NAME: &str = "RelativeCounterPosition";
}

#[derive(Debug)]
struct AbsoluteCounterPosition;

impl SVariable for AbsoluteCounterPosition {
    type VarType = u32;
    const VAR_NAME: &str = "AbsoluteCounterPosition";
}

#[derive(Debug)]
pub enum CurrentTransportActions {
    Play,
    Stop,
    Pause,
    Seek,
    Next,
    Previous,
    Record,
}

impl IntoUpnpValue for CurrentTransportActions {
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

impl IntoXml for CurrentTransportActions {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Self::Play => "PLAY",
            Self::Stop => "STOP",
            Self::Pause => "PAUSE",
            Self::Seek => "SEEK",
            Self::Next => "NEXT",
            Self::Previous => "PREVIOUS",
            Self::Record => "RECORD",
        };
        msg.write_xml(w)
    }
}

impl SVariable for CurrentTransportActions {
    type VarType = Self;
    const VAR_NAME: &str = "CurrentTransportActions";
}

#[derive(Debug)]
pub struct LastChange;

impl SVariable for LastChange {
    type VarType = String;

    const VAR_NAME: &str = "LastChange";
}

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

#[derive(Debug)]
pub struct SyncOffset;

impl SVariable for SyncOffset {
    type VarType = String;

    const VAR_NAME: &str = "SyncOffset";
}

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

#[derive(Debug)]
pub struct ArgSeekTarget;

impl SVariable for ArgSeekTarget {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_SeekTarget";
}

#[derive(Debug)]
pub struct ArgInstanceID;

impl SVariable for ArgInstanceID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_InstanceID";
}

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

#[derive(Debug)]
pub struct ServiceType;

impl SVariable for ServiceType {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_ServiceType";
}

#[derive(Debug)]
pub struct ServiceID;

impl SVariable for ServiceID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_ServiceID";
}

mod statevariable_value_pairs {
    use quick_xml::events::{BytesStart, Event};

    use crate::{
        service_variables::{IntoUpnpValue, SVariable},
        FromXml, IntoXml, XmlReaderExt,
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

#[derive(Debug)]
pub struct ArgStateVariableList;

impl SVariable for ArgStateVariableList {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_StateVariableList";
}

#[derive(Debug)]
pub struct ArgPlaylistData;

impl SVariable for ArgPlaylistData {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistData";
}

#[derive(Debug)]
pub struct ArgPlaylistDataLength;

impl SVariable for ArgPlaylistDataLength {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistDataLength";
}

#[derive(Debug)]
pub struct ArgPlaylistOffset;

impl SVariable for ArgPlaylistOffset {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistOffset";
}

#[derive(Debug)]
pub struct ArgPlaylistTotalLength;

impl SVariable for ArgPlaylistTotalLength {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistTotalLength";
}

#[derive(Debug)]
pub struct ArgPlaylistMIMEType;

impl SVariable for ArgPlaylistMIMEType {
    type VarType = u32;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistMIMEType";
}

#[derive(Debug)]
pub struct ArgPlaylistExtendedType;

impl SVariable for ArgPlaylistExtendedType {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistExtendedType";
}

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

#[derive(Debug)]
pub struct ArgPlaylistStartObjID;

impl SVariable for ArgPlaylistStartObjID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistStartObjID";
}

#[derive(Debug)]
pub struct ArgPlaylistStartGroupID;

impl SVariable for ArgPlaylistStartGroupID {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PlaylistStartGroupID";
}

#[derive(Debug)]
pub struct ArgSyncOffsetAdj;

impl SVariable for ArgSyncOffsetAdj {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_SyncOffsetAdj";
}

#[derive(Debug)]
pub struct ArgPresentationTime;

impl SVariable for ArgPresentationTime {
    type VarType = String;

    const VAR_NAME: &str = "A_ARG_TYPE_PresentationTime";
}

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
    pub async fn play(&self, speed: &str) -> Result<(), ActionCallError> {
        let action = self.action("Play")?;
        let payload = action.av_play("0".into(), speed)?;
        () = self.run_action(action, payload).await.unwrap();
        Ok(())
    }

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

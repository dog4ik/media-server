use serde::Serialize;
use std::ffi::OsStr;

#[derive(Debug, Serialize, Clone, Copy, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoContainer {
    Avi,
    Mkv,
    Mov,
    Mp4,
    Ogg,
    Webm,
}

impl<'a> TryFrom<&'a OsStr> for VideoContainer {
    type Error = anyhow::Error;
    fn try_from(value: &'a OsStr) -> Result<Self, Self::Error> {
        let value = value.to_string_lossy();
        match value.as_bytes() {
            b"avi" => Ok(Self::Avi),
            b"mkv" => Ok(Self::Mkv),
            b"mov" => Ok(Self::Mov),
            b"mp4" => Ok(Self::Mp4),
            b"ogg" => Ok(Self::Ogg),
            b"webm" => Ok(Self::Webm),
            _ => Err(anyhow::format_err!("unsupported container type: {}", value)),
        }
    }
}

impl VideoContainer {
    pub fn mime_type(&self) -> &'static str {
        match self {
            VideoContainer::Avi => "video/x-msvideo",
            VideoContainer::Mkv => "video/x-matroska",
            VideoContainer::Mov => "video/quicktime",
            VideoContainer::Mp4 => "video/mp4",
            VideoContainer::Ogg => "video/ogg",
            VideoContainer::Webm => "video/webm",
        }
    }
}

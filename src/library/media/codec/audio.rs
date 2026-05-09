use serde::{Deserialize, Serialize};
use std::{convert::Infallible, fmt::Display, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    AAC,
    AC3,
    EAC3,
    DTS,
    FLAC,
    Opus,
    Other(String),
}

impl Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AAC => write!(f, "aac"),
            Self::AC3 => write!(f, "ac3"),
            Self::EAC3 => write!(f, "eac3"),
            Self::DTS => write!(f, "dts"),
            Self::FLAC => write!(f, "flack"),
            Self::Opus => write!(f, "opus"),
            Self::Other(codec) => write!(f, "{codec}"),
        }
    }
}

impl FromStr for AudioCodec {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = match s {
            "aac" => AudioCodec::AAC,
            "ac3" => AudioCodec::AC3,
            "eac3" => AudioCodec::EAC3,
            "dts" => AudioCodec::DTS,
            "flack" => AudioCodec::FLAC,
            "opus" => AudioCodec::Opus,
            _ => AudioCodec::Other(s.to_string()),
        };
        Ok(parsed)
    }
}

impl<'de> Deserialize<'de> for AudioCodec {
    fn deserialize<D>(deserializer: D) -> Result<AudioCodec, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AudioCodecVisitor;

        impl serde::de::Visitor<'_> for AudioCodecVisitor {
            type Value = AudioCodec;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an audio codec string")
            }

            fn visit_str<E>(self, value: &str) -> Result<AudioCodec, E>
            where
                E: serde::de::Error,
            {
                Ok(AudioCodec::from_str(value).expect("any str to be valid"))
            }
        }

        deserializer.deserialize_str(AudioCodecVisitor)
    }
}

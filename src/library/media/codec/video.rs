use crate::ffmpeg_abi::{Av1Encoder, H264Encoder, HevcEncoder};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, fmt::Display, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    Hevc,
    H264,
    Av1,
    VP8,
    VP9,
    Other(String),
}

impl VideoCodec {
    /// Try to get Hardware accelerated encoder for given API
    pub async fn gpu_accelerated_encoder(&self) -> Option<&'static str> {
        let apis = crate::ffmpeg_abi::get_or_init_gpu_accelated_apis().await;
        let api = *apis.first()?;
        match self {
            VideoCodec::Hevc => HevcEncoder::gpu_accelerated(api).map(|e| e.as_str()),
            VideoCodec::H264 => H264Encoder::gpu_accelerated(api).map(|e| e.as_str()),
            VideoCodec::Av1 => Av1Encoder::gpu_accelerated(api).map(|e| e.as_str()),
            VideoCodec::VP8 => None,
            VideoCodec::VP9 => None,
            VideoCodec::Other(_) => None,
        }
    }

    /// Encoder with the "best" chances to work.
    pub fn default_encoder(&self) -> &str {
        match self {
            VideoCodec::Hevc => HevcEncoder::default().as_str(),
            VideoCodec::H264 => H264Encoder::default().as_str(),
            VideoCodec::Av1 => Av1Encoder::default().as_str(),
            VideoCodec::VP8 => "vp8",
            VideoCodec::VP9 => "vp9",
            VideoCodec::Other(o) => o,
        }
    }
}

impl Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hevc => f.write_str("hevc"),
            Self::H264 => f.write_str("h264"),
            Self::Av1 => f.write_str("av1"),
            Self::VP8 => f.write_str("vp8"),
            Self::VP9 => f.write_str("vp9"),
            Self::Other(codec) => write!(f, "{codec}"),
        }
    }
}

impl FromStr for VideoCodec {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "hevc" => VideoCodec::Hevc,
            "h264" => VideoCodec::H264,
            "av1" => VideoCodec::Av1,
            "vp8" => VideoCodec::VP8,
            "vp9" => VideoCodec::VP9,
            _ => VideoCodec::Other(s.to_string()),
        })
    }
}

impl<'de> Deserialize<'de> for VideoCodec {
    fn deserialize<D>(deserializer: D) -> Result<VideoCodec, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct VideoCodecVisitor;

        impl serde::de::Visitor<'_> for VideoCodecVisitor {
            type Value = VideoCodec;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an video codec string")
            }

            fn visit_str<E>(self, value: &str) -> Result<VideoCodec, E>
            where
                E: serde::de::Error,
            {
                Ok(VideoCodec::from_str(value).expect("any str to be valid"))
            }
        }

        deserializer.deserialize_str(VideoCodecVisitor)
    }
}

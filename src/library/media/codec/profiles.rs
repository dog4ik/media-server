
#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AAC {
    #[default]
    Main,
    Low,
    SSR,
    LTP,
    HE,
    HEv2,
    LD,
    ELD,
    MPEG2Low,
    MPEG2HE,
}

impl From<ffmpeg_next::codec::profile::AAC> for AAC {
    fn from(value: ffmpeg_next::codec::profile::AAC) -> Self {
        match value {
            ffmpeg_next::codec::profile::AAC::Main => Self::Main,
            ffmpeg_next::codec::profile::AAC::Low => Self::Low,
            ffmpeg_next::codec::profile::AAC::SSR => Self::SSR,
            ffmpeg_next::codec::profile::AAC::LTP => Self::LTP,
            ffmpeg_next::codec::profile::AAC::HE => Self::HE,
            ffmpeg_next::codec::profile::AAC::HEv2 => Self::HEv2,
            ffmpeg_next::codec::profile::AAC::LD => Self::LD,
            ffmpeg_next::codec::profile::AAC::ELD => Self::ELD,
            ffmpeg_next::codec::profile::AAC::MPEG2Low => Self::MPEG2Low,
            ffmpeg_next::codec::profile::AAC::MPEG2HE => Self::MPEG2HE,
        }
    }
}

#[allow(non_camel_case_types)]
#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DTS {
    #[default]
    Default,
    ES,
    _96_24,
    HD_HRA,
    HD_MA,
    Express,
}

impl From<ffmpeg_next::codec::profile::DTS> for DTS {
    fn from(value: ffmpeg_next::codec::profile::DTS) -> Self {
        match value {
            ffmpeg_next::codec::profile::DTS::Default => Self::Default,
            ffmpeg_next::codec::profile::DTS::ES => Self::ES,
            ffmpeg_next::codec::profile::DTS::_96_24 => Self::_96_24,
            ffmpeg_next::codec::profile::DTS::HD_HRA => Self::HD_HRA,
            ffmpeg_next::codec::profile::DTS::HD_MA => Self::HD_MA,
            ffmpeg_next::codec::profile::DTS::Express => Self::Express,
        }
    }
}

#[allow(unused)]
#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MPEG2 {
    _422,
    High,
    SS,
    SNRScalable,
    #[default]
    Main,
    Simple,
}

impl From<ffmpeg_next::codec::profile::MPEG2> for MPEG2 {
    fn from(value: ffmpeg_next::codec::profile::MPEG2) -> Self {
        match value {
            ffmpeg_next::codec::profile::MPEG2::_422 => Self::_422,
            ffmpeg_next::codec::profile::MPEG2::High => Self::High,
            ffmpeg_next::codec::profile::MPEG2::SS => Self::SS,
            ffmpeg_next::codec::profile::MPEG2::SNRScalable => Self::SNRScalable,
            ffmpeg_next::codec::profile::MPEG2::Main => Self::Main,
            ffmpeg_next::codec::profile::MPEG2::Simple => Self::Simple,
        }
    }
}

#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum H264 {
    Constrained,
    Intra,
    #[default]
    Baseline,
    ConstrainedBaseline,
    Main,
    Extended,
    High,
    High10,
    High10Intra,
    High422,
    High422Intra,
    High444,
    High444Predictive,
    High444Intra,
    CAVLC444,
}

impl From<ffmpeg_next::codec::profile::H264> for H264 {
    fn from(value: ffmpeg_next::codec::profile::H264) -> Self {
        match value {
            ffmpeg_next::codec::profile::H264::Constrained => Self::Constrained,
            ffmpeg_next::codec::profile::H264::Intra => Self::Intra,
            ffmpeg_next::codec::profile::H264::Baseline => Self::Baseline,
            ffmpeg_next::codec::profile::H264::ConstrainedBaseline => Self::ConstrainedBaseline,
            ffmpeg_next::codec::profile::H264::Main => Self::Main,
            ffmpeg_next::codec::profile::H264::Extended => Self::Extended,
            ffmpeg_next::codec::profile::H264::High => Self::High,
            ffmpeg_next::codec::profile::H264::High10 => Self::High10,
            ffmpeg_next::codec::profile::H264::High10Intra => Self::High10Intra,
            ffmpeg_next::codec::profile::H264::High422 => Self::High422,
            ffmpeg_next::codec::profile::H264::High422Intra => Self::High422Intra,
            ffmpeg_next::codec::profile::H264::High444 => Self::High444,
            ffmpeg_next::codec::profile::H264::High444Predictive => Self::High444Predictive,
            ffmpeg_next::codec::profile::H264::High444Intra => Self::High444Intra,
            ffmpeg_next::codec::profile::H264::CAVLC444 => Self::CAVLC444,
        }
    }
}

#[allow(unused)]
#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MPEG4 {
    Simple,
    SimpleScalable,
    Core,
    #[default]
    Main,
    NBit,
    ScalableTexture,
    SimpleFaceAnimation,
    BasicAnimatedTexture,
    Hybrid,
    AdvancedRealTime,
    CoreScalable,
    AdvancedCoding,
    AdvancedCore,
    AdvancedScalableTexture,
    SimpleStudio,
    AdvancedSimple,
}

#[rustfmt::skip]
    impl From<ffmpeg_next::codec::profile::MPEG4> for MPEG4 {
        fn from(value: ffmpeg_next::codec::profile::MPEG4) -> Self {
            match value {
                ffmpeg_next::codec::profile::MPEG4::Simple => Self::Simple,
                ffmpeg_next::codec::profile::MPEG4::SimpleScalable => Self::SimpleScalable,
                ffmpeg_next::codec::profile::MPEG4::Core => Self::Core,
                ffmpeg_next::codec::profile::MPEG4::Main => Self::Main,
                ffmpeg_next::codec::profile::MPEG4::NBit => Self::NBit,
                ffmpeg_next::codec::profile::MPEG4::ScalableTexture => Self::ScalableTexture,
                ffmpeg_next::codec::profile::MPEG4::SimpleFaceAnimation => Self::SimpleFaceAnimation,
                ffmpeg_next::codec::profile::MPEG4::BasicAnimatedTexture => Self::BasicAnimatedTexture,
                ffmpeg_next::codec::profile::MPEG4::Hybrid => Self::Hybrid,
                ffmpeg_next::codec::profile::MPEG4::AdvancedRealTime => Self::AdvancedRealTime,
                ffmpeg_next::codec::profile::MPEG4::CoreScalable => Self::CoreScalable,
                ffmpeg_next::codec::profile::MPEG4::AdvancedCoding => Self::AdvancedCoding,
                ffmpeg_next::codec::profile::MPEG4::AdvancedCore => Self::AdvancedCore,
                ffmpeg_next::codec::profile::MPEG4::AdvancedScalableTexture => Self::AdvancedScalableTexture,
                ffmpeg_next::codec::profile::MPEG4::SimpleStudio => Self::SimpleStudio,
                ffmpeg_next::codec::profile::MPEG4::AdvancedSimple => Self::AdvancedSimple,
            }
        }
    }

#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HEVC {
    #[default]
    Main,
    Main10,
    MainStillPicture,
    Rext,
}

impl From<ffmpeg_next::codec::profile::HEVC> for HEVC {
    fn from(value: ffmpeg_next::codec::profile::HEVC) -> Self {
        match value {
            ffmpeg_next::codec::profile::HEVC::Main => Self::Main,
            ffmpeg_next::codec::profile::HEVC::Main10 => Self::Main10,
            ffmpeg_next::codec::profile::HEVC::MainStillPicture => Self::MainStillPicture,
            ffmpeg_next::codec::profile::HEVC::Rext => Self::Rext,
        }
    }
}

#[derive(Eq, PartialEq, Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
pub enum VP9 {
    #[default]
    #[serde(rename = "0")]
    _0,
    #[serde(rename = "1")]
    _1,
    #[serde(rename = "2")]
    _2,
    #[serde(rename = "3")]
    _3,
}

impl From<ffmpeg_next::codec::profile::VP9> for VP9 {
    fn from(value: ffmpeg_next::codec::profile::VP9) -> Self {
        match value {
            ffmpeg_next::codec::profile::VP9::_0 => Self::_0,
            ffmpeg_next::codec::profile::VP9::_1 => Self::_1,
            ffmpeg_next::codec::profile::VP9::_2 => Self::_2,
            ffmpeg_next::codec::profile::VP9::_3 => Self::_3,
        }
    }
}

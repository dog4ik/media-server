use std::{path::Path, time::Duration};

use anyhow::Context;
use ffmpeg_next::{codec, format::stream::Disposition, media};
use tokio::sync::OnceCell;

use crate::library::{AudioCodec, Resolution, SubtitlesCodec, VideoCodec};

#[derive(Debug)]
pub struct Chapter {
    pub title: Option<String>,
    pub start: Duration,
    pub end: Duration,
}

impl Chapter {
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }
}

impl TryFrom<ffmpeg_next::Chapter<'_>> for Chapter {
    type Error = anyhow::Error;

    fn try_from(chapter: ffmpeg_next::Chapter<'_>) -> Result<Self, Self::Error> {
        let mut title = None;
        for (k, v) in &chapter.metadata() {
            match k {
                "title" => {
                    title = Some(v.to_owned());
                    break;
                }
                _ => {}
            }
        }
        let start = Duration::from_millis(
            chapter
                .start()
                .try_into()
                .context("convert start duration to u64")?,
        );
        let end = Duration::from_millis(
            chapter
                .end()
                .try_into()
                .context("convert end duration to u64")?,
        );
        Ok(Self { title, start, end })
    }
}

#[derive(Debug)]
pub struct Video {
    pub codec: VideoCodec,
    pub level: i32,
    pub profile: i32,
    pub avg_frame_rate: u32,
    pub bit_rate: usize,
    pub width: u32,
    pub height: u32,
}

impl Video {
    pub fn resolution(&self) -> Resolution {
        Resolution((self.width as usize, self.height as usize))
    }
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Video {
    type Error = anyhow::Error;

    fn try_from(stream: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        let params = stream.parameters();

        let (level, profile) = unsafe {
            let av_codec_params = params.as_ptr();
            if av_codec_params.is_null() {
                anyhow::bail!("av codec params is null");
            }
            ((*av_codec_params).level, (*av_codec_params).profile)
        };

        let codec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())?;
        let video = codec.decoder().video()?;
        let width = video.width();
        let height = video.height();
        let bit_rate = video.bit_rate();
        let codec = match (params.id(), video.profile()) {
            (codec::Id::H264, codec::Profile::H264(_)) => VideoCodec::H264,
            (codec::Id::H264, codec::Profile::Unknown) => VideoCodec::H264,
            (codec::Id::HEVC, codec::Profile::HEVC(_)) => VideoCodec::Hevc,
            (codec::Id::HEVC, codec::Profile::Unknown) => VideoCodec::Hevc,
            (codec::Id::VP9, codec::Profile::VP9(_)) => VideoCodec::VP9,
            (codec::Id::VP9, codec::Profile::Unknown) => VideoCodec::VP9,
            (codec::Id::VP8, _) => VideoCodec::VP8,
            (codec::Id::AV1, _) => VideoCodec::Av1,
            (codec, profile) => {
                tracing::warn!("Unrecognized video codec: {:?}/{:?}", codec, profile);
                return Err(anyhow::anyhow!(
                    "unrecognized video codec/profile configuration"
                ));
            }
        };

        let avg = stream.avg_frame_rate();
        let avg_frame_rate = (avg.0 / avg.1).try_into()?;

        Ok(Self {
            codec,
            level,
            profile,
            avg_frame_rate,
            bit_rate,
            width,
            height,
        })
    }
}

impl<'s, T> TryFrom<ffmpeg_next::Stream<'s>> for Track<T>
where
    T: TryFrom<ffmpeg_next::Stream<'s>, Error = anyhow::Error>,
{
    type Error = anyhow::Error;

    fn try_from(stream: ffmpeg_next::Stream<'s>) -> Result<Self, Self::Error> {
        let is_default = (stream.disposition() & Disposition::DEFAULT) == Disposition::DEFAULT;
        let index = stream.index();
        let stream = T::try_from(stream)?;
        Ok(Self {
            stream,
            is_default,
            index,
        })
    }
}

#[derive(Debug)]
pub struct Audio {
    pub codec: AudioCodec,
    pub channels: u16,
    pub sample_rate: u32,
    pub profile_idc: i32,
    pub bit_rate: usize,
    pub is_dub: bool,
    pub is_hearing_impaired: bool,
    pub is_visual_impaired: bool,
    pub language: Option<String>,
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Audio {
    type Error = anyhow::Error;

    fn try_from(stream: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        let disposition = stream.disposition();
        let is_dub = (disposition & Disposition::DUB) == Disposition::DUB;
        let is_hearing_impaired =
            (disposition & Disposition::HEARING_IMPAIRED) == Disposition::HEARING_IMPAIRED;
        let is_visual_impaired =
            (disposition & Disposition::VISUAL_IMPAIRED) == Disposition::VISUAL_IMPAIRED;
        let codec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())?;
        let audio = codec.decoder().audio()?;
        let mut profile = unsafe {
            let p = audio.as_ptr();
            if p.is_null() {
                anyhow::bail!("codec context is null");
            }
            (*p).profile
        };
        let bit_rate = audio.bit_rate();
        let channels = audio.channels();
        let sample_rate = audio.rate();

        let mut language = None;

        for (k, v) in &stream.metadata() {
            match k {
                "language" => {
                    language = Some(v.to_owned());
                    break;
                }
                _ => {}
            }
        }

        let codec = match (audio.id(), audio.profile()) {
            (codec::Id::AAC, codec::Profile::AAC(_)) => {
                // adjust profile to follow the standard
                // ffmpeg aac profiles have 0 based index
                profile += 1;
                AudioCodec::AAC
            }
            (codec::Id::AAC, codec::Profile::Unknown) => AudioCodec::AAC,
            (codec::Id::EAC3, _) => AudioCodec::EAC3,
            (codec::Id::AC3, _) => AudioCodec::AC3,
            (codec::Id::DTS, codec::Profile::DTS(_)) => AudioCodec::DTS,
            (codec::Id::DTS, codec::Profile::Unknown) => AudioCodec::DTS,
            (c, p) => {
                tracing::warn!("Unrecognized audio codec/profile: {:?}/{:?}", c, p);
                return Err(anyhow::anyhow!("Unregonized audio codec {:?}/{:?}", c, p));
            }
        };

        Ok(Self {
            codec,
            channels,
            profile_idc: profile,
            bit_rate,
            sample_rate,
            is_dub,
            is_hearing_impaired,
            is_visual_impaired,
            language,
        })
    }
}

#[derive(Debug)]
pub struct Data {}

impl TryFrom<ffmpeg_next::Stream<'_>> for Data {
    type Error = anyhow::Error;

    fn try_from(_value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub struct Subtitle {
    pub codec: SubtitlesCodec,
    pub language: Option<String>,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub is_visual_impaired: bool,
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Subtitle {
    type Error = anyhow::Error;

    fn try_from(stream: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        let disposition = stream.disposition();
        let is_forced = (disposition & Disposition::FORCED) == Disposition::FORCED;
        let is_hearing_impaired =
            (disposition & Disposition::HEARING_IMPAIRED) == Disposition::HEARING_IMPAIRED;
        let is_visual_impaired =
            (disposition & Disposition::VISUAL_IMPAIRED) == Disposition::VISUAL_IMPAIRED;
        let codec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())?;
        let subtitle = codec.decoder().subtitle()?;
        let mut language = None;

        for (k, v) in &stream.metadata() {
            match k {
                "language" => {
                    language = Some(v.to_owned());
                    break;
                }
                _ => {}
            }
        }

        let codec = match subtitle.id() {
            codec::Id::SRT | codec::Id::SUBRIP => SubtitlesCodec::SubRip,
            codec::Id::WEBVTT => SubtitlesCodec::WebVTT,
            codec::Id::DVD_SUBTITLE => SubtitlesCodec::DvdSubtitle,
            codec::Id::MOV_TEXT => SubtitlesCodec::MovText,
            codec::Id::ASS => SubtitlesCodec::ASS,
            rest => {
                tracing::warn!("Unrecognized subtitle codec: {:?}", rest);
                return Err(anyhow::anyhow!("Unregonized subtitle codec"));
            }
        };

        Ok(Self {
            codec,
            language,
            is_forced,
            is_hearing_impaired,
            is_visual_impaired,
        })
    }
}

#[derive(Debug)]
pub struct Attachment {}

impl TryFrom<ffmpeg_next::Stream<'_>> for Attachment {
    type Error = anyhow::Error;

    fn try_from(_value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub struct Track<T> {
    pub stream: T,
    pub index: usize,
    is_default: bool,
}

impl<T> Track<T> {
    pub fn is_default(&self) -> bool {
        self.is_default
    }
}

#[derive(Debug)]
pub enum StreamType {
    Video(Track<Video>),
    Audio(Track<Audio>),
    Data(Track<Data>),
    Subtitle(Track<Subtitle>),
    Attachment(Track<Attachment>),
}

#[derive(Debug)]
pub struct ProbeOutput {
    streams: Vec<StreamType>,
    chapters: Vec<Chapter>,
    duration: Duration,
    bitrate: u32,
    tag_title: Option<String>,
}

impl ProbeOutput {
    pub fn default_audio(&self) -> Option<&Audio> {
        let mut audio = None;

        for track in self.streams.iter().filter_map(|v| match v {
            StreamType::Audio(track) => Some(track),
            _ => None,
        }) {
            audio = Some(&track.stream);
            if track.is_default {
                return audio;
            }
        }

        audio
    }

    pub fn default_video(&self) -> Option<&Video> {
        let mut video = None;

        for track in self.streams.iter().filter_map(|v| match v {
            StreamType::Video(track) => Some(track),
            _ => None,
        }) {
            video = Some(&track.stream);
            if track.is_default {
                return video;
            }
        }

        video
    }

    pub fn video_streams(&self) -> impl Iterator<Item = &Track<Video>> {
        self.streams.iter().filter_map(|v| match v {
            StreamType::Video(track) => Some(track),
            _ => None,
        })
    }

    pub fn audio_streams(&self) -> impl Iterator<Item = &Track<Audio>> {
        self.streams.iter().filter_map(|v| match v {
            StreamType::Audio(track) => Some(track),
            _ => None,
        })
    }

    pub fn subtitle_streams(&self) -> impl Iterator<Item = &Track<Subtitle>> {
        self.streams.iter().filter_map(|v| match v {
            StreamType::Subtitle(track) => Some(track),
            _ => None,
        })
    }

    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    pub fn tag_title(&self) -> Option<&str> {
        self.tag_title.as_deref()
    }
}

impl TryFrom<ffmpeg_next::format::context::Input> for ProbeOutput {
    type Error = anyhow::Error;

    fn try_from(format: ffmpeg_next::format::context::Input) -> Result<Self, Self::Error> {
        let container_metadata = format.metadata();
        let mut tag_title = None;

        for (k, v) in &container_metadata {
            match k {
                "Title" | "title" | "TITLE" => {
                    tag_title = Some(v.to_owned());
                    break;
                }
                _ => {}
            }
        }

        let mut streams = Vec::new();
        let mut chapters = Vec::new();
        for chapter in format.chapters() {
            match chapter.try_into() {
                Ok(c) => chapters.push(c),
                Err(e) => {
                    tracing::warn!("Failed to parse ffmpeg chapter: {e}");
                }
            }
        }
        let duration = Duration::from_micros(
            format
                .duration()
                .try_into()
                .context("convert duration to u64")?,
        );
        let bitrate = format
            .bit_rate()
            .try_into()
            .context("convert bitrate to u32")?;
        for stream in format.streams() {
            let params = stream.parameters();
            match params.medium() {
                media::Type::Unknown => {
                    tracing::warn!("Encountered unknown media type");
                }
                media::Type::Video => match stream.try_into() {
                    Ok(v) => {
                        streams.push(StreamType::Video(v));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse video params: {e}");
                    }
                },
                media::Type::Audio => match stream.try_into() {
                    Ok(a) => {
                        streams.push(StreamType::Audio(a));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse audio params: {e}");
                    }
                },
                media::Type::Data => match stream.try_into() {
                    Ok(d) => streams.push(StreamType::Data(d)),
                    Err(e) => {
                        tracing::warn!("Failed to parse data params: {e}");
                    }
                },
                media::Type::Subtitle => match stream.try_into() {
                    Ok(d) => streams.push(StreamType::Subtitle(d)),
                    Err(e) => {
                        tracing::warn!("Failed to parse subtitle params: {e}");
                    }
                },
                media::Type::Attachment => match stream.try_into() {
                    Ok(a) => streams.push(StreamType::Attachment(a)),
                    Err(e) => {
                        tracing::warn!("Failed to parse attachment params: {e}");
                    }
                },
            }
        }

        Ok(Self {
            streams,
            chapters,
            duration,
            bitrate,
            tag_title,
        })
    }
}

pub async fn get_metadata(path: impl AsRef<Path>) -> anyhow::Result<ProbeOutput> {
    let path = path.as_ref().to_path_buf();
    tracing::trace!(
        "Getting metadata for a file: {}",
        Path::new(path.file_name().unwrap()).display()
    );
    tokio::task::spawn_blocking(move || {
        let format = ffmpeg_next::format::input(&path)?;
        ProbeOutput::try_from(format)
    })
    .await
    .expect("never panic")
}

/// h265 encoders
#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HevcEncoder {
    #[default]
    Libx265,
    Amf,
    Nvenc,
    Qsv,
    V4l2m2m,
    Vaapi,
    Vulkan,
}

impl HevcEncoder {
    pub fn gpu_accelerated(api: GpuEncodingApi) -> Option<Self> {
        match api {
            GpuEncodingApi::Nvenc => Some(Self::Nvenc),
            GpuEncodingApi::Amf => Some(Self::Amf),
            GpuEncodingApi::Vaapi => Some(Self::Vaapi),
            GpuEncodingApi::Vulkan => Some(Self::Vulkan),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Libx265 => "libx265",
            Self::Amf => "hevc_amf",
            Self::Nvenc => "hevc_nvenc",
            Self::Qsv => "hevc_qsv",
            Self::V4l2m2m => "hevc_v4l2m2m",
            Self::Vaapi => "hevc_vaapi",
            Self::Vulkan => "hevc_vulkan",
        }
    }
}

impl std::fmt::Display for HevcEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// av1 encoders
#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Av1Encoder {
    /// libaom AV1
    #[default]
    Libaom,
    /// librav1e AV1
    Librav1e,
    /// SVT-AV1(Scalable Video Technology for AV1) encoder
    Libsvtav1,
    /// NVIDIA NVENC av1 encoder
    Nvenc,
    /// AV1 (Intel Quick Sync Video acceleration)
    Qsv,
    /// AMD AMF AV1 encoder
    Amf,
    /// AV1 (VAAPI)
    Vaapi,
    /// Windows Media Audio 1
    Wmav1,
}

impl Av1Encoder {
    pub fn gpu_accelerated(api: GpuEncodingApi) -> Option<Self> {
        match api {
            GpuEncodingApi::Nvenc => Some(Self::Nvenc),
            GpuEncodingApi::Amf => Some(Self::Amf),
            GpuEncodingApi::Vaapi => Some(Self::Vaapi),
            GpuEncodingApi::Vulkan => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Av1Encoder::Libaom => "libaom-av1",
            Av1Encoder::Librav1e => "librav1e",
            Av1Encoder::Libsvtav1 => "libsvtav1",
            Av1Encoder::Nvenc => "av1_nvenc",
            Av1Encoder::Qsv => "av1_qsv",
            Av1Encoder::Amf => "av1_amf",
            Av1Encoder::Vaapi => "av1_vaapi",
            Av1Encoder::Wmav1 => "wmav1",
        }
    }
}

impl std::fmt::Display for Av1Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Advanced video coding encoders
#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum H264Encoder {
    #[default]
    Libx264,
    Amf,
    Nvenc,
    Qsv,
    V4l2m2m,
    Vaapi,
    Vulkan,
}

impl H264Encoder {
    pub fn gpu_accelerated(api: GpuEncodingApi) -> Option<Self> {
        match api {
            GpuEncodingApi::Nvenc => Some(Self::Nvenc),
            GpuEncodingApi::Amf => Some(Self::Amf),
            GpuEncodingApi::Vaapi => Some(Self::Vaapi),
            GpuEncodingApi::Vulkan => Some(Self::Vulkan),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::Amf => "h264_amf",
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::V4l2m2m => "h264_v4l2m2m",
            Self::Vaapi => "h264_vaapi",
            Self::Vulkan => "h264_vulkan",
        }
    }
}

impl std::fmt::Display for H264Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Hardware accelerated APIs
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GpuEncodingApi {
    Nvenc,
    Amf,
    Vaapi,
    Vulkan,
}

impl GpuEncodingApi {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nvenc => "nvenc",
            Self::Amf => "amf",
            Self::Vaapi => "vaapi",
            Self::Vulkan => "vulkan",
        }
    }
}

impl std::fmt::Display for GpuEncodingApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

static GPU_ACCEL_APIS: OnceCell<Box<[GpuEncodingApi]>> = OnceCell::const_new();

pub async fn get_or_init_gpu_accelated_apis() -> &'static [GpuEncodingApi] {
    GPU_ACCEL_APIS
        .get_or_init(async move || {
            let handle = tokio::task::spawn_blocking(gpu_accel_apis);
            handle.await.unwrap().into_boxed_slice()
        })
        .await
}

/// List all supported GPU accelerated APIs
fn gpu_accel_apis() -> Vec<GpuEncodingApi> {
    let mut encoders = Vec::new();
    let mut check = |api: GpuEncodingApi, encoder: &str| {
        match check_encoder_support(encoder) {
            Ok(_) => {
                tracing::info!("Supported GPU accelerated encoder {encoder}");
                encoders.push(api);
            }
            Err(e) => {
                tracing::trace!("{api} hw encoder is not supported: {e}")
            }
        };
    };
    check(GpuEncodingApi::Nvenc, "h264_nvenc");
    check(GpuEncodingApi::Amf, "h264_amf");
    check(GpuEncodingApi::Vaapi, "h264_vaapi");
    check(GpuEncodingApi::Vulkan, "h264_vulkan");

    encoders
}

fn check_encoder_support(encoder_name: &str) -> anyhow::Result<()> {
    let codec = ffmpeg_next::codec::encoder::find_by_name(encoder_name)
        .ok_or(anyhow::anyhow!("encoder {encoder_name} is not found"))?;
    let mut encoder = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;

    encoder.set_width(1280);
    encoder.set_height(720);
    encoder.set_format(ffmpeg_next::format::Pixel::YUV420P);
    encoder.set_time_base((1, 25));

    encoder.open()?;
    Ok(())
}

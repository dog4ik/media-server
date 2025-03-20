use std::{path::Path, time::Duration};

use anyhow::Context;
use ffmpeg_next::{codec, format::stream::Disposition, media};

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
    format_name: String,
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

    pub fn guess_mime(&self) -> &'static str {
        match self.format_name.as_str() {
            "matroska,webm" => "video/x-matroska",
            "mov,mp4,m4a,3gp,3g2,mj2" => "video/mp4",
            _ => "video/x-matroska",
        }
    }

    pub fn tag_title(&self) -> Option<&str> {
        self.tag_title.as_deref()
    }
}

impl TryFrom<ffmpeg_next::format::context::Input> for ProbeOutput {
    type Error = anyhow::Error;

    fn try_from(format: ffmpeg_next::format::context::Input) -> Result<Self, Self::Error> {
        let format_name = format.format().name().to_string();
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
            format_name,
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

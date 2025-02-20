use std::{path::Path, time::Duration};

use ffmpeg_next::{
    codec::traits::{Decoder, Encoder},
    media,
};

use crate::library::{AudioCodec, SubtitlesCodec, VideoCodec};

#[derive(Debug)]
pub struct Chapter {
    pub title: Option<String>,
}

impl TryFrom<ffmpeg_next::Chapter<'_>> for Chapter {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Chapter<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Video {
    pub codec: VideoCodec,
    // pub profile: &'a str,
    // pub display_aspect_ratio: &'a str,
    pub level: i32,
    pub avg_frame_rate: u32,
    pub width: u32,
    pub height: u32,
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Track<Video> {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Audio {
    pub codec: AudioCodec,
    pub channels: u8,
    // pub profile: Option<&'a str>,
    // pub sample_rate: &'a str,
    pub bit_rate: Option<u64>,
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Track<Audio> {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Data {}

impl TryFrom<ffmpeg_next::Stream<'_>> for Track<Data> {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Subtitle {
    pub codec: SubtitlesCodec,
    pub language: Option<String>,
}

impl TryFrom<ffmpeg_next::Stream<'_>> for Track<Subtitle> {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Attachment {}

impl TryFrom<ffmpeg_next::Stream<'_>> for Track<Attachment> {
    type Error = anyhow::Error;

    fn try_from(value: ffmpeg_next::Stream<'_>) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Track<T> {
    pub stream: T,
    pub is_default: bool,
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
    pub streams: Vec<StreamType>,
    pub chapters: Vec<Chapter>,
    pub duration: Duration,
    pub bit_rate: u64,
}

impl TryFrom<ffmpeg_next::format::context::Input> for ProbeOutput {
    type Error = anyhow::Error;

    fn try_from(format: ffmpeg_next::format::context::Input) -> Result<Self, Self::Error> {
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
                    Ok(d) => {
                        streams.push(StreamType::Data(d));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse data params: {e}");
                    }
                },
                media::Type::Subtitle => match stream.try_into() {
                    Ok(d) => {
                        streams.push(StreamType::Subtitle(d));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse subtitle params: {e}");
                    }
                },
                media::Type::Attachment => match stream.try_into() {
                    Ok(a) => {
                        streams.push(StreamType::Attachment(a));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse attachment params: {e}");
                    }
                },
            }
        }

        Ok(Self {
            streams,
            chapters,
            duration: todo!(),
            bit_rate: todo!(),
        })
    }
}

pub async fn get_metadata(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let format = ffmpeg_next::format::input(&path).unwrap();
    for stream in format.streams() {
        let md = stream.metadata();
        for (key, value) in &md {
            println!("{key}: {value}")
        }
        let params = stream.parameters();
        let id = params.id();
        match params.medium() {
            media::Type::Unknown => {}
            media::Type::Video => {
                let decoder = id.decoder().unwrap();
                match params.id() {
                    ffmpeg_next::codec::Id::H264 => {}
                    _ => {}
                }
                let video = decoder.video().unwrap();
                println!("decoder video name: {}", video.name());
            }
            media::Type::Audio => {}
            media::Type::Data => {}
            media::Type::Subtitle => {}
            media::Type::Attachment => {}
        }
        let encoder = id.encoder().unwrap();
        if let Ok(video) = encoder.video() {
            dbg!(video.name());
        }
        dbg!(params.id());
        dbg!(params.medium());
        println!("stream disposition: {:?}", stream.disposition());
        println!("stream id {}", stream.id());
        dbg!(&stream);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::get_metadata;

    #[tokio::test]
    pub async fn application_binary_interface() {
        ffmpeg_next::init().unwrap();
        let start = Instant::now();
        get_metadata("/mnt/win/Users/Stas/Videos/show/dexter.original.sin.s01e01.1080p.web.h264-successfulcrab[EZTVx.to].mkv").await.unwrap();
        println!("took: {:?}", start.elapsed());
    }
}

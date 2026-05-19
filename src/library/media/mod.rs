pub mod codec;
pub mod container;

use self::container::VideoContainer;
use crate::{
    app_state::AppError,
    db::{DbActions, DbVideo},
    ffmpeg_abi::{ProbeOutput, get_metadata},
};
use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};
use axum_extra::{TypedHeader, headers::Range};
use serde::{Deserialize, Serialize, de::Visitor, ser::SerializeStruct};
use std::{
    fmt::Display,
    io::SeekFrom,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::OnceCell,
};
use tokio_util::codec::{BytesCodec, FramedRead};

#[derive(Debug, Clone)]
pub struct Video {
    path: PathBuf,
    metadata: LazyFFprobeOutput,
}

/// Lazily evaluated ffprobe metadata
#[derive(Debug, Clone)]
struct LazyFFprobeOutput {
    metadata: Arc<OnceCell<ProbeOutput>>,
}

impl LazyFFprobeOutput {
    fn new() -> Self {
        Self {
            metadata: Arc::new(OnceCell::new()),
        }
    }

    async fn get_or_init(&self, path: impl AsRef<Path>) -> anyhow::Result<&ProbeOutput> {
        self.metadata
            .get_or_try_init(|| async { get_metadata(path).await })
            .await
    }

    #[allow(unused)]
    fn try_get(&self) -> Option<&ProbeOutput> {
        self.metadata.get()
    }
}

impl Video {
    /// Returns struct compatible with database Video table
    pub async fn into_db_video(&self) -> std::io::Result<DbVideo> {
        let now = time::OffsetDateTime::now_utc();

        Ok(DbVideo {
            id: None,
            path: self.path.to_string_lossy().to_string(),
            size: self.file_size().await? as i64,
            metadata_id: None,
            is_prime: false,
            scan_date: now.to_string(),
        })
    }

    pub async fn fetch_duration(&self) -> anyhow::Result<std::time::Duration> {
        let metadata = self.metadata().await?;
        Ok(metadata.duration())
    }

    pub async fn get_or_insert_id(&self, tx: &mut crate::db::DbTransaction) -> anyhow::Result<i64> {
        let path = self.path().to_string_lossy();
        let res = sqlx::query!("SELECT id FROM videos WHERE path = ?", path)
            .fetch_one(&mut **tx)
            .await;
        let video_id: Result<i64, anyhow::Error> = match res {
            Ok(r) => Ok(r.id),
            Err(sqlx::Error::RowNotFound) => {
                let db_video = self.into_db_video().await?;
                let id = tx.insert_video(db_video).await?;
                Ok(id)
            }
            Err(e) => Err(e.into()),
        };
        video_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create self from path, checks only file existence
    pub async fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if tokio::fs::try_exists(&path).await? {
            Ok(Self {
                path: path.as_ref().to_path_buf(),
                metadata: LazyFFprobeOutput::new(),
            })
        } else {
            Err(anyhow::anyhow!(
                "Video {} does not exist",
                path.as_ref().display()
            ))
        }
    }

    /// Creates video from path and evaluates ffprobe metadata
    /// Errors if video file is corrupted or missing
    pub async fn from_path_with_metadata(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let metadata = LazyFFprobeOutput::new();
        metadata.get_or_init(&path).await?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            metadata,
        })
    }

    /// Do not check file existence
    pub fn from_path_unchecked(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            metadata: LazyFFprobeOutput::new(),
        }
    }

    pub async fn metadata(&self) -> anyhow::Result<&ProbeOutput> {
        self.metadata.get_or_init(self.path()).await
    }

    /// Get file size in bytes
    pub async fn file_size(&self) -> std::io::Result<u64> {
        tokio::fs::metadata(&self.path).await.map(|m| m.len())
    }

    /// Delete self
    pub async fn delete(&self) -> Result<(), std::io::Error> {
        tracing::debug!("Removing video file {}", self.path.display());
        tokio::fs::remove_file(&self.path).await
    }

    /// Video file container based on the file extension
    pub fn container(&self) -> VideoContainer {
        let ext = self.path().extension().expect("all videos have extension");
        VideoContainer::try_from(ext).expect("all videos have known container")
    }

    pub async fn serve(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse + use<> {
        let file_size = match self.file_size().await {
            Ok(size) => size,
            Err(e) => return AppError::from(e).into_response(),
        };
        let range = range.map(|h| h.0).unwrap_or(Range::bytes(0..).unwrap());
        let (start, end) = range
            .satisfiable_ranges(file_size)
            .next()
            .expect("at least one tuple");
        let start = match start {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => 0,
        };

        let end = match end {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => file_size,
        };

        let Ok(mut file) = tokio::fs::File::open(&self.path).await else {
            return AppError::internal_error("Failed to open file").into_response();
        };
        if file.seek(SeekFrom::Start(start)).await.is_err() {
            return AppError::bad_request("Failed to seek file to requested range").into_response();
        };

        let chunk_size = end - start + 1;
        let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(end - start),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(self.container().mime_type()),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0"),
        );
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size)).unwrap(),
        );

        (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream_of_bytes),
        )
            .into_response()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution(pub (usize, usize));

impl utoipa::ToSchema for Resolution {
    fn name() -> std::borrow::Cow<'static, str> {
        "Resolution".into()
    }
}
impl utoipa::PartialSchema for Resolution {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::Type;
        use utoipa::openapi::schema::SchemaType;
        utoipa::openapi::ObjectBuilder::new()
            .property(
                "width",
                utoipa::openapi::ObjectBuilder::new().schema_type(SchemaType::Type(Type::Integer)),
            )
            .required("width")
            .property(
                "height",
                utoipa::openapi::ObjectBuilder::new().schema_type(SchemaType::Type(Type::Integer)),
            )
            .required("height")
            .into()
    }
}

impl Resolution {
    pub fn new(width: usize, height: usize) -> Self {
        Self((width, height))
    }

    pub fn width(&self) -> usize {
        self.0.0
    }

    pub fn height(&self) -> usize {
        self.0.1
    }
}

impl Serialize for Resolution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (x, y) = self.0;
        let mut resolution = serializer.serialize_struct("Resolution", 2)?;
        resolution.serialize_field("width", &x)?;
        resolution.serialize_field("height", &y)?;
        resolution.end()
    }
}

impl<'de> Deserialize<'de> for Resolution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ResolutionVisitor;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Height,
            Width,
        }

        impl<'de> Visitor<'de> for ResolutionVisitor {
            type Value = Resolution;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "String like 1920x1080 or tuple of integers or { height, width } object",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Resolution::from_str(v).expect("any str to be valid"))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let width = seq
                    .next_element()?
                    .ok_or(serde::de::Error::missing_field("width"))?;
                let height = seq
                    .next_element()?
                    .ok_or(serde::de::Error::missing_field("height"))?;
                Ok(Resolution::from((width, height)))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut width = None;
                let mut height = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Width => {
                            if width.is_some() {
                                return Err(serde::de::Error::duplicate_field("width"));
                            }
                            width = Some(map.next_value()?);
                        }
                        Field::Height => {
                            if height.is_some() {
                                return Err(serde::de::Error::duplicate_field("height"));
                            }
                            height = Some(map.next_value()?);
                        }
                    }
                }
                let width = width.ok_or_else(|| serde::de::Error::missing_field("width"))?;
                let height = height.ok_or_else(|| serde::de::Error::missing_field("height"))?;
                Ok(Resolution::new(width, height))
            }
        }
        deserializer.deserialize_any(ResolutionVisitor)
    }
}

impl Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (x, y) = self.0;
        write!(f, "{}x{}", x, y)
    }
}

impl From<(usize, usize)> for Resolution {
    fn from(value: (usize, usize)) -> Self {
        Self((value.0, value.1))
    }
}

impl From<Resolution> for (usize, usize) {
    fn from(val: Resolution) -> Self {
        val.0
    }
}

impl FromStr for Resolution {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (x, y) = s
            .split_once('x')
            .ok_or(anyhow::anyhow!("str must be separated with 'x'"))?;
        let x = x.parse()?;
        let y = y.parse()?;
        Ok((x, y).into())
    }
}

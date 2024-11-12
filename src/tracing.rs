use std::convert::Infallible;
use std::fmt::{self};
use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::path::Path;

use axum::response::sse::{Event, KeepAlive};
use axum::response::Sse;
use axum::Extension;
use serde_json::{Map, Number, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::{Stream, StreamExt};
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

use crate::config::AppResources;

#[derive(Debug)]
pub struct FileLoggingLayer {
    pub sender: mpsc::Sender<JsonTracingEvent>,
}

#[derive(Debug)]
struct PublicTracerLayer {
    channel: broadcast::Sender<JsonTracingEvent>,
}

#[derive(Debug, Clone)]
pub struct LogChannel(pub broadcast::Sender<JsonTracingEvent>);

impl FileLoggingLayer {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new().append(true).create(true).open(&path)?;
        let mut writer = LineWriter::new(file);
        let (tx, mut rx) = mpsc::channel(1000);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                serde_json::to_writer(&mut writer, &event).unwrap();
                writer.write_all(b"\n").unwrap();
            }
        });
        Ok(Self { sender: tx })
    }
}

impl LogChannel {
    pub async fn into_sse_stream(
        Extension(channel): Extension<LogChannel>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let receiver = channel.0.subscribe();
        let stream = tokio_stream::wrappers::BroadcastStream::new(receiver).map(|item| {
            if let Ok(item) = item {
                Ok(Event::default().json_data(item).unwrap())
            } else {
                Ok(Event::default())
            }
        });

        Sse::new(stream).keep_alive(KeepAlive::default())
    }
}

impl PublicTracerLayer {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { channel: tx }
    }
}

struct JsonVisitor {
    value: Map<String, Value>,
}

impl JsonVisitor {
    fn new() -> Self {
        let str = Map::new();
        Self { value: str }
    }
}

impl Visit for JsonVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.value.insert(
            field.name().to_string(),
            Value::String(format!("{:?}", value)),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.value
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.value
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.value
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.value
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(num) = Number::from_f64(value) {
            self.value
                .insert(field.name().to_string(), Value::Number(num));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.value
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
}

#[derive(Debug, serde::Serialize, Clone, utoipa::ToSchema)]
pub struct JsonTracingEvent {
    timestamp: String,
    target: &'static str,
    level: String,
    name: &'static str,
    fields: Map<String, Value>,
}

impl JsonTracingEvent {
    pub fn from_event(event: &tracing::Event) -> Self {
        let metadata = event.metadata();
        let mut visitor = JsonVisitor::new();
        let now = time::OffsetDateTime::now_utc().to_string();
        let level = metadata.level().to_string();
        event.record(&mut visitor);
        Self {
            timestamp: now,
            target: metadata.target(),
            level,
            name: metadata.name(),
            fields: visitor.value,
        }
    }
}

impl<S: Subscriber> Layer<S> for FileLoggingLayer {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let _ = (metadata, ctx);
        true
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let json = JsonTracingEvent::from_event(event);
        let _ = self.sender.try_send(json);
    }
}

impl<S: Subscriber> Layer<S> for PublicTracerLayer {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let target = metadata.target();
        let exclude_patterns = ["hyper", "mio", "notify", "sqlx", "reqwest", "tokio_util"];
        !exclude_patterns
            .iter()
            .any(|pattern| target.starts_with(pattern))
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if self.channel.receiver_count() > 0 {
            let json = JsonTracingEvent::from_event(event);
            let _ = self.channel.send(json);
        }
    }
}

pub fn init_tracer() -> LogChannel {
    let log_path = AppResources::log();

    let pub_tracer = PublicTracerLayer::new();
    let file_logger = FileLoggingLayer::from_path(log_path).unwrap();
    let log_channel = LogChannel(pub_tracer.channel.clone());
    let sub = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();
    sub.with(pub_tracer).with(file_logger).init();
    log_channel
}

use std::convert::Infallible;
use std::fmt::{self};

use axum::response::sse::{Event, KeepAlive};
use axum::response::Sse;
use axum::Extension;
use serde_json::{Map, Value};
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};
use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

#[derive(Debug)]
struct PublicTracerLayer {
    channel: broadcast::Sender<String>,
}

#[derive(Debug, Clone)]
pub struct LogChannel(pub broadcast::Sender<String>);

impl LogChannel {
    pub async fn into_sse_stream(
        Extension(channel): Extension<LogChannel>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let receiver = channel.0.subscribe();
        let stream = tokio_stream::wrappers::BroadcastStream::new(receiver).map(|item| {
            if let Ok(item) = item {
                Ok(Event::default().data(item))
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
}

impl<S: Subscriber> Layer<S> for PublicTracerLayer {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let target = metadata.target();
        let patterns = ["hyper", "mio", "notify"];
        !patterns.iter().any(|pattern| target.starts_with(pattern))
    }
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let mut visitor = JsonVisitor::new();
        let now = time::OffsetDateTime::now_utc().to_string();
        let level = metadata.level().to_string();
        event.record(&mut visitor);
        let json = serde_json::json!({
        "timestamp": now,
        "target": metadata.target(),
        "level": level,
        "name": metadata.name(),
        "fields": visitor.value
        });
        let _ = self.channel.send(serde_json::to_string(&json).unwrap());
    }
}

pub fn init_tracer(max_level: Level) -> LogChannel {
    let sub = tracing_subscriber::fmt()
        .pretty()
        .with_max_level(max_level)
        .finish();
    let pub_tracer = PublicTracerLayer::new();
    let log_channel = LogChannel(pub_tracer.channel.clone());
    sub.with(pub_tracer).init();
    return log_channel;
}

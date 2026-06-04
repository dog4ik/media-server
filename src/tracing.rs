use std::fmt::{self};
use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::path::Path;

use opentelemetry::{KeyValue, global, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::{
    Resource,
    metrics::{MeterProviderBuilder, PeriodicReader, SdkMeterProvider},
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_VERSION},
};
use serde_json::{Map, Number, Value};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::AppResources;

/// File logging layer: serializes every event to JSON-lines in the log file.
#[derive(Debug)]
pub struct FileLoggingLayer {
    pub sender: mpsc::Sender<JsonTracingEvent>,
}

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

/// Resource that captures information about the entity for which telemetry is recorded.
fn resource() -> Resource {
    Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_schema_url(
            [
                KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
                KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, "develop"),
            ],
            SCHEMA_URL,
        )
        .build()
}

/// Construct MeterProvider for the metrics layer, exporting to `endpoint`.
fn init_meter_provider(endpoint: &str) -> SdkMeterProvider {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_temporality(opentelemetry_sdk::metrics::Temporality::default())
        .build()
        .unwrap();

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(30))
        .build();

    let meter_provider = MeterProviderBuilder::default()
        .with_resource(resource())
        .with_reader(reader)
        .build();

    global::set_meter_provider(meter_provider.clone());

    meter_provider
}

/// Construct TracerProvider for the OpenTelemetry layer, exporting to `endpoint`.
fn init_tracer_provider(endpoint: &str) -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .unwrap();

    SdkTracerProvider::builder()
        // Customize sampling strategy
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            1.0,
        ))))
        // If export trace to AWS X-Ray, you can use XrayIdGenerator
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource())
        .with_batch_exporter(exporter)
        .build()
}

struct Providers {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
}

/// Held for the lifetime of the process; flushes and shuts down the OTel
/// providers on drop so buffered spans/metrics are exported before exit.
pub struct OtelGuard {
    providers: Option<Providers>,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        let Some(providers) = &self.providers else {
            return;
        };
        if let Err(err) = providers.tracer_provider.shutdown() {
            eprintln!("{err:?}");
        }
        if let Err(err) = providers.meter_provider.shutdown() {
            eprintln!("{err:?}");
        }
    }
}

/// Install the global tracing subscriber
/// OpenTelemetry export is added only when `otel_endpoint` is set
pub fn init_tracer(otel_endpoint: Option<&str>) -> OtelGuard {
    let file_logger = FileLoggingLayer::from_path(AppResources::log()).unwrap();

    let env_filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env_lossy();

    let providers = otel_endpoint.map(|endpoint| Providers {
        tracer_provider: init_tracer_provider(endpoint),
        meter_provider: init_meter_provider(endpoint),
    });

    let otel_layer = providers.as_ref().map(|p| {
        let tracer = p.tracer_provider.tracer("tracing-otel-subscriber");
        OpenTelemetryLayer::new(tracer)
    });
    let metrics_layer = providers
        .as_ref()
        .map(|p| MetricsLayer::new(p.meter_provider.clone()));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .with(file_logger)
        .with(metrics_layer)
        .with(otel_layer)
        .init();

    OtelGuard { providers }
}

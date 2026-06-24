use std::{
    fmt::Display,
    future::Future,
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{
    extract::{MatchedPath, Request},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram},
    trace::TracerProvider as _,
};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_prometheus::PrometheusExporter;
use opentelemetry_sdk::{
    Resource,
    metrics::{
        Aggregation, Instrument as MetricInstrument, InstrumentKind, SdkMeterProvider, Stream,
    },
    trace::SdkTracerProvider,
};
use prometheus::{Encoder, Registry, TextEncoder};
use tracing::Instrument;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_FILTER: &str = "debug,hyper=off,rustls=error,tungstenite=error";
const LATENCY_HISTOGRAM_BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

static INSTRUMENTS: LazyLock<Instruments> = LazyLock::new(Instruments::new);
static PROMETHEUS_REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

struct Instruments {
    http_requests: Counter<u64>,
    http_request_duration: Histogram<f64>,
    db_queries: Counter<u64>,
    db_query_duration: Histogram<f64>,
    actor_starts: Counter<u64>,
    actor_start_duration: Histogram<f64>,
    actor_messages: Counter<u64>,
    actor_message_duration: Histogram<f64>,
}

impl Instruments {
    fn new() -> Self {
        let meter = global::meter(env!("CARGO_PKG_NAME"));

        Self {
            http_requests: meter
                .u64_counter("http.server.request.count")
                .with_description("HTTP server requests.")
                .build(),
            http_request_duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("HTTP server request duration.")
                .build(),
            db_queries: meter
                .u64_counter("db.client.operation.count")
                .with_description("MongoDB client operations.")
                .build(),
            db_query_duration: meter
                .f64_histogram("db.client.operation.duration")
                .with_unit("s")
                .with_description("MongoDB client operation duration.")
                .build(),
            actor_starts: meter
                .u64_counter("actor.startup.count")
                .with_description("Actor startups.")
                .build(),
            actor_start_duration: meter
                .f64_histogram("actor.startup.duration")
                .with_unit("s")
                .with_description("Actor startup duration.")
                .build(),
            actor_messages: meter
                .u64_counter("actor.message.count")
                .with_description("Actor messages processed.")
                .build(),
            actor_message_duration: meter
                .f64_histogram("actor.message.duration")
                .with_unit("s")
                .with_description("Actor message processing duration.")
                .build(),
        }
    }
}

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take()
            && let Err(error) = provider.shutdown()
        {
            eprintln!("error shutting down OpenTelemetry tracer provider: {error}");
        }

        if let Some(provider) = self.meter_provider.take()
            && let Err(error) = provider.shutdown()
        {
            eprintln!("error shutting down OpenTelemetry meter provider: {error}");
        }
    }
}

pub fn init() -> TelemetryGuard {
    let resource = Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();

    let tracer_provider = otlp_configured()
        .then(|| init_tracer_provider(resource.clone()))
        .flatten();
    let meter_provider = init_meter_provider(resource);

    let otel_layer = tracer_provider.as_ref().map(|provider| {
        let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
        tracing_opentelemetry::layer().with_tracer(tracer)
    });

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from(
            std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_string()),
        ))
        .with(tracing_subscriber::fmt::layer().with_line_number(true))
        .with(otel_layer)
        .init();

    TelemetryGuard {
        tracer_provider,
        meter_provider,
    }
}

fn otlp_configured() -> bool {
    [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    ]
    .into_iter()
    .any(|key| std::env::var_os(key).is_some())
}

fn init_tracer_provider(resource: Resource) -> Option<SdkTracerProvider> {
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => exporter,
        Err(error) => {
            eprintln!("failed to build OpenTelemetry span exporter: {error}");
            return None;
        }
    };

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();
    global::set_tracer_provider(provider.clone());

    Some(provider)
}

fn init_meter_provider(resource: Resource) -> Option<SdkMeterProvider> {
    let prometheus_exporter = init_prometheus_exporter()?;

    let mut provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_view(latency_histogram_view)
        .with_reader(prometheus_exporter);

    if let Some(exporter) = init_otlp_metric_exporter() {
        provider = provider.with_periodic_exporter(exporter);
    }

    let provider = provider.build();
    global::set_meter_provider(provider.clone());

    Some(provider)
}

fn latency_histogram_view(instrument: &MetricInstrument) -> Option<Stream> {
    if instrument.kind() != InstrumentKind::Histogram {
        return None;
    }

    match instrument.name() {
        "http.server.request.duration"
        | "db.client.operation.duration"
        | "actor.startup.duration"
        | "actor.message.duration" => Stream::builder()
            .with_aggregation(Aggregation::ExplicitBucketHistogram {
                boundaries: LATENCY_HISTOGRAM_BUCKETS.to_vec(),
                record_min_max: true,
            })
            .build()
            .ok(),
        _ => None,
    }
}

fn init_prometheus_exporter() -> Option<PrometheusExporter> {
    match opentelemetry_prometheus::exporter()
        .with_registry(PROMETHEUS_REGISTRY.clone())
        .build()
    {
        Ok(exporter) => Some(exporter),
        Err(error) => {
            eprintln!("failed to build Prometheus exporter: {error}");
            None
        }
    }
}

fn init_otlp_metric_exporter() -> Option<opentelemetry_otlp::MetricExporter> {
    if !otlp_configured() {
        return None;
    }

    match opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => Some(exporter),
        Err(error) => {
            eprintln!("failed to build OpenTelemetry metric exporter: {error}");
            None
        }
    }
}

pub async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = PROMETHEUS_REGISTRY.gather();
    let mut body = Vec::new();

    if let Err(error) = encoder.encode(&metric_families, &mut body) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode prometheus metrics: {error}"),
        )
            .into_response();
    }

    (
        [(
            axum::http::header::CONTENT_TYPE,
            encoder.format_type().to_string(),
        )],
        body,
    )
        .into_response()
}

pub async fn http_middleware(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = http_route(&req);
    let path = req.uri().path().to_string();
    let span = tracing::info_span!(
        "http.request",
        "otel.kind" = "server",
        "http.request.method" = %method,
        "http.route" = %route,
        "url.path" = %path,
        "http.response.status_code" = tracing::field::Empty,
    );
    let started = Instant::now();
    let response = next.run(req).instrument(span.clone()).await;
    let status = response.status();

    span.record("http.response.status_code", status.as_u16() as i64);
    record_http_request(&method, &route, status, started.elapsed());

    response
}

fn http_route(req: &Request) -> String {
    req.extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string())
}

pub(crate) fn record_http_endpoint(
    method: &'static str,
    route: &'static str,
    status: StatusCode,
    duration: Duration,
) {
    let attrs = [
        KeyValue::new("http.request.method", method),
        KeyValue::new("http.route", route),
        KeyValue::new("http.response.status_code", status.as_u16() as i64),
    ];

    INSTRUMENTS.http_requests.add(1, &attrs);
    INSTRUMENTS
        .http_request_duration
        .record(duration.as_secs_f64(), &attrs);
}

fn record_http_request(method: &Method, route: &str, status: StatusCode, duration: Duration) {
    let attrs = [
        KeyValue::new("http.request.method", method.as_str().to_string()),
        KeyValue::new("http.route", route.to_string()),
        KeyValue::new("http.response.status_code", status.as_u16() as i64),
    ];

    INSTRUMENTS.http_requests.add(1, &attrs);
    INSTRUMENTS
        .http_request_duration
        .record(duration.as_secs_f64(), &attrs);
}

pub(crate) async fn db_query<T, E, F>(
    collection: &'static str,
    operation: &'static str,
    future: F,
) -> Result<T, E>
where
    E: Display,
    F: Future<Output = Result<T, E>>,
{
    let span = tracing::info_span!(
        "db.query",
        "db.system.name" = "mongodb",
        "db.collection.name" = collection,
        "db.operation.name" = operation,
        error = tracing::field::Empty,
    );
    let started = Instant::now();
    let result = future.instrument(span.clone()).await;
    let success = result.is_ok();

    span.record("error", !success);

    if let Err(error) = &result {
        let _entered = span.enter();
        tracing::warn!(error = %error, "database query failed");
    }

    let attrs = [
        KeyValue::new("db.system.name", "mongodb"),
        KeyValue::new("db.collection.name", collection),
        KeyValue::new("db.operation.name", operation),
        KeyValue::new("success", success),
    ];

    INSTRUMENTS.db_queries.add(1, &attrs);
    INSTRUMENTS
        .db_query_duration
        .record(started.elapsed().as_secs_f64(), &attrs);

    result
}

pub(crate) fn record_actor_start(startup_kind: &'static str, duration: Duration, success: bool) {
    let attrs = [
        KeyValue::new("actor.type", "match"),
        KeyValue::new("actor.startup.kind", startup_kind),
        KeyValue::new("success", success),
    ];

    INSTRUMENTS.actor_starts.add(1, &attrs);
    INSTRUMENTS
        .actor_start_duration
        .record(duration.as_secs_f64(), &attrs);
}

pub(crate) fn record_actor_message(message_kind: &'static str, duration: Duration) {
    let attrs = [
        KeyValue::new("actor.type", "match"),
        KeyValue::new("actor.message", message_kind),
    ];

    INSTRUMENTS.actor_messages.add(1, &attrs);
    INSTRUMENTS
        .actor_message_duration
        .record(duration.as_secs_f64(), &attrs);
}

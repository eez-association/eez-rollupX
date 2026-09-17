//! Optional finite EEZ spans and bounded OTLP/HTTP export. The process owns the
//! subscriber; this module supplies one additional, independently filtered layer.
use std::{
    net::SocketAddr,
    sync::{Mutex, mpsc},
    time::Duration,
};

use opentelemetry::{
    Context, KeyValue,
    trace::{TraceContextExt, TracerProvider},
};
use opentelemetry_proto::{
    tonic::collector::trace::v1::{ExportTraceServiceRequest, ExportTraceServiceResponse},
    transform::{
        common::tonic::ResourceAttributesWithSchema,
        trace::tonic::group_spans_by_resource_and_scope,
    },
};
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    trace::{Sampler, SdkTracerProvider, SpanData, SpanProcessor},
};
use prost::Message;
use serde::{Deserialize, Serialize};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{Layer, Registry, filter::filter_fn};

use crate::schema::Identity;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Private OTLP/HTTP receiver; the path is always `/v1/traces`.
    pub receiver: SocketAddr,
    #[serde(default)]
    pub allow_non_loopback: bool,
    /// Explicit pilot choice, never inherited from OTEL_* environment variables.
    pub sample_ratio: f64,
    #[serde(default = "default_queue")]
    pub queue_capacity: usize,
    #[serde(default = "default_timeout")]
    pub export_timeout_ms: u64,
}
fn default_queue() -> usize {
    2048
}
fn default_timeout() -> u64 {
    1000
}
impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.receiver.port() == 0
            || (!self.receiver.ip().is_loopback() && !self.allow_non_loopback)
        {
            return Err(
                "tracing receiver must have a port and non-loopback requires explicit opt-in",
            );
        }
        if !self.sample_ratio.is_finite() || !(0.0..=1.0).contains(&self.sample_ratio) {
            return Err("trace sample_ratio must be between zero and one");
        }
        if !(1..=16384).contains(&self.queue_capacity)
            || !(1..=2000).contains(&self.export_timeout_ms)
        {
            return Err("trace queue or export timeout outside supported bounds");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct CountedSampler(Sampler);
impl opentelemetry_sdk::trace::ShouldSample for CountedSampler {
    fn should_sample(
        &self,
        parent: Option<&Context>,
        trace_id: opentelemetry::trace::TraceId,
        name: &str,
        kind: &opentelemetry::trace::SpanKind,
        attributes: &[KeyValue],
        links: &[opentelemetry::trace::Link],
    ) -> opentelemetry::trace::SamplingResult {
        let result = self
            .0
            .should_sample(parent, trace_id, name, kind, attributes, links);
        let decision = match result.decision {
            opentelemetry::trace::SamplingDecision::RecordAndSample => "sampled",
            _ => "sampled_out",
        };
        metrics::counter!("eez_trace_sampling_decisions_total", "decision" => decision)
            .increment(1);
        result
    }
}

pub type TraceLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Owns the provider, without installing a competing subscriber or global tracer.
#[derive(Debug, Default)]
pub struct Guard(Option<SdkTracerProvider>);
impl Guard {
    pub fn start(
        identity: &Identity,
        config: Option<&Config>,
    ) -> Result<(Self, Option<TraceLayer>), &'static str> {
        let Some(config) = config else {
            return Ok((Self::default(), None));
        };
        identity.validate()?;
        config.validate()?;
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", identity.service.clone()),
                KeyValue::new("service.instance.id", identity.instance.clone()),
                KeyValue::new("deployment.environment.name", identity.environment.clone()),
                KeyValue::new("eez.deployment", identity.deployment.clone()),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            ])
            .build();
        let processor = Processor::start(config, &resource)?;
        let provider = SdkTracerProvider::builder()
            .with_sampler(CountedSampler(Sampler::ParentBased(Box::new(
                Sampler::TraceIdRatioBased(config.sample_ratio),
            ))))
            .with_max_attributes_per_span(32)
            .with_max_events_per_span(0)
            .with_max_links_per_span(128)
            .with_max_attributes_per_link(4)
            .with_resource(resource)
            .with_span_processor(processor)
            .build();
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("eez"))
            .with_location(false)
            .with_threads(false)
            // Only explicitly reviewed spans cross the export boundary. Existing
            // log events (including errors with arbitrary text) stay in logging.
            .with_filter(filter_fn(|metadata| {
                metadata.is_span() && metadata.target() == "eez_trace"
            }))
            .boxed();
        // Register zeros before any work so rate alerts can observe the first
        // failure instead of waiting for a previously absent counter series.
        for reason in [
            "queue_full",
            "contention",
            "shutdown",
            "export_failure",
            "collector_rejected",
            "invalid_context",
            "link_limit",
        ] {
            metrics::counter!("eez_telemetry_dropped_events_total", "signal" => "trace", "reason" => reason).increment(0);
        }
        for decision in ["sampled", "sampled_out"] {
            metrics::counter!("eez_trace_sampling_decisions_total", "decision" => decision)
                .increment(0);
        }
        metrics::counter!("eez_trace_exported_spans_total").increment(0);
        metrics::gauge!("eez_trace_sample_ratio").set(config.sample_ratio);
        metrics::gauge!("eez_trace_export_enabled").set(1.);
        Ok((Self(Some(provider)), Some(layer)))
    }

    /// Call after protocol draining with the actual remaining application budget.
    /// A zero budget signals stop without extending an unknown/exhausted deadline.
    pub fn shutdown(self, remaining: Duration) -> bool {
        self.shutdown_with_budget(flush_budget(remaining))
    }

    /// Call after protocol draining only when the application explicitly has no
    /// overall shutdown deadline. Adds at most two seconds for telemetry; this
    /// must not substitute for an unknown or exhausted finite application budget.
    pub fn shutdown_without_application_deadline(self) -> bool {
        self.shutdown_with_budget(MAX_FLUSH_BUDGET)
    }

    fn shutdown_with_budget(mut self, budget: Duration) -> bool {
        self.0
            .take()
            .is_none_or(|provider| provider.shutdown_with_timeout(budget).is_ok())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.0.take() {
            let _ = provider.shutdown_with_timeout(Duration::ZERO);
        }
    }
}
const MAX_FLUSH_BUDGET: Duration = Duration::from_secs(2);

pub fn flush_budget(remaining: Duration) -> Duration {
    MAX_FLUSH_BUDGET.min(remaining / 4)
}

/// The SDK processor hides queue drops until shutdown. This small transport
/// owns a nonblocking bounded channel so losses are observable while running.
/// All encoding/network work belongs to its single dedicated thread. No runtime
/// handle, environment-derived endpoint/headers/proxy, or unbounded retry exists.
#[derive(Debug)]
struct Processor {
    sender: Mutex<Option<mpsc::SyncSender<SpanData>>>,
    done: Mutex<mpsc::Receiver<()>>,
}
impl Processor {
    fn start(config: &Config, resource: &Resource) -> Result<Self, &'static str> {
        let (sender, receiver) = mpsc::sync_channel(config.queue_capacity);
        let (finished, done) = mpsc::sync_channel(1);
        let config = config.clone();
        let resource: ResourceAttributesWithSchema = resource.into();
        std::thread::Builder::new()
            .name("eez-trace-export".into())
            .spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_millis(config.export_timeout_ms))
                    .build();
                let endpoint = format!("http://{}/v1/traces", config.receiver);
                while let Ok(first) = receiver.recv() {
                    let mut batch = vec![first];
                    batch.extend(receiver.try_iter().take(127));
                    let count = batch.len() as u64;
                    let request = ExportTraceServiceRequest {
                        resource_spans: group_spans_by_resource_and_scope(batch, &resource),
                    };
                    let outcome = client.as_ref().ok().and_then(|client| {
                        use std::io::Read;
                        let response = client
                            .post(&endpoint)
                            .header("content-type", "application/x-protobuf")
                            .body(request.encode_to_vec())
                            .send()
                            .ok()?;
                        if !response.status().is_success() {
                            return None;
                        }
                        let mut body = Vec::new();
                        response.take(65537).read_to_end(&mut body).ok()?;
                        if body.len() > 65536 {
                            return None;
                        }
                        let response = ExportTraceServiceResponse::decode(body.as_slice()).ok()?;
                        Some(
                            response
                                .partial_success
                                .map_or(0, |partial| partial.rejected_spans.max(0).cast_unsigned())
                                .min(count),
                        )
                    });
                    match outcome {
                        Some(rejected) => {
                            metrics::counter!("eez_trace_exported_spans_total")
                                .increment(count - rejected);
                            dropped("collector_rejected", rejected);
                        }
                        None => dropped("export_failure", count),
                    }
                }
                let _ = finished.send(());
            })
            .map_err(|_| "cannot start trace export worker")?;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            done: Mutex::new(done),
        })
    }
}
fn dropped(reason: &'static str, count: u64) {
    if count != 0 {
        metrics::counter!("eez_telemetry_dropped_events_total", "signal" => "trace", "reason" => reason).increment(count);
    }
}
impl SpanProcessor for Processor {
    fn on_start(&self, _: &mut opentelemetry_sdk::trace::Span, _: &Context) {}
    fn on_end(&self, span: SpanData) {
        let Ok(sender) = self.sender.try_lock() else {
            dropped("contention", 1);
            return;
        };
        let Some(sender) = sender.as_ref() else {
            dropped("shutdown", 1);
            return;
        };
        match sender.try_send(span) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => dropped("queue_full", 1),
            Err(mpsc::TrySendError::Disconnected(_)) => dropped("shutdown", 1),
        }
    }
    fn force_flush(&self) -> OTelSdkResult {
        // Only post-drain shutdown is supported; normal export starts immediately.
        Err(OTelSdkError::InternalFailure(
            "use budgeted tracing shutdown".into(),
        ))
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        // No I/O under this lock. Removing the only sender orders shutdown after
        // every admitted send and disconnects only after all queued spans drain.
        self.sender
            .lock()
            .map_err(|_| OTelSdkError::InternalFailure("trace sender poisoned".into()))?
            .take();
        self.done
            .lock()
            .map_err(|_| OTelSdkError::InternalFailure("trace completion poisoned".into()))?
            .recv_timeout(timeout)
            .map_err(|_| OTelSdkError::Timeout(timeout))
    }
}

/// Optional diagnostic identity; absence/sampling never filters lifecycle events.
pub fn current() -> Option<crate::schema::TraceContext> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let context = span.span_context();
    context.is_valid().then(|| crate::schema::TraceContext {
        trace_id: crate::schema::TraceId(context.trace_id().to_bytes()),
        span_id: crate::schema::SpanId(context.span_id().to_bytes()),
    })
}

/// Make the optional OTel identity visible to the existing JSON logging layer.
/// Call only after setting a remote parent: asking for context starts the span.
/// Callers declare a `trace_id` field; IDs remain fields, never indexed labels.
pub fn record_trace_id(span: &tracing::Span) {
    let context = span.context();
    let context_span = context.span();
    let identity = context_span.span_context();
    if identity.is_valid() {
        span.record("trace_id", identity.trace_id().to_string());
    }
}

pub fn record_preparation(id: crate::schema::PreparationId) {
    let span = tracing::Span::current();
    span.record("preparation_epoch", hex::encode(id.epoch.0));
    span.record("preparation_sequence", id.sequence);
}

/// Finite per-source preparation references, linked to the batch span. These are
/// diagnostic observations at preparation, not simulated admission-to-finality
/// spans. Exact SQL admission identities survive independently of sampling.
pub fn link_members(members: &[crate::schema::Member]) {
    let batch = tracing::Span::current();
    if !batch.context().span().span_context().is_sampled() {
        return;
    }
    for member in members.iter().take(128) {
        let source = tracing::info_span!(target: "eez_trace", parent: None, "batch_member",
            admission_epoch = %hex::encode(member.admission.epoch.0),
            admission_sequence = member.admission.sequence,
            source_hash = %hex::encode(member.source.transaction_hash.0),
            chain_id = member.source.chain_id,
            genesis_hash = %hex::encode(member.source.genesis_hash.0),
            direction = ?member.source.direction,
            rollup_id = member.source.rollup_id,
        );
        let context = source.context();
        batch.add_link(context.span().span_context().clone());
    }
    dropped("link_limit", members.len().saturating_sub(128) as u64);
}

/// Inject only W3C trace context. No baggage, pending envelopes or RPC payloads.
pub fn inject(metadata: &mut tonic::metadata::MetadataMap) {
    use opentelemetry::propagation::{Injector, TextMapPropagator};
    struct Headers<'a>(&'a mut tonic::metadata::MetadataMap);
    impl Injector for Headers<'_> {
        fn set(&mut self, key: &str, value: String) {
            if let (Ok(key), Ok(value)) = (
                key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                value.parse(),
            ) {
                self.0.insert(key, value);
            }
        }
    }
    opentelemetry_sdk::propagation::TraceContextPropagator::new()
        .inject_context(&tracing::Span::current().context(), &mut Headers(metadata));
}

/// Extract before consuming the request stream. Invalid, duplicate or oversized
/// headers are ignored without changing RPC admission, deadlines or statuses.
pub fn extract(metadata: &tonic::metadata::MetadataMap) -> Context {
    use opentelemetry::propagation::{Extractor, TextMapPropagator};
    struct Headers<'a> {
        parent: &'a str,
        state: Option<&'a str>,
    }
    impl Extractor for Headers<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            match key {
                "traceparent" => Some(self.parent),
                "tracestate" => self.state,
                _ => None,
            }
        }
        fn keys(&self) -> Vec<&str> {
            vec!["traceparent", "tracestate"]
        }
    }
    let invalid = || {
        dropped("invalid_context", 1);
        Context::new()
    };
    if metadata.get_all("traceparent").iter().count() > 1
        || metadata.get_all("tracestate").iter().count() > 1
    {
        return invalid();
    }
    let Some(parent) = metadata.get("traceparent") else {
        return Context::new();
    };
    let Ok(parent) = parent.to_str() else {
        return invalid();
    };
    if parent.len() < 55
        || parent.len() > 512
        || parent.as_bytes().get(2) != Some(&b'-')
        || parent.as_bytes().get(35) != Some(&b'-')
        || parent.as_bytes().get(52) != Some(&b'-')
    {
        return invalid();
    }
    let state = match metadata.get("tracestate") {
        Some(state) => match state.to_str() {
            Ok(state)
                if state.len() <= 512
                    && state.split(',').count() <= 32
                    && state.parse::<opentelemetry::trace::TraceState>().is_ok() =>
            {
                Some(state)
            }
            _ => return invalid(),
        },
        None => None,
    };
    let context = opentelemetry_sdk::propagation::TraceContextPropagator::new()
        .extract(&Headers { parent, state });
    if !context.span().span_context().is_valid() {
        return invalid();
    }
    context
}

pub fn attach_remote_parent(span: &tracing::Span, metadata: &tonic::metadata::MetadataMap) {
    let context = extract(metadata);
    if context.span().span_context().is_valid() {
        let _ = span.set_parent(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    fn config() -> Config {
        Config {
            receiver: "127.0.0.1:4318".parse().unwrap(),
            allow_non_loopback: false,
            sample_ratio: 1.0,
            queue_capacity: 2,
            export_timeout_ms: 20,
        }
    }
    fn identity() -> Identity {
        Identity {
            environment: "devnet".into(),
            deployment: "test".into(),
            service: "eez-composer".into(),
            instance: "one".into(),
        }
    }
    #[test]
    fn config_is_explicit_bounded_and_disabled_by_default() {
        let config: Config =
            serde_json::from_str(r#"{"receiver":"127.0.0.1:4318","sample_ratio":0.1}"#).unwrap();
        assert_eq!(config.queue_capacity, 2048);
        assert_eq!(config.export_timeout_ms, 1000);
        assert!(config.validate().is_ok());
        let (guard, layer) = Guard::start(&identity(), None).unwrap();
        assert!(layer.is_none());
        assert!(guard.shutdown(Duration::ZERO));
        assert!(Guard::default().shutdown_without_application_deadline());
        for ratio in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            let mut candidate = config.clone();
            candidate.sample_ratio = ratio;
            assert!(candidate.validate().is_err());
        }
        for capacity in [0, 16385] {
            let mut candidate = config.clone();
            candidate.queue_capacity = capacity;
            assert!(candidate.validate().is_err());
        }
        for timeout in [0, 2001] {
            let mut candidate = config.clone();
            candidate.export_timeout_ms = timeout;
            assert!(candidate.validate().is_err());
        }
        let mut candidate = config.clone();
        candidate.receiver = "10.0.0.1:4318".parse().unwrap();
        assert!(candidate.validate().is_err());
        candidate.allow_non_loopback = true;
        assert!(candidate.validate().is_ok());
        assert_eq!(flush_budget(Duration::from_secs(4)), Duration::from_secs(1));
        assert_eq!(
            flush_budget(Duration::from_secs(40)),
            Duration::from_secs(2)
        );
        assert_eq!(flush_budget(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn malformed_duplicate_and_oversized_context_cannot_affect_rpc() {
        let valid = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
        for parent in [
            "",
            "00-0-0-01",
            "00-00000000000000000000000000000000-0123456789abcdef-01",
            "00-0123456789ABCDEF0123456789abcdef-0123456789abcdef-01",
        ] {
            let mut headers = tonic::metadata::MetadataMap::new();
            headers.insert("traceparent", parent.parse().unwrap());
            assert!(!extract(&headers).span().span_context().is_valid());
        }
        let mut headers = tonic::metadata::MetadataMap::new();
        headers.insert("traceparent", valid.parse().unwrap());
        headers.insert("baggage", "secret=ignored".parse().unwrap());
        let context = extract(&headers);
        assert!(context.span().span_context().is_remote());
        assert_eq!(
            context.span().span_context().trace_id().to_string(),
            "0123456789abcdef0123456789abcdef"
        );
        headers.append("traceparent", valid.parse().unwrap());
        assert!(!extract(&headers).span().span_context().is_valid());
        headers.remove("traceparent");
        headers.insert("traceparent", valid.parse().unwrap());
        headers.insert("tracestate", "not-a-member".parse().unwrap());
        assert!(!extract(&headers).span().span_context().is_valid());
        headers.insert(
            "tracestate",
            format!("key={}", "x".repeat(513)).parse().unwrap(),
        );
        assert!(!extract(&headers).span().span_context().is_valid());
    }

    #[derive(Debug, Clone)]
    struct Capture(Arc<Mutex<Vec<SpanData>>>);
    impl SpanProcessor for Capture {
        fn on_start(&self, _: &mut opentelemetry_sdk::trace::Span, _: &Context) {}
        fn on_end(&self, span: SpanData) {
            self.0.lock().unwrap().push(span);
        }
        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }
        fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
            Ok(())
        }
    }
    #[test]
    fn composer_client_and_remote_signer_share_trace_with_separate_span_ids() {
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let provider = SdkTracerProvider::builder()
            .with_span_processor(capture.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        let headers = tracing::subscriber::with_default(subscriber, || {
            let composer = tracing::info_span!(target: "eez_trace", "composer_proving");
            let _composer = composer.enter();
            let client = tracing::info_span!(target: "eez_trace", "prover_client");
            let _client = client.enter();
            let mut headers = tonic::metadata::MetadataMap::new();
            inject(&mut headers);
            assert!(headers.get("baggage").is_none());
            headers
        });
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let server = tracing::info_span!(target: "eez_trace", "prove");
            attach_remote_parent(&server, &headers);
            let _server = server.enter();
        });
        let spans = capture.0.lock().unwrap();
        let composer = spans
            .iter()
            .find(|span| span.name == "composer_proving")
            .unwrap();
        let client = spans
            .iter()
            .find(|span| span.name == "prover_client")
            .unwrap();
        let server = spans.iter().find(|span| span.name == "prove").unwrap();
        assert_eq!(client.parent_span_id, composer.span_context.span_id());
        assert_eq!(server.parent_span_id, client.span_context.span_id());
        assert_eq!(
            server.span_context.trace_id(),
            composer.span_context.trace_id()
        );
        assert!(server.parent_span_is_remote);
    }

    #[test]
    fn collector_outage_does_not_block_emission_or_exhausted_shutdown() {
        let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let receiver = reserved.local_addr().unwrap();
        drop(reserved);
        let mut config = config();
        config.receiver = receiver;
        let (guard, layer) = Guard::start(&identity(), Some(&config)).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let start = std::time::Instant::now();
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..1000 {
                let _span = tracing::info_span!(target: "eez_trace", "prover_attempt");
            }
        });
        let _ = guard.shutdown(Duration::ZERO);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn otlp_http_export_uses_exact_identity_and_excludes_log_events() {
        assert_otlp_http_export(Some(Duration::from_secs(8)));
    }

    #[test]
    fn deadline_free_post_drain_shutdown_flushes_reachable_collector() {
        assert_otlp_http_export(None);
    }

    fn assert_otlp_http_export(remaining: Option<Duration>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = config();
        config.receiver = listener.local_addr().unwrap();
        config.export_timeout_ms = 1000;
        let receiver = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 8192);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("POST /v1/traces HTTP/1.1"));
            assert!(!header.to_lowercase().contains("authorization"));
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|value| value.parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            ExportTraceServiceRequest::decode(body.as_slice()).unwrap()
        });
        let (guard, layer) = Guard::start(&identity(), Some(&config)).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span =
                tracing::info_span!(target: "eez_trace", "prover_attempt", retry_attempt = 2u64);
            let _entered = span.enter();
            tracing::error!(target: "eez_trace", arbitrary_payload = "must not be exported", "diagnostic only");
        });
        assert!(match remaining {
            Some(remaining) => guard.shutdown(remaining),
            None => guard.shutdown_without_application_deadline(),
        });
        let request = receiver.join().unwrap();
        assert_eq!(request.resource_spans.len(), 1);
        let resource = request.resource_spans[0].resource.as_ref().unwrap();
        assert!(
            resource
                .attributes
                .iter()
                .any(|attribute| attribute.key == "service.name")
        );
        assert!(
            resource
                .attributes
                .iter()
                .any(|attribute| attribute.key == "eez.deployment")
        );
        let spans = &request.resource_spans[0].scope_spans[0].spans;
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "prover_attempt");
        assert!(spans[0].events.is_empty());
        assert!(!format!("{request:?}").contains("must not be exported"));
    }

    #[test]
    fn deadline_free_post_drain_shutdown_is_bounded_when_collector_stalls() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = config();
        config.receiver = listener.local_addr().unwrap();
        config.export_timeout_ms = 2000;
        let (accepted, connected) = std::sync::mpsc::sync_channel(1);
        let (release, finish) = std::sync::mpsc::sync_channel(1);
        let receiver = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            accepted.send(()).unwrap();
            // Hold the actual accepted socket without a response until the
            // caller's bounded shutdown returns; no success-by-sleep assertion.
            let _ = finish.recv_timeout(Duration::from_secs(5));
            drop(stream);
        });
        let (guard, layer) = Guard::start(&identity(), Some(&config)).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            drop(tracing::info_span!(target: "eez_trace", "prover_attempt"));
            connected.recv_timeout(Duration::from_secs(3)).unwrap();
            // A second queued batch ensures one request timeout cannot finish
            // the export worker before the independent total flush budget.
            drop(tracing::info_span!(target: "eez_trace", "prover_attempt"));
        });
        let start = std::time::Instant::now();
        assert!(!guard.shutdown_without_application_deadline());
        assert!(start.elapsed() < Duration::from_millis(2500));
        release.send(()).unwrap();
        receiver.join().unwrap();
    }

    #[test]
    fn batch_links_keep_admission_identity_and_lifecycle_context() {
        use crate::schema::{AdmissionId, Direction, Epoch, Hash32, Member, Source};
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let provider = SdkTracerProvider::builder()
            .with_span_processor(capture.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(target: "eez_trace", "composer_preparation");
            let _entered = span.enter();
            let members = [1, 2].map(|sequence| Member {
                admission: AdmissionId {
                    epoch: Epoch([1; 16]),
                    sequence,
                },
                source: Source {
                    chain_id: 100,
                    genesis_hash: Hash32([2; 32]),
                    rollup_id: 9,
                    transaction_hash: Hash32([sequence as u8; 32]),
                    direction: Direction::Inbound,
                },
            });
            link_members(&members);
            assert!(current().is_some());
        });
        let spans = capture.0.lock().unwrap();
        let batch = spans
            .iter()
            .find(|span| span.name == "composer_preparation")
            .unwrap();
        assert_eq!(batch.links.links.len(), 2);
        let sources: Vec<_> = spans
            .iter()
            .filter(|span| span.name == "batch_member")
            .collect();
        assert_eq!(sources.len(), 2);
        for source in sources {
            assert!(
                batch
                    .links
                    .links
                    .iter()
                    .any(|link| link.span_context == source.span_context)
            );
            assert!(
                source
                    .attributes
                    .iter()
                    .any(|attribute| attribute.key.as_str() == "admission_sequence")
            );
        }
    }
    #[cfg(feature = "native")]
    #[test]
    fn queue_pressure_contention_and_shutdown_have_distinct_loss_counters() {
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let provider = SdkTracerProvider::builder()
            .with_span_processor(capture.clone())
            .build();
        tracing::subscriber::with_default(
            tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test"))),
            || {
                let _span = tracing::info_span!(target: "eez_trace", "prover_attempt");
            },
        );
        let data = capture.0.lock().unwrap().pop().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let (_finished, done) = mpsc::sync_channel(1);
        let processor = Processor {
            sender: Mutex::new(Some(sender)),
            done: Mutex::new(done),
        };
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            processor.on_end(data.clone());
            processor.on_end(data.clone()); // A stalled collector fills the sole queue slot.
            let held = processor.sender.lock().unwrap();
            processor.on_end(data.clone()); // Emission cannot wait even for another publisher.
            drop(held);
            assert!(processor.shutdown_with_timeout(Duration::ZERO).is_err());
            processor.on_end(data);
        });
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
        let rendered = handle.render();
        for reason in ["queue_full", "contention", "shutdown"] {
            assert!(
                rendered.lines().any(|line| line
                    .starts_with("eez_telemetry_dropped_events_total{")
                    && line.contains(&format!("reason=\"{reason}\""))
                    && line.ends_with(" 1")),
                "{rendered}"
            );
        }
    }
    #[cfg(feature = "native")]
    #[test]
    fn configured_trace_health_and_zero_loss_counters_exist_before_first_span() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let (guard, layer) = Guard::start(&identity(), Some(&config())).unwrap();
            drop(layer);
            assert!(guard.shutdown(Duration::from_secs(4)));
        });
        let metrics = handle.render();
        assert!(metrics.contains("eez_trace_export_enabled 1"), "{metrics}");
        assert!(metrics.contains("eez_trace_sample_ratio 1"), "{metrics}");
        assert!(
            metrics.lines().any(
                |line| line.starts_with("eez_telemetry_dropped_events_total{")
                    && line.contains("reason=\"queue_full\"")
                    && line.ends_with(" 0")
            ),
            "{metrics}"
        );
    }
}

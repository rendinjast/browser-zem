//! Forwards this add-on's structured tracing events to the host's log viewer.
//!
//! A [`HostLogLayer`] sits on the add-on's subscriber alongside the `fmt`
//! layer. Events at `INFO` and above are pushed (non-blocking, dropped when the
//! queue is full) to a background task that sends them to the host's addon DB
//! socket as `log` messages. `DEBUG`/`TRACE` stay local to the add-on's stderr
//! so debug verbosity never floods the host control channel.

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tracing::Level;
use tracing_subscriber::Layer;
use zem_addon_protocol::JsonClient;

#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: Vec<serde_json::Value>,
}

impl FieldVisitor {
    fn collect(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(serde_json::json!({
                "name": field.name(),
                "value": value,
            }));
        }
    }
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{:?}", value);
        let rendered = rendered
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&rendered);
        self.collect(field, rendered);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.collect(field, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.collect(field, &value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.collect(field, &value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.collect(field, &value.to_string());
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.collect(field, &value.to_string());
    }
}

/// Forwards events to the host. Dropping this layer's task stops forwarding;
/// log loss is acceptable, so failures are never surfaced.
pub struct HostLogLayer {
    tx: mpsc::Sender<serde_json::Value>,
}

impl HostLogLayer {
    pub fn new() -> Self {
        let (tx, mut rx): (
            mpsc::Sender<serde_json::Value>,
            mpsc::Receiver<serde_json::Value>,
        ) = mpsc::channel(1024);
        let addon_id = std::env::var("ADDON_ID").unwrap_or_else(|_| "addon".to_string());
        let db_endpoint = std::env::var("ADDON_DB_ENDPOINT").unwrap_or_default();
        let socket_path = db_endpoint
            .strip_prefix("unix://")
            .unwrap_or(&db_endpoint)
            .to_string();

        tokio::spawn(async move {
            if socket_path.is_empty() {
                return;
            }
            let client = JsonClient::new(socket_path);
            while let Some(mut params) = rx.recv().await {
                params["addon_id"] = addon_id.clone().into();
                let _ = client.request("log", params).await;
            }
        });

        Self { tx }
    }
}

impl<S> Layer<S> for HostLogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() < Level::INFO {
            return;
        }

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let params = serde_json::json!({
            "level": event.metadata().level().to_string(),
            "target": event.metadata().target(),
            "message": visitor.message,
            "fields": visitor.fields,
            "timestamp": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        });

        // try_send: never block the add-on's own logging on the host channel.
        let _ = self.tx.try_send(params);
    }
}

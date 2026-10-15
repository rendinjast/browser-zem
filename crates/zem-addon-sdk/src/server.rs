use base64::Engine;
use futures_util::FutureExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixStream;

use zem_addon_protocol::framing::{read_message, write_message, write_output_event};
use zem_addon_protocol::{AddonError, JsonMessage};

use crate::builders::{
    ActionHandler, ActionRequest, AppEventHandler, OnCreate, OnDestroy, OnResize, OperationHandler,
    OperationRequest, SubscribeOutput,
};
use crate::context::AddonContext;

// ---------------------------------------------------------------------------
// Panic isolation
//
// Handlers run user-supplied add-on code. A panic must never abort the add-on
// process (release profile uses `panic = "abort"`, which would kill every
// other pane served by this add-on). Every dispatched future is therefore
// caught at the boundary and converted into a structured `AddonError` so only
// the offending request fails.
// ---------------------------------------------------------------------------

/// Await `fut`, converting any panic inside it into `AddonError::handler_panic`.
async fn catch_panic<F, T>(fut: F) -> Result<T, AddonError>
where
    F: Future<Output = Result<T, AddonError>> + Send,
{
    let out = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
    match out {
        Ok(result) => result,
        Err(payload) => {
            let detail = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "addon handler panicked".to_string()
            };
            tracing::error!("addon handler panicked: {}", detail);
            Err(AddonError::code(
                zem_addon_protocol::ERR_HANDLER_PANIC,
                format!("addon handler panicked: {}", detail),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn json_pane_id(v: &serde_json::Value) -> i32 {
    v.get("pane_id").and_then(|x| x.as_i64()).unwrap_or(0) as i32
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn json_extra(v: &serde_json::Value) -> serde_json::Value {
    v.get("extra")
        .and_then(|x| {
            if x.is_string() {
                serde_json::from_str(x.as_str()?).ok()
            } else {
                Some(x.clone())
            }
        })
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
}

// ---------------------------------------------------------------------------
// PaneLifecycle (internal)
// ---------------------------------------------------------------------------

pub(crate) struct PaneLifecycle {
    pub on_create: Option<OnCreate>,
    pub on_destroy: Option<OnDestroy>,
    pub on_resize: Option<OnResize>,
    pub subscribe_output: Option<SubscribeOutput>,
}

impl Clone for PaneLifecycle {
    fn clone(&self) -> Self {
        Self {
            on_create: self.on_create.clone(),
            on_destroy: self.on_destroy.clone(),
            on_resize: self.on_resize.clone(),
            subscribe_output: self.subscribe_output.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// JsonServer (internal)
// ---------------------------------------------------------------------------

pub(crate) struct JsonServer {
    pub queries: HashMap<String, OperationHandler>,
    pub mutations: HashMap<String, OperationHandler>,
    pub actions: HashMap<String, ActionHandler>,
    pub input_handlers: HashMap<String, crate::builders::InputHandler>,
    pub pane_lifecycle: HashMap<String, PaneLifecycle>,
    pub pane_collections: HashMap<String, String>,
    pub app_events: HashMap<String, AppEventHandler>,
    pub ctx: AddonContext,
    pub panes: Arc<Mutex<HashMap<i32, String>>>,
    pub log_level_handle: tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >,
}

impl JsonServer {
    pub async fn handle_message(&self, msg: JsonMessage) -> JsonMessage {
        let method = match &msg.method {
            Some(m) => m.as_str(),
            None => return JsonMessage::error(msg.id, AddonError::internal("missing method")),
        };

        let params = msg.params.unwrap_or(serde_json::Value::Null);
        let result = match method {
            "on_create" => self.handle_on_create(&params).await,
            "on_destroy" => self.handle_on_destroy(&params).await,
            "on_resize" => self.handle_on_resize(&params).await,
            "on_app_event" => self.handle_on_app_event(&params).await,
            "execute_pane_operation" => self.handle_execute_pane_operation(&params).await,
            "execute_action" => self.handle_execute_action(&params).await,
            "write_input" => self.handle_write_input(&params).await,
            _ => Err(AddonError::code(
                zem_addon_protocol::ERR_UNKNOWN_METHOD,
                format!("unknown method: {}", method),
            )),
        };

        match result {
            Ok(r) => JsonMessage::response(msg.id, r),
            Err(e) => JsonMessage::error(msg.id, e),
        }
    }

    async fn handle_on_create(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let pane_id = json_pane_id(params);
        let pane = json_str(params, "pane");
        let cwd = json_str(params, "cwd");
        let extra = json_extra(params);

        tracing::info!("on_create pane {}", pane_id);

        let collection = self
            .pane_collections
            .get(&pane)
            .cloned()
            .unwrap_or_default();
        self.panes.lock().unwrap().insert(pane_id, pane.clone());

        // The handler is awaited before responding so the host only continues to
        // `subscribe_output` once the pane is fully initialized.
        if let Some(lc) = self.pane_lifecycle.get(&pane) {
            if let Some(on_create) = &lc.on_create {
                catch_panic(on_create(pane_id, &cwd, &extra, &self.ctx)).await?;
            }
        }

        Ok(serde_json::json!({ "extra": "", "collection": collection }))
    }

    async fn handle_on_destroy(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let pane_id = json_pane_id(params);
        let pane = json_str(params, "pane");
        let reason = json_str(params, "reason");

        tracing::info!("on_destroy pane {} ({})", pane_id, reason);

        self.panes.lock().unwrap().remove(&pane_id);
        if let Some(lc) = self.pane_lifecycle.get(&pane) {
            if let Some(on_destroy) = &lc.on_destroy {
                catch_panic(on_destroy(pane_id, &reason, &self.ctx)).await?;
            }
        }

        Ok(serde_json::Value::Null)
    }

    async fn handle_on_resize(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let pane_id = json_pane_id(params);
        let pane = json_str(params, "pane");
        let cols = params.get("cols").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let rows = params.get("rows").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let width_px = params.get("width_px").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let height_px = params
            .get("height_px")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u16;

        tracing::info!("on_resize pane {} -> {}x{}", pane_id, cols, rows);

        if let Some(lc) = self.pane_lifecycle.get(&pane) {
            if let Some(on_resize) = &lc.on_resize {
                catch_panic(on_resize(
                    pane_id, cols, rows, width_px, height_px, &self.ctx,
                ))
                .await?;
            }
        }

        Ok(serde_json::Value::Null)
    }

    async fn handle_on_app_event(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let event_type = json_str(params, "event_type");
        let payload = params
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Built-in handling for the host log level so this add-on's verbosity
        // follows the host. Runs before user handlers so they see the new level.
        if event_type == "log_level_changed" {
            if let Some(level) = payload.get("level").and_then(|v| v.as_str()) {
                if let Ok(filter) = tracing_subscriber::EnvFilter::try_new(level) {
                    let _ = self.log_level_handle.reload(filter);
                }
            }
        }

        if let Some(handler) = self.app_events.get(&event_type) {
            if let Err(e) = catch_panic(handler(&payload, &self.ctx)).await {
                tracing::warn!("app event handler failed: {}", e);
            }
        }

        Ok(serde_json::Value::Null)
    }

    async fn handle_execute_pane_operation(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let pane_id = json_pane_id(params);
        let pane = json_str(params, "pane");
        let operation = json_str(params, "operation");
        let kind = json_str(params, "kind");
        let cwd = json_str(params, "cwd");
        let payload_b64 = params.get("payload").and_then(|v| v.as_str()).unwrap_or("");

        let raw = base64::engine::general_purpose::STANDARD
            .decode(payload_b64)
            .map_err(|e| AddonError::invalid_payload(format!("invalid payload encoding: {}", e)))?;
        let payload = serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null);

        if pane.is_empty() {
            return Err(AddonError::invalid_payload(
                "pane is required for pane operations",
            ));
        }
        let key = format!("{}.{}", pane, operation);
        let handler = match kind.as_str() {
            "query" => self.queries.get(&key).cloned(),
            "mutation" => self.mutations.get(&key).cloned(),
            _ => None,
        };
        let handler = handler.ok_or_else(|| AddonError::unknown_operation(&key))?;

        let req = OperationRequest {
            pane_id,
            pane: &pane,
            cwd: &cwd,
            payload: &payload,
            raw: &raw,
        };

        match catch_panic(handler(&req, &self.ctx)).await {
            Ok(v) => {
                let out = serde_json::to_vec(&v).map_err(|e| {
                    AddonError::internal(format!("failed to serialize response: {}", e))
                })?;
                Ok(serde_json::json!({
                    "is_final": true,
                    "payload": base64::engine::general_purpose::STANDARD.encode(&out),
                    "error": "",
                }))
            }
            Err(e) => Ok(serde_json::json!({
                "is_final": true,
                "payload": "",
                "error": serde_json::to_value(&e).unwrap_or(serde_json::Value::Null),
            })),
        }
    }

    async fn handle_execute_action(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let cwd = json_str(params, "cwd");
        let payload_b64 = params.get("payload").and_then(|v| v.as_str()).unwrap_or("");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(payload_b64)
            .map_err(|e| AddonError::invalid_payload(format!("invalid payload encoding: {}", e)))?;
        let payload = serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null);
        let action = json_str(params, "action");
        let handler = self
            .actions
            .get(&action)
            .cloned()
            .ok_or_else(|| AddonError::unknown_action(&action))?;
        let request = ActionRequest {
            pane_id: json_pane_id(params),
            cwd: &cwd,
            payload: &payload,
            raw: &raw,
        };
        let response = catch_panic(handler(&request, &self.ctx)).await?;
        Ok(serde_json::to_value(response).map_err(|e| AddonError::internal(e.to_string()))?)
    }

    async fn handle_write_input(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AddonError> {
        let pane_id = json_pane_id(params);
        let pane = json_str(params, "pane");
        let cwd = json_str(params, "cwd");
        let payload_b64 = params.get("payload").and_then(|v| v.as_str()).unwrap_or("");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(payload_b64)
            .map_err(|e| AddonError::invalid_payload(format!("invalid payload encoding: {}", e)))?;
        let payload = serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null);
        let handler = self.input_handlers.get(&pane).cloned().ok_or_else(|| {
            AddonError::not_supported(format!("input is not supported for pane '{}'", pane))
        })?;
        let request = OperationRequest {
            pane_id,
            pane: &pane,
            cwd: &cwd,
            payload: &payload,
            raw: &raw,
        };
        catch_panic(handler(&request, &self.ctx)).await?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn forward_output_events(
        &self,
        params: serde_json::Value,
        writer: &mut (impl AsyncWriteExt + Unpin),
    ) -> Result<(), AddonError> {
        let pane_id = json_pane_id(&params);

        let pane_name = {
            let panes = self.panes.lock().unwrap();
            panes.get(&pane_id).cloned().unwrap_or_default()
        };

        let subscribe = self
            .pane_lifecycle
            .get(&pane_name)
            .and_then(|lc| lc.subscribe_output.clone())
            .ok_or_else(|| AddonError::not_supported("subscribe_output not implemented"))?;

        let mut rx = subscribe(pane_id, &self.ctx).await?;

        while let Some(event) = rx.recv().await {
            // Output streams as binary frames, not line-delimited JSON arrays —
            // terminal PTY chunks are much cheaper as raw bytes.
            write_output_event(writer, &event)
                .await
                .map_err(|e| AddonError::internal(e.to_string()))?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Connection handler (internal)
// ---------------------------------------------------------------------------

pub(crate) async fn handle_connection(
    stream: UnixStream,
    server: Arc<JsonServer>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = AsyncBufReader::new(reader);

    loop {
        let msg = match read_message(&mut reader).await? {
            Some(m) => m,
            None => break,
        };

        if msg.method.as_deref() == Some("subscribe_output") {
            let params = msg.params.unwrap_or(serde_json::Value::Null);
            let response = JsonMessage::response(msg.id, serde_json::json!({}));
            write_message(&mut writer, &response).await?;

            if let Err(e) = server.forward_output_events(params, &mut writer).await {
                tracing::warn!("output forwarding failed: {}", e);
            }
            break;
        }

        let response = server.handle_message(msg).await;
        write_message(&mut writer, &response).await?;
    }

    Ok(())
}

use base64::Engine;

use zem_addon_protocol::JsonClient;

/// Context passed to addon handlers, providing access to server communication.
///
/// The context is immutable and cheaply cloneable. Per-request state (`pane_id`,
/// `cwd`, payload) is passed to handlers explicitly as a [`OperationRequest`](crate::OperationRequest)
/// instead of being stashed in the context, so concurrent handlers can never
/// clobber each other's request data.
#[derive(Clone)]
pub struct AddonContext {
    client: JsonClient,
    addon_id: String,
}

impl AddonContext {
    /// Create a context for the given addon DB endpoint (`unix://` prefix
    /// optional) and addon identifier.
    pub fn new(db_endpoint: &str, addon_id: &str) -> Self {
        let path = db_endpoint.strip_prefix("unix://").unwrap_or(db_endpoint);
        Self {
            client: JsonClient::new(path.to_string()),
            addon_id: addon_id.to_string(),
        }
    }

    /// Build a context from the standard addon environment (`ADDON_DB_ENDPOINT`
    /// and `ADDON_ID`), which is what the zem host sets when spawning addons.
    pub fn from_env() -> Self {
        let db_endpoint = std::env::var("ADDON_DB_ENDPOINT").unwrap_or_default();
        let addon_id = std::env::var("ADDON_ID").unwrap_or_else(|_| "addon".to_string());
        Self::new(&db_endpoint, &addon_id)
    }

    /// The addon identifier (e.g. `com.zem.terminal`).
    pub fn addon_id(&self) -> &str {
        &self.addon_id
    }

    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.client.request(method, params).await
    }

    /// Set the status of a pane.
    pub async fn set_status(&self, pane_id: i32, status: &str) -> anyhow::Result<()> {
        self.send_request(
            "set_status",
            serde_json::json!({
                "pane_id": pane_id,
                "status": status,
            }),
        )
        .await?;
        Ok(())
    }

    /// Set extra metadata for a pane as a raw string.
    pub async fn set_extra_raw(&self, pane_id: i32, extra: &str) -> anyhow::Result<()> {
        self.send_request(
            "set_extra",
            serde_json::json!({
                "pane_id": pane_id,
                "extra": extra,
            }),
        )
        .await?;
        Ok(())
    }

    /// Set extra metadata for a pane as a JSON value.
    pub async fn set_extra(&self, pane_id: i32, extra: &serde_json::Value) -> anyhow::Result<()> {
        self.set_extra_raw(pane_id, &extra.to_string()).await
    }

    /// Get extra metadata for a pane as a string.
    pub async fn get_extra(&self, pane_id: i32) -> anyhow::Result<String> {
        let resp = self
            .send_request(
                "get_extra",
                serde_json::json!({
                    "pane_id": pane_id,
                }),
            )
            .await?;
        Ok(resp
            .get("extra")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// Get extra metadata for a pane as a JSON value.
    pub async fn get_extra_json(&self, pane_id: i32) -> anyhow::Result<serde_json::Value> {
        let raw = self.get_extra(pane_id).await?;
        Ok(serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null))
    }

    /// Add or remove this addon's collection membership for a pane.
    ///
    /// `collection_id` is the addon's manifest collection source id (for
    /// example `Explorer`), not the namespaced database id. Other collection
    /// memberships are preserved.
    pub async fn set_pane_collection(
        &self,
        pane_id: i32,
        collection_id: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        self.send_request(
            "set_pane_collection",
            serde_json::json!({
                "addon_id": self.addon_id,
                "pane_id": pane_id,
                "collection_id": collection_id,
                "enabled": enabled,
            }),
        )
        .await?;
        Ok(())
    }

    /// Ask the host to close a pane (e.g. the terminal addon closing a pane
    /// whose shell process exited after the user typed `exit`). The host runs
    /// the same close path as the HTTP layout API: the pane is removed from
    /// the layout, its addon session is torn down, and clients are notified.
    /// The host only permits closing panes of this addon's own kind.
    pub async fn close_pane(&self, pane_id: i32) -> anyhow::Result<()> {
        self.send_request(
            "close_pane",
            serde_json::json!({
                "addon_id": self.addon_id,
                "pane_id": pane_id,
            }),
        )
        .await?;
        Ok(())
    }

    /// Set the working directory for a pane.
    pub async fn set_cwd(&self, pane_id: i32, cwd: &str) -> anyhow::Result<()> {
        self.send_request(
            "set_cwd",
            serde_json::json!({
                "pane_id": pane_id,
                "cwd": cwd,
            }),
        )
        .await?;
        Ok(())
    }

    /// Dynamically set the pane's display name based on its state (e.g. the
    /// explorer addon naming a pane after the active connection). The
    /// user-set manual name always wins over this; an empty `name` clears it.
    pub async fn set_temp_name(&self, pane_id: i32, name: &str) -> anyhow::Result<()> {
        self.send_request(
            "set_temp_name",
            serde_json::json!({
                "pane_id": pane_id,
                "name": name,
            }),
        )
        .await?;
        Ok(())
    }

    /// Dynamically set the pane's display icon based on its state (e.g. a
    /// server icon while the explorer pane is on a remote connection). The
    /// user-set manual icon always wins over this; an empty `icon` clears it.
    pub async fn set_temp_icon(&self, pane_id: i32, icon: &str) -> anyhow::Result<()> {
        self.send_request(
            "set_temp_icon",
            serde_json::json!({
                "pane_id": pane_id,
                "icon": icon,
            }),
        )
        .await?;
        Ok(())
    }

    /// Append data to persistent storage (per-pane scope).
    pub async fn append_data(&self, pane_id: i32, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.append_data_inner(Some(pane_id), key, data).await
    }

    /// Get data from persistent storage (per-pane scope).
    pub async fn get_data(&self, pane_id: i32, key: &str) -> anyhow::Result<Vec<u8>> {
        self.get_data_inner(Some(pane_id), key).await
    }

    /// Send a notification to the user.
    pub async fn notify(
        &self,
        pane_id: i32,
        title: &str,
        body: &str,
        level: &str,
        silent: bool,
        action: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        let mut params = serde_json::json!({
            "pane_id": pane_id,
            "title": title,
            "body": body,
            "level": level,
            "silent": silent,
        });
        if let Some(action) = action {
            params["action"] = action;
        }
        self.send_request("notify", params).await?;
        Ok(())
    }

    /// Update an addon action item (view, options, and optional badge).
    /// `workspace_id` scopes the action to a specific workspace; use 0 for global actions.
    pub async fn set_action(
        &self,
        action_id: &str,
        workspace_id: i32,
        data: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.send_request(
            "update_action",
            serde_json::json!({
                "addon_id": self.addon_id,
                "action_id": action_id,
                "workspace_id": workspace_id,
                "data": data,
            }),
        )
        .await?;
        Ok(())
    }

    /// Validate that a user has access to a given path.
    pub async fn validate_path_access(&self, path: &str, user_uuid: &str) -> anyhow::Result<()> {
        self.send_request(
            "validate_path_access",
            serde_json::json!({
                "path": path,
                "user_uuid": user_uuid,
            }),
        )
        .await?;
        Ok(())
    }

    /// Get addon settings (stored under key "settings" in addon data, no pane scope).
    pub async fn get_settings(&self) -> anyhow::Result<serde_json::Value> {
        let data = self.get_data_global("settings").await?;
        if data.is_empty() {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        }
        Ok(serde_json::from_slice(&data)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())))
    }

    /// Set addon settings (stored under key "settings" in addon data, no pane scope).
    pub async fn set_settings(&self, settings: &serde_json::Value) -> anyhow::Result<()> {
        let data = serde_json::to_vec(settings).unwrap_or_default();
        self.send_request(
            "set_addon_data",
            serde_json::json!({
                "addon_id": self.addon_id,
                "key": "settings",
                "data": base64::engine::general_purpose::STANDARD.encode(&data),
            }),
        )
        .await?;
        Ok(())
    }

    /// Get data from global (non-pane-scoped) persistent storage.
    pub async fn get_global_data(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        self.get_data_global(key).await
    }

    /// Set data in global (non-pane-scoped) persistent storage (replace semantics).
    pub async fn set_global_data(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.send_request(
            "set_addon_data",
            serde_json::json!({
                "addon_id": self.addon_id,
                "key": key,
                "data": base64::engine::general_purpose::STANDARD.encode(data),
            }),
        )
        .await?;
        Ok(())
    }

    /// Set data in per-pane persistent storage (replace semantics).
    /// The explicit `pane_id` parameter is intentional — background tasks
    /// (e.g. scrollback autosave) may target a different pane than the current one.
    pub async fn set_pane_data(&self, pane_id: i32, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.send_request(
            "set_addon_data",
            serde_json::json!({
                "addon_id": self.addon_id,
                "pane_id": pane_id,
                "key": key,
                "data": base64::engine::general_purpose::STANDARD.encode(data),
            }),
        )
        .await?;
        Ok(())
    }

    // -- internal helpers --

    async fn get_data_global(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        self.get_data_inner(None, key).await
    }

    async fn append_data_inner(
        &self,
        pane_id: Option<i32>,
        key: &str,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.send_request(
            "append_data",
            serde_json::json!({
                "addon_id": self.addon_id,
                "pane_id": pane_id,
                "key": key,
                "data": base64::engine::general_purpose::STANDARD.encode(data),
            }),
        )
        .await?;
        Ok(())
    }

    async fn get_data_inner(&self, pane_id: Option<i32>, key: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .send_request(
                "get_data",
                serde_json::json!({
                    "addon_id": self.addon_id,
                    "pane_id": pane_id,
                    "key": key,
                    "from_seq": 0,
                    "limit": 0,
                }),
            )
            .await?;
        let rows = resp
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut data = Vec::new();
        for row in rows {
            if let Some(d) = row.get("data").and_then(|v| v.as_str()) {
                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(d) {
                    data.extend_from_slice(&decoded);
                }
            }
        }
        Ok(data)
    }
}

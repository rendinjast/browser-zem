//! The browser pane: exposes state persistence (history, current URL, scroll),
//! per-workspace bookmarks, and the proxy port to the client, plus the global
//! "open URL in a browser pane" actions.

use std::sync::Arc;

use zem_addon_sdk::{Action, ActionExecuteResponse, AddonContext, AddonError, Pane};

use crate::proxy::ProxyClient;

const STATE_KEY: &str = "browser_state";
const SCROLL_KEY: &str = "browser_scroll";

/// Adapt a typed async handler into the SDK's `Fn(&OperationRequest, &AddonContext)`
/// closure by extracting the owned request fields synchronously (the closure
/// must not borrow from the request across the await point).
macro_rules! op {
    ($handler:path) => {
        |req, ctx| {
            let pane_id = req.pane_id;
            let payload = req.payload.clone();
            let ctx = ctx.clone();
            async move { $handler(pane_id, payload, &ctx).await }
        }
    };
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct BrowserState {
    /// Current URL shown in the pane.
    url: String,
    /// History stack, oldest first. `index` is the current position.
    history: Vec<String>,
    #[serde(default)]
    index: usize,
}

pub fn browser_pane(proxy: Arc<ProxyClient>) -> Pane {
    Pane::new("browser")
        .icon("nerd:globe")
        .collection("browser")
        .query("get_state", op!(get_state))
        .query("get_bookmarks", op!(get_bookmarks))
        .query("get_proxy_info", {
            let proxy = proxy.clone();
            move |_req, _ctx| {
                let proxy = proxy.clone();
                async move { Ok(serde_json::json!({ "port": proxy.port() })) }
            }
        })
        .mutation("navigate", op!(navigate))
        .mutation("save_scroll", op!(save_scroll))
        .mutation("set_title", op!(set_title))
        .mutation("add_bookmark", op!(add_bookmark))
        .mutation("remove_bookmark", op!(remove_bookmark))
        .on_create(on_create_impl)
}

fn on_create_impl(
    pane_id: i32,
    cwd: &str,
    extra: &serde_json::Value,
    ctx: &AddonContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AddonError>> + Send>> {
    let extra = extra.clone();
    let cwd = cwd.to_string();
    let ctx = ctx.clone();
    Box::pin(async move { on_create(pane_id, &cwd, &extra, &ctx).await })
}

// ─── State ───────────────────────────────────────────────────────────────────

async fn read_state(ctx: &AddonContext, pane_id: i32) -> BrowserState {
    match ctx.get_data(pane_id, STATE_KEY).await {
        Ok(bytes) if !bytes.is_empty() => serde_json::from_slice(&bytes).unwrap_or_default(),
        _ => BrowserState::default(),
    }
}

async fn write_state(ctx: &AddonContext, pane_id: i32, state: &BrowserState) {
    if let Ok(bytes) = serde_json::to_vec(state) {
        let _ = ctx.set_pane_data(pane_id, STATE_KEY, &bytes).await;
    }
}

async fn get_state(
    pane_id: i32,
    _payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let state = read_state(ctx, pane_id).await;
    let scroll = ctx
        .get_data(pane_id, SCROLL_KEY)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<u32>(&b).ok())
        .unwrap_or(0);
    Ok(serde_json::json!({
        "url": state.url,
        "history": state.history,
        "index": state.index,
        "scroll": scroll,
    }))
}

/// Persist the client-side navigation state after any history/URL change.
///
/// Accepts `url` + `replace`, or an explicit `index` jump (back/forward). The
/// full resulting state is returned so the client can render it authoritatively.
async fn navigate(
    pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let mut state = read_state(ctx, pane_id).await;

    if let Some(idx) = payload.get("index").and_then(|v| v.as_i64()) {
        // Back/forward: jump to an existing history entry.
        if idx >= 0 && (idx as usize) < state.history.len() {
            state.index = idx as usize;
            state.url = state.history[state.index].clone();
        }
        write_state(ctx, pane_id, &state).await;
        return Ok(state_json(&state));
    }

    let url = payload
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        return Ok(state_json(&state));
    }
    let replace = payload.get("replace").and_then(|v| v.as_bool()).unwrap_or(false);

    if replace {
        if !state.history.is_empty() {
            state.history[state.index] = url.clone();
        } else {
            state.history.push(url.clone());
            state.index = 0;
        }
    } else {
        // Trim any forward entries (a new navigation discards the redo stack).
        state.history.truncate(state.index + 1);
        if state.history.last().map(String::as_str) != Some(url.as_str()) {
            state.history.push(url.clone());
        }
        state.index = state.history.len().saturating_sub(1);
    }
    state.url = url;
    write_state(ctx, pane_id, &state).await;

    Ok(state_json(&state))
}

fn state_json(state: &BrowserState) -> serde_json::Value {
    serde_json::json!({
        "url": state.url,
        "history": state.history,
        "index": state.index,
    })
}

/// Update the pane's visible title (page title reported by the bridge).
async fn set_title(
    pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !title.is_empty() {
        let _ = ctx.set_temp_name(pane_id, &title).await;
    }
    Ok(serde_json::json!({ "ok": true }))
}

/// Debounced scroll persistence, keyed per pane.
async fn save_scroll(
    pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    if let Some(y) = payload.get("y").and_then(|v| v.as_u64()) {
        if let Ok(bytes) = serde_json::to_vec(&(y as u32)) {
            let _ = ctx.set_pane_data(pane_id, SCROLL_KEY, &bytes).await;
        }
    }
    Ok(serde_json::json!({ "ok": true }))
}

async fn on_create(
    pane_id: i32,
    _cwd: &str,
    extra: &serde_json::Value,
    ctx: &AddonContext,
) -> Result<(), AddonError> {
    // The URL arrives as `{launch: {url}}` from the launch dialog or as a
    // bare `{url}` from the "Open URL…" action.
    let url = extra
        .get("launch")
        .and_then(|l| l.get("url"))
        .and_then(|v| v.as_str())
        .or_else(|| extra.get("url").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    if !url.is_empty() {
        let mut state = read_state(ctx, pane_id).await;
        state.history = vec![url.clone()];
        state.index = 0;
        state.url = url;
        write_state(ctx, pane_id, &state).await;
    }
    Ok(())
}

// ─── Bookmarks (workspace-scoped) ────────────────────────────────────────────

fn bookmarks_key(workspace_id: i32) -> String {
    format!("bookmarks:{}", workspace_id)
}

async fn get_bookmarks(
    _pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let workspace_id = payload
        .get("workspace_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let data = ctx.get_global_data(&bookmarks_key(workspace_id)).await.unwrap_or_default();
    let list: Vec<serde_json::Value> = if data.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(&data).unwrap_or_default()
    };
    Ok(serde_json::json!({ "bookmarks": list }))
}

async fn save_bookmarks(ctx: &AddonContext, workspace_id: i32, bookmarks: &[serde_json::Value]) {
    if let Ok(bytes) = serde_json::to_vec(bookmarks) {
        let _ = ctx.set_global_data(&bookmarks_key(workspace_id), &bytes).await;
    }
}

async fn add_bookmark(
    _pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let workspace_id = payload
        .get("workspace_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if url.is_empty() {
        return Err(AddonError::invalid_payload("url is required"));
    }

    let mut list: Vec<serde_json::Value> = {
        let data = ctx.get_global_data(&bookmarks_key(workspace_id)).await.unwrap_or_default();
        if data.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&data).unwrap_or_default()
        }
    };
    if !list.iter().any(|b| b.get("url").and_then(|u| u.as_str()) == Some(url.as_str())) {
        list.push(serde_json::json!({ "url": url, "title": title }));
    }
    save_bookmarks(ctx, workspace_id, &list).await;
    Ok(serde_json::json!({ "ok": true }))
}

async fn remove_bookmark(
    _pane_id: i32,
    payload: serde_json::Value,
    ctx: &AddonContext,
) -> Result<serde_json::Value, AddonError> {
    let workspace_id = payload
        .get("workspace_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let mut list: Vec<serde_json::Value> = {
        let data = ctx.get_global_data(&bookmarks_key(workspace_id)).await.unwrap_or_default();
        if data.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&data).unwrap_or_default()
        }
    };
    list.retain(|b| b.get("url").and_then(|u| u.as_str()) != Some(url.as_str()));
    save_bookmarks(ctx, workspace_id, &list).await;
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Actions ─────────────────────────────────────────────────────────────────

/// "Open URL…" — the action the client calls from the command palette / status
/// bar. Each URL gets its own browser pane (force_create) so multiple pages can
/// be open side by side.
pub fn open_url_action() -> Action {
    Action::new("open_url")
        .title("Open URL…")
        .icon("nerd:globe")
        .description("Open a URL in a new browser pane")
        .requires_write(false)
        .location("statusbar-right")
        .on_execute(|req, _ctx| {
            let url = req
                .payload
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            async move {
                if url.is_empty() {
                    return Err(AddonError::invalid_payload("url is required"));
                }
                Ok(ActionExecuteResponse::open_pane_with_params(
                    "browser",
                    serde_json::json!({ "url": url }),
                )
                .force_create())
            }
        })
}

/// Plain "New browser pane" action.
pub fn new_pane_action() -> Action {
    Action::new("browser_new")
        .title("New Browser Pane")
        .icon("nerd:globe")
        .description("Open a blank browser pane")
        .requires_write(false)
        .location("statusbar-right")
        .on_execute(|_req, _ctx| async move {
            Ok(ActionExecuteResponse::open_pane("browser").force_create())
        })
}

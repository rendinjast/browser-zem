use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload::Layer as ReloadLayer;
use tracing_subscriber::util::SubscriberInitExt;

use crate::builders::Addon;
use crate::context::AddonContext;
use crate::host_log::HostLogLayer;
use crate::manifest::generate_manifest;
use crate::server::{handle_connection, JsonServer, PaneLifecycle};

/// Initialize and run the addon.
///
/// This is the main entry point for addons. It:
/// 1. Handles `--generate-manifest` flag
/// 2. Sets up tracing (reloadable filter + host log forwarding)
/// 3. Generates the manifest file
/// 4. Builds the JSON server
/// 5. Starts listening for connections
pub async fn init(addon: Addon) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Handle --generate-manifest flag
    if std::env::args().any(|a| a == "--generate-manifest") {
        println!("{}", generate_manifest(&addon));
        std::process::exit(0);
    }

    // Setup tracing. The filter reads ADDON_LOG_LEVEL (set by the host) and can
    // be changed at runtime when the host broadcasts `log_level_changed`.
    let filter = tracing_subscriber::EnvFilter::try_new(
        &std::env::var("ADDON_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
    )
    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let (reload_filter, log_level_handle) = ReloadLayer::new(filter);
    let _ = tracing_subscriber::registry()
        .with(reload_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(HostLogLayer::new())
        .try_init();

    // Terminate if the host daemon dies, so this addon is never orphaned. Must
    // run before any long-lived work (socket bind, manifest write).
    crate::parent_watch::start();

    // Read environment
    let socket_path = std::env::var("ADDON_SOCKET").unwrap_or_else(|_| {
        tracing::warn!("ADDON_SOCKET not set, using default");
        "/tmp/zem-addon.sock".to_string()
    });

    // Generate manifest
    let addon_dir = std::env::var("ADDON_DIR").unwrap_or_else(|_| ".".to_string());
    let manifest_path = std::path::PathBuf::from(&addon_dir).join("manifest.toml");
    let manifest = generate_manifest(&addon);
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&manifest_path, &manifest)?;
    tracing::info!("Generated manifest at {}", manifest_path.display());

    // Build context and server
    let ctx = AddonContext::from_env();
    let (server, addon_name) = build_server(addon, ctx, log_level_handle);

    // Start listener
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    tracing::info!("Addon '{}' listening on {}", addon_name, socket_path);

    // Wait for DB socket to be ready before firing startup
    let db_socket = std::env::temp_dir().join("zem-internal.sock");
    for _ in 0..50 {
        if tokio::fs::metadata(&db_socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Fire startup event
    {
        let handler = server.app_events.get("startup").cloned();
        if let Some(ref handler) = handler {
            if let Err(e) = handler(&serde_json::Value::Null, &server.ctx).await {
                tracing::warn!("startup event handler failed: {}", e);
            }
        }
    }

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let server = server.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, server).await {
                        tracing::warn!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::error!("Accept error: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

fn build_server(
    addon: Addon,
    ctx: AddonContext,
    log_level_handle: tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >,
) -> (Arc<JsonServer>, String) {
    let mut queries = HashMap::new();
    let mut mutations = HashMap::new();
    let mut actions: HashMap<String, crate::builders::ActionHandler> = HashMap::new();
    let mut input_handlers: HashMap<String, crate::builders::InputHandler> = HashMap::new();
    let mut pane_lifecycle = HashMap::new();
    let mut pane_collections = HashMap::new();
    let mut app_events = HashMap::new();

    // Register global action handlers. Actions are the only global executable
    // surface; pane queries and mutations are registered separately below.
    for action in &addon.actions {
        if let Some(ref handler) = action.handler {
            let handler = handler.clone();
            actions.insert(action.id.clone(), handler);
        }
    }

    // Register app event handlers
    for event in &addon.app_events {
        app_events.insert(event.event_type.clone(), event.handler.clone());
    }

    // Register on_startup handler
    if let Some(startup_handler) = addon.on_startup {
        app_events.insert("startup".to_string(), startup_handler);
    }

    // Register pane queries, mutations, and lifecycle
    for pane in &addon.panes {
        for operation in &pane.queries {
            queries.insert(
                format!("{}.{}", pane.name, operation.name),
                operation.handler.clone(),
            );
        }
        for operation in &pane.mutations {
            mutations.insert(
                format!("{}.{}", pane.name, operation.name),
                operation.handler.clone(),
            );
        }
        if let Some(handler) = &pane.input_handler {
            input_handlers.insert(pane.name.clone(), handler.clone());
        }
        if let Some(ref collection) = pane.collection {
            pane_collections.insert(pane.name.clone(), collection.clone());
        }
        pane_lifecycle.insert(
            pane.name.clone(),
            PaneLifecycle {
                on_create: pane.on_create.clone(),
                on_destroy: pane.on_destroy.clone(),
                on_resize: pane.on_resize.clone(),
                subscribe_output: pane.subscribe_output.clone(),
            },
        );
    }

    let server = Arc::new(JsonServer {
        queries,
        mutations,
        actions,
        input_handlers,
        pane_lifecycle,
        pane_collections,
        app_events,
        ctx,
        panes: Arc::new(Mutex::new(HashMap::new())),
        log_level_handle,
    });

    (server, addon.name)
}

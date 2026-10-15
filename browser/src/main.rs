mod pane;
mod proxy;

use std::sync::Arc;

use zem_addon_sdk::{init, Addon};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let proxy = Arc::new(proxy::ProxyClient::new());

    // Start the embedded HTTP proxy on an ephemeral localhost port. Must run
    // before `init()` blocks on the addon's Unix socket accept loop. The pane
    // discovers the port via the `get_proxy_info` query.
    let proxy_task_proxy = proxy.clone();
    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("browser proxy failed to bind: {e}");
                return;
            }
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(e) => {
                tracing::error!("browser proxy local_addr failed: {e}");
                return;
            }
        };
        proxy_task_proxy.set_port(port);
        tracing::info!("browser proxy listening on 127.0.0.1:{port}");

        let app = proxy::router(proxy_task_proxy.clone());
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("browser proxy server error: {e}");
        }
    });

    let addon = build_addon(proxy);
    init(addon).await
}

fn build_addon(proxy: Arc<proxy::ProxyClient>) -> Addon {
    Addon::new()
        .id("com.rendinjast.browser")
        .name("browser")
        .version("1.0.0")
        .description("Browser webview pane with history, bookmarks and devtools")
        .author("zem")
        .collection("browser", "Browsers")
        .pane(pane::browser_pane(proxy.clone()))
        .action(pane::open_url_action())
        .action(pane::new_pane_action())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zem_addon_sdk::manifest_generator;

    #[test]
    fn manifest_contains_browser_declaration() {
        let proxy = Arc::new(proxy::ProxyClient::new());
        let toml = manifest_generator(&build_addon(proxy));

        assert!(toml.contains("id               = \"com.rendinjast.browser\""));
        assert!(toml.contains("[[collection]]\nid = \"browser\""));
        assert!(toml.contains("[[action]]\nid = \"open_url\""));
        assert!(toml.contains("[[action]]\nid = \"browser_new\""));
        assert!(toml.contains("[[pane]]\nname = \"browser\""));
        assert!(toml.contains("[[pane.query]]\nname = \"get_state\""));
        assert!(toml.contains("[[pane.query]]\nname = \"get_proxy_info\""));
        assert!(toml.contains("[[pane.mutation]]\nname = \"add_bookmark\""));
        assert!(toml.contains("[[pane.mutation]]\nname = \"remove_bookmark\""));

        std::fs::write("/tmp/browser-manifest.toml", &toml).expect("write manifest for deployment");
    }
}

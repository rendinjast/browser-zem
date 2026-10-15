use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::context::AddonContext;
use zem_addon_protocol::{AddonError, OutputEvent};

// ---------------------------------------------------------------------------
// Type aliases (internal) — all take &AddonContext, never &mut
// ---------------------------------------------------------------------------

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Request context passed to pane queries and mutations.
#[derive(Debug)]
pub struct OperationRequest<'a> {
    pub pane_id: i32,
    pub pane: &'a str,
    /// The resolved project working directory for the pane (pane → tab → project).
    pub cwd: &'a str,
    /// The request payload parsed as JSON, or `null` when not valid JSON.
    pub payload: &'a serde_json::Value,
    /// The raw request payload bytes (for non-JSON payloads like terminal input).
    pub raw: &'a [u8],
}

/// Request context passed to a global action.
#[derive(Debug)]
pub struct ActionRequest<'a> {
    pub pane_id: i32,
    pub cwd: &'a str,
    pub payload: &'a serde_json::Value,
    pub raw: &'a [u8],
}

pub(crate) type OperationHandler = Arc<
    dyn Fn(&OperationRequest, &AddonContext) -> BoxFuture<Result<serde_json::Value, AddonError>>
        + Send
        + Sync,
>;
pub(crate) type ActionHandler = Arc<
    dyn Fn(&ActionRequest, &AddonContext) -> BoxFuture<Result<ActionExecuteResponse, AddonError>>
        + Send
        + Sync,
>;
pub(crate) type InputHandler = Arc<
    dyn Fn(&OperationRequest, &AddonContext) -> BoxFuture<Result<(), AddonError>> + Send + Sync,
>;

pub(crate) type AppEventHandler = Arc<
    dyn Fn(&serde_json::Value, &AddonContext) -> BoxFuture<Result<(), AddonError>> + Send + Sync,
>;

pub(crate) type OnCreate = Arc<
    dyn Fn(i32, &str, &serde_json::Value, &AddonContext) -> BoxFuture<Result<(), AddonError>>
        + Send
        + Sync,
>;
pub(crate) type OnDestroy =
    Arc<dyn Fn(i32, &str, &AddonContext) -> BoxFuture<Result<(), AddonError>> + Send + Sync>;
pub(crate) type OnResize = Arc<
    dyn Fn(i32, u16, u16, u16, u16, &AddonContext) -> BoxFuture<Result<(), AddonError>>
        + Send
        + Sync,
>;
pub(crate) type SubscribeOutput = Arc<
    dyn Fn(i32, &AddonContext) -> BoxFuture<Result<mpsc::Receiver<OutputEvent>, AddonError>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// OperationKind distinguishes read-only queries from mutating mutations.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Query => "query",
            OperationKind::Mutation => "mutation",
        }
    }
}

// ---------------------------------------------------------------------------
// Pane operation
// ---------------------------------------------------------------------------

/// An addon-declared collection registered on the server. Unlike a pane's
/// default collection, declaring it here does not make every pane of an addon
/// a member — membership is driven by rules or explicit assignment.
#[derive(Debug, Clone)]
pub struct AddonCollection {
    pub id: String,
    pub name: String,
}

impl AddonCollection {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
        }
    }
}

/// A named query or mutation exposed by a pane.
pub struct PaneOperation {
    pub name: String,
    pub kind: OperationKind,
    pub(crate) handler: OperationHandler,
}

impl PaneOperation {
    fn query<F, Fut>(name: &str, f: F) -> Self
    where
        F: Fn(&OperationRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AddonError>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            kind: OperationKind::Query,
            handler: Arc::new(move |req, ctx| Box::pin(f(req, ctx))),
        }
    }

    fn mutation<F, Fut>(name: &str, f: F) -> Self
    where
        F: Fn(&OperationRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AddonError>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            kind: OperationKind::Mutation,
            handler: Arc::new(move |req, ctx| Box::pin(f(req, ctx))),
        }
    }
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// A global app event handler.
pub struct Event {
    pub event_type: String,
    pub(crate) handler: AppEventHandler,
}

impl Event {
    /// Register a handler for a global app event (e.g. `cwd_changed`,
    /// `server_stopped`). The payload is deserialized to `T` automatically.
    pub fn on<T, F, Fut>(event_type: &str, f: F) -> Self
    where
        T: serde::de::DeserializeOwned + Send + 'static,
        F: Fn(T, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        let f = Arc::new(f);
        Self {
            event_type: event_type.to_string(),
            handler: Arc::new(move |payload, ctx| {
                let f = Arc::clone(&f);
                let parsed: Result<T, AddonError> = serde_json::from_value(payload.clone())
                    .map_err(|e| {
                        AddonError::invalid_payload(format!(
                            "failed to deserialize event payload: {}",
                            e
                        ))
                    });
                match parsed {
                    Ok(payload) => Box::pin(f(payload, ctx)),
                    Err(e) => Box::pin(async move { Err(e) }),
                }
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Pane
// ---------------------------------------------------------------------------

/// A pane type that can be created in a project.
pub struct Pane {
    pub name: String,
    /// Default icon shown for new panes of this kind, in the same format the
    /// client renders (e.g. `nerd:terminal`, `nerd:note-sticky`, `emoji:📝`).
    /// The host stores it on the pane when it is created without an explicit
    /// icon, so the icon survives without the addon manifest. Empty means the
    /// pane has no default icon.
    pub icon: String,
    pub collection: Option<String>,
    pub queries: Vec<PaneOperation>,
    pub mutations: Vec<PaneOperation>,
    pub(crate) input_handler: Option<InputHandler>,
    pub(crate) on_create: Option<OnCreate>,
    pub(crate) on_destroy: Option<OnDestroy>,
    pub(crate) on_resize: Option<OnResize>,
    pub(crate) subscribe_output: Option<SubscribeOutput>,
}

impl Pane {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            icon: String::new(),
            collection: None,
            queries: Vec::new(),
            mutations: Vec::new(),
            input_handler: None,
            on_create: None,
            on_destroy: None,
            on_resize: None,
            subscribe_output: None,
        }
    }

    /// Set the default icon for new panes of this kind, using the client's
    /// icon format (e.g. `nerd:terminal`, `nerd:folder-open`, `emoji:🗂️`).
    /// The host applies it to panes created without an explicit icon.
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_string();
        self
    }

    pub fn collection(mut self, collection: &str) -> Self {
        self.collection = Some(collection.to_string());
        self
    }

    /// Add a read-only query operation to this pane.
    pub fn query<F, Fut>(mut self, name: &str, f: F) -> Self
    where
        F: Fn(&OperationRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AddonError>> + Send + 'static,
    {
        self.queries.push(PaneOperation::query(name, f));
        self
    }

    /// Register the private input sink used by host-delivered pane input.
    /// Input is a transport concern, not a user-visible pane mutation.
    pub fn on_input<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&OperationRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        self.input_handler = Some(Arc::new(move |req, ctx| Box::pin(f(req, ctx))));
        self
    }

    /// Add a mutating operation to this pane.
    pub fn mutation<F, Fut>(mut self, name: &str, f: F) -> Self
    where
        F: Fn(&OperationRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AddonError>> + Send + 'static,
    {
        self.mutations.push(PaneOperation::mutation(name, f));
        self
    }

    pub fn on_create<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(i32, &str, &serde_json::Value, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        self.on_create = Some(Arc::new(move |pane_id, cwd, extra, ctx| {
            Box::pin(f(pane_id, cwd, extra, ctx))
        }));
        self
    }

    pub fn on_destroy<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(i32, &str, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        self.on_destroy = Some(Arc::new(move |pane_id, reason, ctx| {
            Box::pin(f(pane_id, reason, ctx))
        }));
        self
    }

    pub fn on_resize<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(i32, u16, u16, u16, u16, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        self.on_resize = Some(Arc::new(
            move |pane_id, cols, rows, width_px, height_px, ctx| {
                Box::pin(f(pane_id, cols, rows, width_px, height_px, ctx))
            },
        ));
        self
    }

    pub fn subscribe_output<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(i32, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<mpsc::Receiver<OutputEvent>, AddonError>> + Send + 'static,
    {
        self.subscribe_output = Some(Arc::new(move |pane_id, ctx| Box::pin(f(pane_id, ctx))));
        self
    }
}

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

/// How the host resolves an `open_pane` action: focus an existing pane of the
/// same kind, always create a new one, or (the default) match by params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneBehavior {
    /// Focus the first existing pane of this kind. If none exists, open a new
    /// one. Use for actions like version-control that should reuse their pane.
    FocusExisting,
    /// Always create a new pane — never reuse an existing one, even if params
    /// match. Use for actions like "open note" where each note gets its own pane.
    ForceCreate,
}

/// The strictly typed response of an [`Action::on_execute`] handler.
///
/// Actions do not return arbitrary data — the response surface is fixed so the
/// host and client always know exactly what an action can affect. Unknown or
/// malformed fields are rejected at the SDK boundary.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ActionExecuteResponse {
    /// When set, the client opens a new pane of the given kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_pane: Option<String>,
    /// Optional JSON payload passed to the newly opened pane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// How the host should handle an existing pane of this kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_behavior: Option<PaneBehavior>,
}

impl ActionExecuteResponse {
    /// An action response that opens a new pane of the given kind.
    pub fn open_pane(kind: &str) -> Self {
        Self {
            open_pane: Some(kind.to_string()),
            params: None,
            pane_behavior: None,
        }
    }

    /// An action response that opens a pane with a JSON-serializable payload.
    pub fn open_pane_with_params(kind: &str, params: serde_json::Value) -> Self {
        Self {
            open_pane: Some(kind.to_string()),
            params: Some(params),
            pane_behavior: None,
        }
    }

    /// Reuse the first existing pane of this kind; create only if none exists.
    pub fn focus_existing(mut self) -> Self {
        self.pane_behavior = Some(PaneBehavior::FocusExisting);
        self
    }

    /// Always create a new pane, never reuse an existing one.
    pub fn force_create(mut self) -> Self {
        self.pane_behavior = Some(PaneBehavior::ForceCreate);
        self
    }
}

/// An action item that appears in the addon's status bar, tab bar, or sidebar.
/// Can be a simple status indicator or a command button with a handler.
pub struct Action {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub description: String,
    /// Whether executing this global UI action requires workspace write access.
    pub requires_write: bool,
    /// Default placement when the action is first enabled (e.g. `sidebar`,
    /// `statusbar-left`, `tabbar`). Empty means the action is hidden until the
    /// user explicitly places it. The user can always move or hide it later.
    pub location: String,
    pub(crate) handler: Option<ActionHandler>,
}

impl Action {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            title: String::new(),
            icon: String::new(),
            description: String::new(),
            requires_write: true,
            location: String::new(),
            handler: None,
        }
    }

    /// Mark this global action as safe for read-only workspace viewers.
    pub fn read_only(mut self) -> Self {
        self.requires_write = false;
        self
    }

    /// Require write access when this global action is executed (the default).
    pub fn requires_write(mut self, requires_write: bool) -> Self {
        self.requires_write = requires_write;
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_string();
        self
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.description = desc.to_string();
        self
    }

    /// Set the default location where this action should appear when first
    /// enabled (e.g. `"sidebar"`, `"statusbar-left"`, `"statusbar-right"`,
    /// `"tabbar"`). The user can move or hide it afterwards.
    pub fn location(mut self, location: &str) -> Self {
        self.location = location.to_string();
        self
    }

    pub fn on_execute<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&ActionRequest, &AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ActionExecuteResponse, AddonError>> + Send + 'static,
    {
        self.handler = Some(Arc::new(move |req, ctx| Box::pin(f(req, ctx))));
        self
    }
}

// ---------------------------------------------------------------------------
// ConfigField — declarative addon settings schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ConfigField {
    pub name: String,
    pub label: String,
    pub field_type: String,
    pub default: String,
    pub description: String,
    pub choices: Vec<ConfigFieldChoice>,
}

#[derive(Debug, Clone)]
pub struct ConfigFieldChoice {
    pub value: String,
    pub label: String,
}

impl ConfigField {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            label: name.to_string(),
            field_type: "string".to_string(),
            default: String::new(),
            description: String::new(),
            choices: Vec::new(),
        }
    }

    pub fn label(mut self, label: &str) -> Self {
        self.label = label.to_string();
        self
    }

    pub fn field_type(mut self, t: &str) -> Self {
        self.field_type = t.to_string();
        self
    }

    pub fn default(mut self, d: &str) -> Self {
        self.default = d.to_string();
        self
    }

    pub fn description(mut self, d: &str) -> Self {
        self.description = d.to_string();
        self
    }

    pub fn choice(mut self, value: &str, label: &str) -> Self {
        self.choices.push(ConfigFieldChoice {
            value: value.to_string(),
            label: label.to_string(),
        });
        self
    }
}

// ---------------------------------------------------------------------------
// Addon
// ---------------------------------------------------------------------------

/// The main addon definition.
pub struct Addon {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub min_zem_version: String,
    pub platforms: Vec<String>,
    pub panes: Vec<Pane>,
    pub collections: Vec<AddonCollection>,
    pub app_events: Vec<Event>,
    pub actions: Vec<Action>,
    pub config_fields: Vec<ConfigField>,
    pub on_startup: Option<AppEventHandler>,
}

impl Default for Addon {
    fn default() -> Self {
        Self::new()
    }
}

impl Addon {
    pub fn new() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
            author: String::new(),
            min_zem_version: "0.1.0".to_string(),
            platforms: vec!["macos".into(), "linux".into()],
            panes: Vec::new(),
            collections: Vec::new(),
            app_events: Vec::new(),
            actions: Vec::new(),
            config_fields: Vec::new(),
            on_startup: None,
        }
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }
    pub fn version(mut self, v: &str) -> Self {
        self.version = v.to_string();
        self
    }
    pub fn description(mut self, d: &str) -> Self {
        self.description = d.to_string();
        self
    }
    pub fn author(mut self, a: &str) -> Self {
        self.author = a.to_string();
        self
    }
    pub fn min_zem_version(mut self, v: &str) -> Self {
        self.min_zem_version = v.to_string();
        self
    }
    pub fn platform(mut self, p: &str) -> Self {
        self.platforms.push(p.to_string());
        self
    }
    pub fn pane(mut self, pane: Pane) -> Self {
        self.panes.push(pane);
        self
    }

    /// Declare an addon-managed collection. The collection is registered on
    /// the server under `<addon_id>:<id>` but does NOT default panes into it.
    pub fn collection(mut self, id: &str, name: &str) -> Self {
        self.collections.push(AddonCollection::new(id, name));
        self
    }
    pub fn event(mut self, event: Event) -> Self {
        self.app_events.push(event);
        self
    }
    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }
    pub fn config(mut self, field: ConfigField) -> Self {
        self.config_fields.push(field);
        self
    }

    /// Declare several config fields at once (e.g. a generated settings set).
    pub fn configs(mut self, fields: Vec<ConfigField>) -> Self {
        self.config_fields.extend(fields);
        self
    }

    /// Register a startup handler that runs once when the addon initializes.
    pub fn on_startup<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&AddonContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AddonError>> + Send + 'static,
    {
        self.on_startup = Some(Arc::new(move |_payload, ctx| Box::pin(f(ctx))));
        self
    }
}

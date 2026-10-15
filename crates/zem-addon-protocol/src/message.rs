//! Wire types for the JSON-over-Unix-socket protocol shared by the zem host,
//! the addon SDK, and addon processes.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error codes
//
// Host-owned codes 1-9 are reserved for the zem host and SDK. Add-ons author
// their own codes starting at `ERR_ADDON_BASE` (100). The client can switch on
// a code programmatically without having to parse free-form messages.
// ---------------------------------------------------------------------------

pub const ERR_OK: u32 = 0;
pub const ERR_INTERNAL: u32 = 1;
pub const ERR_HANDLER_PANIC: u32 = 2;
pub const ERR_UNKNOWN_OPERATION: u32 = 3;
pub const ERR_UNKNOWN_METHOD: u32 = 4;
pub const ERR_INVALID_PAYLOAD: u32 = 5;
pub const ERR_NOT_SUPPORTED: u32 = 6;
pub const ERR_ADDON_PROCESS_DEAD: u32 = 7;
pub const ERR_TIMEOUT: u32 = 8;
pub const ERR_PERMISSION_DENIED: u32 = 9;
/// Errors emitted by add-on code start here.
pub const ERR_ADDON_BASE: u32 = 100;

/// A structured error with a numeric code and a human-readable message.
///
/// Replaces the free-form string error so clients can react to failures
/// programmatically instead of parsing message text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddonError {
    pub code: u32,
    pub message: String,
}

impl AddonError {
    /// Build an error with the given code. `code` should be a host-owned
    /// constant or `ERR_ADDON_BASE + n` for add-on-specific errors.
    pub fn code(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::code(ERR_INTERNAL, message)
    }

    pub fn handler_panic() -> Self {
        Self::code(ERR_HANDLER_PANIC, "addon handler panicked")
    }

    pub fn unknown_operation(name: &str) -> Self {
        Self::code(
            ERR_UNKNOWN_OPERATION,
            format!("unknown pane operation: {}", name),
        )
    }

    pub fn unknown_action(name: &str) -> Self {
        Self::code(
            ERR_UNKNOWN_OPERATION,
            format!("unknown global action: {}", name),
        )
    }

    pub fn invalid_payload(message: impl Into<String>) -> Self {
        Self::code(ERR_INVALID_PAYLOAD, message)
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::code(ERR_NOT_SUPPORTED, message)
    }

    pub fn process_dead(message: impl Into<String>) -> Self {
        Self::code(ERR_ADDON_PROCESS_DEAD, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::code(ERR_TIMEOUT, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::code(ERR_PERMISSION_DENIED, message)
    }
}

impl std::fmt::Display for AddonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for AddonError {}

/// Add-on-authored errors default to the add-on code range (`ERR_ADDON_BASE`).
/// Use `AddonError::code(...)` for a specific code.
impl From<String> for AddonError {
    fn from(message: String) -> Self {
        AddonError::code(ERR_ADDON_BASE, message)
    }
}

impl From<&str> for AddonError {
    fn from(message: &str) -> Self {
        AddonError::code(ERR_ADDON_BASE, message)
    }
}

/// A single line-delimited JSON message exchanged over the addon sockets.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AddonError>,
}

impl JsonMessage {
    pub fn request(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            id: Some(id),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn response(id: Option<u64>, result: serde_json::Value) -> Self {
        Self {
            id,
            method: None,
            params: None,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<u64>, error: AddonError) -> Self {
        Self {
            id,
            method: None,
            params: None,
            result: None,
            error: Some(error),
        }
    }

    /// An unsolicited server-push message (e.g. `output_event`).
    pub fn event(method: &str, params: serde_json::Value) -> Self {
        Self {
            id: None,
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        }
    }
}

/// Output event streamed from an addon pane (e.g. terminal output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputEvent {
    pub data: Vec<u8>,
    pub exited: bool,
}

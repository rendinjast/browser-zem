//! Shared JSON-over-Unix-socket protocol for zem addons.
//!
//! Defines the wire types, framing, and a request/response client used by the
//! zem host, the addon SDK, and addon processes so the protocol is implemented
//! exactly once.

pub mod client;
pub mod framing;
pub mod message;

pub use client::JsonClient;
pub use framing::{read_message, read_output_event, write_message, write_output_event};
pub use message::{
    AddonError, JsonMessage, OutputEvent, ERR_ADDON_BASE, ERR_ADDON_PROCESS_DEAD,
    ERR_HANDLER_PANIC, ERR_INTERNAL, ERR_INVALID_PAYLOAD, ERR_NOT_SUPPORTED, ERR_OK,
    ERR_PERMISSION_DENIED, ERR_TIMEOUT, ERR_UNKNOWN_METHOD, ERR_UNKNOWN_OPERATION,
};

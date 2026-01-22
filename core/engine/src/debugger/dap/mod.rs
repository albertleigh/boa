//! Debug Adapter Protocol (DAP) implementation for Boa
//!
//! This module implements the Debug Adapter Protocol specification to enable
//! debugging Boa JavaScript code from IDEs like VS Code.
//!
//! # Architecture
//!
//! The DAP implementation consists of:
//! - Protocol types and messages (requests, responses, events)
//! - A DAP server that communicates via JSON-RPC
//! - Integration with Boa's debugger API
//! - Support for breakpoints, stepping, variable inspection
//!
//! # References
//!
//! - [DAP Specification](https://microsoft.github.io/debug-adapter-protocol/)
//! - [VS Code Debug Extension Guide](https://code.visualstudio.com/api/extension-guides/debugger-extension)

pub mod eval_context;
pub mod messages;
pub mod server;
pub mod session;

pub use messages::*;
pub use server::DapServer;
pub use session::DebugSession;
pub use eval_context::DebugEvent;

use serde::{Deserialize, Serialize};

/// DAP protocol message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProtocolMessage {
    #[serde(rename = "request")]
    Request(Request),
    #[serde(rename = "response")]
    Response(Response),
    #[serde(rename = "event")]
    Event(Event),
}

/// DAP request message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub seq: i64,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// DAP response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub seq: i64,
    pub request_seq: i64,
    pub success: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// DAP event message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

impl ProtocolMessage {
    pub fn seq(&self) -> i64 {
        match self {
            Self::Request(r) => r.seq,
            Self::Response(r) => r.seq,
            Self::Event(e) => e.seq,
        }
    }
}

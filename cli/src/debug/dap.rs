//! DAP debugger for Boa CLI
//!
//! This module provides the Debug Adapter Protocol integration for the Boa CLI.
//! It uses the DapServer from boa_engine to handle all protocol communication.

use boa_engine::{
    JsResult,
    debugger::{
        Debugger,
        dap::{DapServer, session::DebugSession},
    },
    js_error,
};
use std::env;
use std::sync::{Arc, Mutex};

/// Runs the DAP server over stdio
///
/// This creates a debugger instance, wraps it in a DebugSession,
/// and runs the DapServer to handle all protocol communication.
/// The DapServer in boa_engine handles all DAP messages, breakpoints,
/// stepping, variable inspection, etc.
///
/// Set BOA_DAP_DEBUG=1 environment variable to enable debug logging.
pub fn run_dap_server() -> JsResult<()> {
    eprintln!("[DAP] Starting Boa Debug Adapter");

    // Create the debugger instance
    let debugger = Arc::new(Mutex::new(Debugger::new()));

    // Create a debug session that manages the debugger state
    let session = Arc::new(Mutex::new(DebugSession::new(debugger.clone())));

    // Create and run the DAP server (handles all protocol communication)
    let mut server = DapServer::new(session);

    server
        .run()
        .map_err(|e| js_error!("DAP server error: {}", e))?;

    eprintln!("[DAP] Server stopped");
    Ok(())
}

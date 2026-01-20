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
use std::io;
use std::sync::{Arc, Mutex};

/// Transport mode for the DAP server
pub enum DapTransportMode {
    /// Standard input/output (default)
    Stdio,
    /// HTTP server on specified port
    Http(u16),
}

/// Runs the DAP server with specified transport mode
///
/// This creates a debugger instance, wraps it in a DebugSession,
/// and runs the DapServer to handle all protocol communication.
/// The DapServer in boa_engine handles all DAP messages, breakpoints,
/// stepping, variable inspection, etc.
///
/// Set BOA_DAP_DEBUG=1 environment variable to enable debug logging.
pub fn run_dap_server_with_mode(mode: DapTransportMode) -> JsResult<()> {
    let mode_str = match &mode {
        DapTransportMode::Stdio => "stdio".to_string(),
        DapTransportMode::Http(port) => format!("HTTP on port {}", port),
    };

    eprintln!("[DAP] Starting Boa Debug Adapter ({})", mode_str);

    // Check if debug mode is enabled via environment variable
    let debug = env::var("BOA_DAP_DEBUG")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if debug {
        eprintln!("[DAP] Debug logging enabled");
    }

    // Create the debugger instance
    let debugger = Arc::new(Mutex::new(Debugger::new()));

    // Create a debug session that manages the debugger state
    let session = Arc::new(Mutex::new(DebugSession::new(debugger.clone())));

    match mode {
        DapTransportMode::Stdio => {
            // Create and run the DAP server over stdio
            let mut server = DapServer::with_debug(session, debug);
            server
                .run()
                .map_err(|e| js_error!("DAP server error: {}", e))?;
        }
        DapTransportMode::Http(port) => {
            // Run HTTP server
            run_http_server(session, port, debug)
                .map_err(|e| js_error!("HTTP server error: {}", e))?;
        }
    }

    eprintln!("[DAP] Server stopped");
    Ok(())
}

/// Runs the DAP server over stdio (default mode)
pub fn run_dap_server() -> JsResult<()> {
    run_dap_server_with_mode(DapTransportMode::Stdio)
}

/// Runs the DAP server as an HTTP server
fn run_http_server(session: Arc<Mutex<DebugSession>>, port: u16, debug: bool) -> io::Result<()> {
    use tiny_http::{Method, Response, Server};

    let addr = format!("127.0.0.1:{}", port);
    eprintln!("[DAP-HTTP] Starting HTTP server on {}", addr);

    let server = Server::http(&addr).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to start HTTP server: {}", e),
        )
    })?;

    eprintln!("[DAP-HTTP] Server listening on http://{}", addr);
    eprintln!("[DAP-HTTP] Ready to accept connections");

    let mut dap_server = DapServer::with_debug(session, debug);

    for mut request in server.incoming_requests() {
        if debug {
            eprintln!(
                "[DAP-HTTP] Received {} request to {}",
                request.method(),
                request.url()
            );
        }

        match request.method() {
            Method::Post => {
                // Read the request body
                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body).ok();

                if debug {
                    if let Ok(body_str) = String::from_utf8(body.clone()) {
                        eprintln!("[DAP-HTTP] Request body: {}", body_str);
                    }
                }

                // Parse DAP message
                let response_json = match serde_json::from_slice::<
                    boa_engine::debugger::dap::ProtocolMessage,
                >(&body)
                {
                    Ok(boa_engine::debugger::dap::ProtocolMessage::Request(dap_request)) => {
                        // Process the request using DapServer's handle_request method
                        let responses = dap_server.handle_request(dap_request);

                        // For HTTP, we only return the first response (typically the direct response to the request)
                        // Events would need to be sent separately via WebSocket or polling
                        if let Some(first_response) = responses.first() {
                            serde_json::to_string(first_response)
                                .unwrap_or_else(|_| "{}".to_string())
                        } else {
                            "{}".to_string()
                        }
                    }
                    Err(e) => {
                        eprintln!("[DAP-HTTP] Failed to parse request: {}", e);
                        format!("{{\"error\": \"Failed to parse request: {}\"}}", e)
                    }
                    _ => {
                        eprintln!("[DAP-HTTP] Unexpected message type (not a request)");
                        "{\"error\": \"Expected a request message\"}".to_string()
                    }
                };

                if debug {
                    eprintln!("[DAP-HTTP] Response: {}", response_json);
                }

                // Send response
                let response = Response::from_string(response_json)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Access-Control-Allow-Origin"[..],
                            &b"*"[..],
                        )
                        .unwrap(),
                    );

                if let Err(e) = request.respond(response) {
                    eprintln!("[DAP-HTTP] Failed to send response: {}", e);
                }
            }
            Method::Options => {
                // Handle CORS preflight
                let response = Response::from_string("")
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Access-Control-Allow-Origin"[..],
                            &b"*"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Access-Control-Allow-Methods"[..],
                            &b"POST, OPTIONS"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Access-Control-Allow-Headers"[..],
                            &b"Content-Type"[..],
                        )
                        .unwrap(),
                    );

                if let Err(e) = request.respond(response) {
                    eprintln!("[DAP-HTTP] Failed to send CORS response: {}", e);
                }
            }
            _ => {
                let response = Response::from_string("Method not allowed").with_status_code(405);
                let _ = request.respond(response);
            }
        }
    }

    Ok(())
}

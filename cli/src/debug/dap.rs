//! DAP debugger for Boa CLI
//!
//! This module provides the Debug Adapter Protocol integration for the Boa CLI.
//! It intercepts DAP messages, manages the JavaScript context with runtime,
//! and handles execution and output capture.

use boa_engine::{
    Context, JsResult, JsValue, Source,
    context::ContextBuilder,
    debugger::{
        Debugger,
        dap::{DapServer, session::DebugSession, ProtocolMessage, Request, Event, messages::*},
    },
    js_error,
    property::Attribute,
};
use boa_runtime::console::{Console, ConsoleState, Logger};
use boa_gc::{Finalize, Trace};
use std::env;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::fs;

/// Transport mode for the DAP server
pub enum DapTransportMode {
    /// Standard input/output (default)
    Stdio,
    /// TCP server on specified port
    Tcp(u16),
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
        DapTransportMode::Tcp(port) => format!("TCP on port {}", port),
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
        DapTransportMode::Tcp(port) => {
            // Run TCP server
            run_tcp_server(session, port, debug)
                .map_err(|e| js_error!("TCP server error: {}", e))?;
        }
    }

    eprintln!("[DAP] Server stopped");
    Ok(())
}

/// Runs the DAP server over stdio (default mode)
pub fn run_dap_server() -> JsResult<()> {
    run_dap_server_with_mode(DapTransportMode::Stdio)
}

/// Runs the DAP server as a TCP server (raw socket, not HTTP)
fn run_tcp_server(session: Arc<Mutex<DebugSession>>, port: u16, debug: bool) -> io::Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    let addr = format!("127.0.0.1:{}", port);
    eprintln!("[BOA-DAP] Starting TCP server on {}", addr);

    let listener = TcpListener::bind(&addr)?;
    eprintln!("[BOA-DAP] Server listening on {}", addr);
    eprintln!("[BOA-DAP] Ready to accept connections");

    // Accept connections in a loop
    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                eprintln!("[BOA-DAP] Client connected from {}", peer_addr);
                
                // Handle this client connection
                if let Err(e) = handle_tcp_client(stream, session.clone(), debug) {
                    eprintln!("[BOA-DAP] Client handler error: {}", e);
                    // Continue accepting new connections even if one fails
                    continue;
                }
                
                // After handling one successful session, exit
                // (DAP typically uses one connection per debug session)
                break;
            }
            Err(e) => {
                eprintln!("[BOA-DAP] Error accepting connection: {}", e);
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Custom Logger that sends console output directly as DAP output events
#[derive(Clone, Trace, Finalize)]
struct DapLogger<W: Write + 'static> {
    /// TCP writer for sending DAP messages
    #[unsafe_ignore_trace]
    writer: Arc<Mutex<W>>,
    
    /// Sequence counter for DAP messages
    #[unsafe_ignore_trace]
    seq_counter: Arc<Mutex<i64>>,
    
    /// Debug flag
    #[unsafe_ignore_trace]
    debug: bool,
}

impl<W: Write + 'static> DapLogger<W> {
    fn new(writer: Arc<Mutex<W>>, seq_counter: Arc<Mutex<i64>>, debug: bool) -> Self {
        Self {
            writer,
            seq_counter,
            debug,
        }
    }
    
    fn send_output(&self, msg: String, category: &str) {
        // Create output event
        let seq = {
            let mut counter = self.seq_counter.lock().unwrap();
            let current = *counter;
            *counter += 1;
            current
        };
        
        let output_event = Event {
            seq,
            event: "output".to_string(),
            body: Some(serde_json::to_value(OutputEventBody {
                category: Some(category.to_string()),
                output: msg + "\n",
                group: None,
                variables_reference: None,
                source: None,
                line: None,
                column: None,
                data: None,
            }).unwrap()),
        };
        
        let output_message = ProtocolMessage::Event(output_event);
        
        // Send immediately to TCP stream
        if let Ok(mut writer) = self.writer.lock() {
            let _ = send_message_internal(&output_message, &mut *writer, self.debug);
        }
    }
}

impl<W: Write + 'static> Logger for DapLogger<W> {
    fn log(&self, msg: String, _state: &ConsoleState, _context: &mut Context) -> JsResult<()> {
        self.send_output(msg, "stdout");
        Ok(())
    }

    fn info(&self, msg: String, _state: &ConsoleState, _context: &mut Context) -> JsResult<()> {
        self.send_output(msg, "stdout");
        Ok(())
    }

    fn warn(&self, msg: String, _state: &ConsoleState, _context: &mut Context) -> JsResult<()> {
        self.send_output(msg, "console");
        Ok(())
    }

    fn error(&self, msg: String, _state: &ConsoleState, _context: &mut Context) -> JsResult<()> {
        self.send_output(msg, "stderr");
        Ok(())
    }
}

/// Internal function to send a DAP message (used by logger)
fn send_message_internal<W: Write>(message: &ProtocolMessage, writer: &mut W, debug: bool) -> io::Result<()> {
    let json = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());
    
    if debug {
        eprintln!("[BOA-DAP] Output Event: {}", json);
    }

    write!(writer, "Content-Length: {}\r\n\r\n{}", json.len(), json)?;
    writer.flush()?;
    Ok(())
}

/// Handle a single TCP client connection using DAP protocol
/// This intercepts launch requests to handle execution and output capture at the CLI level
fn handle_tcp_client(
    stream: std::net::TcpStream,
    session: Arc<Mutex<DebugSession>>,
    debug: bool,
) -> io::Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};

    let mut reader = BufReader::new(stream.try_clone()?);
    let writer = Arc::new(Mutex::new(stream));

    let mut dap_server = DapServer::with_debug(session.clone(), debug);
    
    // Sequence counter shared with logger for creating events
    let seq_counter = Arc::new(Mutex::new(100i64)); // Start at 100 to avoid conflicts with server seq
    
    // Create context with runtime and custom logger that sends output immediately
    let mut context = create_context_with_logger(writer.clone(), seq_counter, debug);

    loop {
        // Read the Content-Length header
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => {
                eprintln!("[BOA-DAP] Client disconnected");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[BOA-DAP] Error reading header: {}", e);
                break;
            }
        }

        if header.trim().is_empty() {
            continue;
        }

        let content_length: usize = match header
            .trim()
            .strip_prefix("Content-Length: ")
            .and_then(|s| s.parse().ok())
        {
            Some(len) => len,
            None => {
                eprintln!("[BOA-DAP] Invalid Content-Length header: {}", header);
                continue;
            }
        };

        // Read the empty line separator
        let mut empty = String::new();
        reader.read_line(&mut empty)?;

        // Read the message body
        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer)?;

        if debug {
            if let Ok(body_str) = String::from_utf8(buffer.clone()) {
                eprintln!("[BOA-DAP] Request: {}", body_str);
            }
        }

        // Parse DAP message
        match serde_json::from_slice::<ProtocolMessage>(&buffer) {
            Ok(ProtocolMessage::Request(dap_request)) => {
                // Check if this is a launch request - handle it specially
                if dap_request.command == "launch" {
                    // Handle launch at CLI level
                    let responses = handle_launch_request(
                        dap_request,
                        &mut dap_server,
                        &mut context,
                        writer.clone(),
                        debug,
                    )?;
                    
                    // Send all responses
                    for response in responses {
                        let mut w = writer.lock().unwrap();
                        send_dap_message(&response, &mut *w, debug)?;
                    }
                } else {
                    // Process other requests normally through the server
                    let responses = dap_server.handle_request(dap_request);

                    // Send all responses
                    for response in responses {
                        let mut w = writer.lock().unwrap();
                        send_dap_message(&response, &mut *w, debug)?;
                    }
                }
            }
            Err(e) => {
                eprintln!("[BOA-DAP] Failed to parse request: {}", e);
            }
            _ => {
                eprintln!("[BOA-DAP] Unexpected message type (not a request)");
            }
        }
    }

    Ok(())
}

/// Send a DAP protocol message
fn send_dap_message<W: io::Write>(message: &ProtocolMessage, writer: &mut W, debug: bool) -> io::Result<()> {
    let json = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());
    
    if debug {
        eprintln!("[BOA-DAP] Response: {}", json);
    }

    // Write with Content-Length header
    write!(writer, "Content-Length: {}\r\n\r\n{}", json.len(), json)?;
    writer.flush()?;
    Ok(())
}

/// Handle launch request: execute JS and capture output
fn handle_launch_request<W: Write + 'static>(
    mut request: Request,
    dap_server: &mut DapServer,
    context: &mut Context,
    writer: Arc<Mutex<W>>,
    debug: bool,
) -> io::Result<Vec<ProtocolMessage>> {
    // Parse launch arguments to get the program path
    let launch_args: LaunchRequestArguments = serde_json::from_value(
        request.arguments.clone().unwrap_or(serde_json::Value::Null)
    ).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    
    // First, let the DAP server handle the launch request (validates args, etc.)
    let responses = dap_server.handle_request(request);
    
    // Execute the program if specified
    if let Some(program_path) = launch_args.program {
        eprintln!("[DAP-CLI] Executing: {}", program_path);
        
        // Read the file
        match fs::read_to_string(&program_path) {
            Ok(source_code) => {
                // Execute the script - console output is sent immediately via DapLogger
                match context.eval(Source::from_bytes(&source_code)) {
                    Ok(result) => {
                        // Send the result if not undefined
                        if !result.is_undefined() {
                            if let Ok(result_str) = result.to_string(context) {
                                let output_event = dap_server.create_event(
                                    "output",
                                    Some(serde_json::to_value(OutputEventBody {
                                        category: Some("stdout".to_string()),
                                        output: result_str.to_std_string_escaped() + "\n",
                                        group: None,
                                        variables_reference: None,
                                        source: None,
                                        line: None,
                                        column: None,
                                        data: None,
                                    }).unwrap())
                                );
                                
                                let mut w = writer.lock().unwrap();
                                send_dap_message(&output_event, &mut *w, debug)?;
                            }
                        }
                    }
                    Err(e) => {
                        // Send error output
                        let error_event = dap_server.create_event(
                            "output",
                            Some(serde_json::to_value(OutputEventBody {
                                category: Some("stderr".to_string()),
                                output: format!("Error: {}\n", e.to_string()),
                                group: None,
                                variables_reference: None,
                                source: None,
                                line: None,
                                column: None,
                                data: None,
                            }).unwrap())
                        );
                        
                        let mut w = writer.lock().unwrap();
                        send_dap_message(&error_event, &mut *w, debug)?;
                    }
                }
            }
            Err(e) => {
                // Send file read error
                let error_event = dap_server.create_event(
                    "output",
                    Some(serde_json::to_value(OutputEventBody {
                        category: Some("stderr".to_string()),
                        output: format!("Failed to read file: {}\n", e),
                        group: None,
                        variables_reference: None,
                        source: None,
                        line: None,
                        column: None,
                        data: None,
                    }).unwrap())
                );
                
                let mut w = writer.lock().unwrap();
                send_dap_message(&error_event, &mut *w, debug)?;
            }
        }
        
        // Send terminated event
        let terminated_event = dap_server.create_event(
            "terminated",
            Some(serde_json::to_value(TerminatedEventBody {
                restart: None,
            }).unwrap())
        );
        
        let mut w = writer.lock().unwrap();
        send_dap_message(&terminated_event, &mut *w, debug)?;
    }
    
    Ok(responses)
}

/// Create a Context with runtime and custom DAP logger
fn create_context_with_logger<W: Write + 'static>(
    writer: Arc<Mutex<W>>,
    seq_counter: Arc<Mutex<i64>>,
    debug: bool,
) -> Context {
    let mut context = ContextBuilder::new().build().expect("Failed to create context");

    // Add console support with custom logger that sends output immediately
    let logger = DapLogger::new(writer, seq_counter, debug);
    let console = Console::init_with_logger(logger, &mut context);
    context
        .register_global_property(Console::NAME, console, Attribute::all())
        .expect("Failed to register console");
    
    context
}

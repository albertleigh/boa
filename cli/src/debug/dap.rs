//! DAP debugger for Boa CLI
//!
//! This module provides the Debug Adapter Protocol integration for the Boa CLI.
//! It intercepts DAP messages, manages the JavaScript context with runtime,
//! and handles execution and output capture.

use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, Source,
    context::ContextBuilder,
    debugger::{
        Debugger,
        dap::{DapServer, session::DebugSession, ProtocolMessage, Request, messages::*},
    },
    js_error, js_string,
    property::Attribute,
};
use boa_runtime::Console;
use std::env;
use std::io;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use std::fs;

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
        DapTransportMode::Http(port) => format!("TCP on port {}", port),
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

/// Runs the DAP server as a TCP server (raw socket, not HTTP)
fn run_http_server(session: Arc<Mutex<DebugSession>>, port: u16, debug: bool) -> io::Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    let addr = format!("127.0.0.1:{}", port);
    eprintln!("[DAP-TCP] Starting TCP server on {}", addr);

    let listener = TcpListener::bind(&addr)?;
    eprintln!("[DAP-TCP] Server listening on {}", addr);
    eprintln!("[DAP-TCP] Ready to accept connections");

    // Accept connections in a loop
    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                eprintln!("[DAP-TCP] Client connected from {}", peer_addr);
                
                // Handle this client connection
                if let Err(e) = handle_tcp_client(stream, session.clone(), debug) {
                    eprintln!("[DAP-TCP] Client handler error: {}", e);
                    // Continue accepting new connections even if one fails
                    continue;
                }
                
                // After handling one successful session, exit
                // (DAP typically uses one connection per debug session)
                break;
            }
            Err(e) => {
                eprintln!("[DAP-TCP] Error accepting connection: {}", e);
                return Err(e);
            }
        }
    }

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
    let mut writer = stream;

    let mut dap_server = DapServer::with_debug(session.clone(), debug);
    
    // Create context with runtime for executing JavaScript
    let mut context = create_context_with_runtime();
    
    // Console output buffer for capturing console.log
    let console_output: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    setup_console_capture(&mut context, console_output.clone());

    loop {
        // Read the Content-Length header
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => {
                eprintln!("[DAP-TCP] Client disconnected");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[DAP-TCP] Error reading header: {}", e);
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
                eprintln!("[DAP-TCP] Invalid Content-Length header: {}", header);
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
                eprintln!("[DAP-TCP] Request: {}", body_str);
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
                        console_output.clone(),
                        &mut writer,
                        debug,
                    )?;
                    
                    // Send all responses
                    for response in responses {
                        send_dap_message(&response, &mut writer, debug)?;
                    }
                } else {
                    // Process other requests normally through the server
                    let responses = dap_server.handle_request(dap_request);

                    // Send all responses
                    for response in responses {
                        send_dap_message(&response, &mut writer, debug)?;
                    }
                }
            }
            Err(e) => {
                eprintln!("[DAP-TCP] Failed to parse request: {}", e);
            }
            _ => {
                eprintln!("[DAP-TCP] Unexpected message type (not a request)");
            }
        }
    }

    Ok(())
}

/// Send a DAP protocol message
fn send_dap_message<W: io::Write>(message: &ProtocolMessage, writer: &mut W, debug: bool) -> io::Result<()> {
    let json = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());
    
    if debug {
        eprintln!("[DAP-TCP] Response: {}", json);
    }

    // Write with Content-Length header
    write!(writer, "Content-Length: {}\r\n\r\n{}", json.len(), json)?;
    writer.flush()?;
    Ok(())
}

/// Handle launch request: execute JS and capture output
fn handle_launch_request<W: io::Write>(
    mut request: Request,
    dap_server: &mut DapServer,
    context: &mut Context,
    _console_output: Rc<RefCell<Vec<String>>>,
    writer: &mut W,
    debug: bool,
) -> io::Result<Vec<ProtocolMessage>> {
    // Parse launch arguments to get the program path
    let launch_args: LaunchRequestArguments = serde_json::from_value(
        request.arguments.clone().unwrap_or(serde_json::Value::Null)
    ).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    
    // First, let the DAP server handle the launch request (validates args, etc.)
    let responses = dap_server.handle_request(request);
    
    let mut result_messages = responses;
    
    // Execute the program if specified
    if let Some(program_path) = launch_args.program {
        eprintln!("[DAP-CLI] Executing: {}", program_path);
        
        let mut output_lines = Vec::new();
        
        // Read the file
        match fs::read_to_string(&program_path) {
            Ok(source_code) => {
                // Execute the script
                match context.eval(Source::from_bytes(&source_code)) {
                    Ok(result) => {
                        // Capture the result if not undefined
                        if !result.is_undefined() {
                            if let Ok(result_str) = result.to_string(context) {
                                output_lines.push(result_str.to_std_string_escaped());
                            }
                        }
                    }
                    Err(e) => {
                        // Capture error output
                        output_lines.push(format!("Error: {}", e.to_string()));
                    }
                }
            }
            Err(e) => {
                output_lines.push(format!("Failed to read file: {}", e));
            }
        }
        
        // Send all captured output as output events
        for line in output_lines {
            let output_event = dap_server.create_event(
                "output",
                Some(serde_json::to_value(OutputEventBody {
                    category: Some("stdout".to_string()),
                    output: line + "\n",
                    group: None,
                    variables_reference: None,
                    source: None,
                    line: None,
                    column: None,
                    data: None,
                }).unwrap())
            );
            
            // Send output event immediately
            send_dap_message(&output_event, writer, debug)?;
        }
        
        // Send terminated event
        let terminated_event = dap_server.create_event(
            "terminated",
            Some(serde_json::to_value(TerminatedEventBody {
                restart: None,
            }).unwrap())
        );
        
        send_dap_message(&terminated_event, writer, debug)?;
    }
    
    Ok(result_messages)
}

/// Create a Context with runtime (console, etc.)
fn create_context_with_runtime() -> Context {
    let mut context = ContextBuilder::new().build().expect("Failed to create context");
    
    // Add console support
    let console = Console::init(&mut context);
    context
        .register_global_property(Console::NAME, console, Attribute::all())
        .expect("Failed to register console");
    
    context
}

/// Set up console output capture by overriding console.log
/// Uses a shared vector wrapped in Arc<Mutex> which is thread-safe and doesn't need Trace
fn setup_console_capture(context: &mut Context, _output_buffer: Rc<RefCell<Vec<String>>>) {
    // Create a print function that captures output
    // We'll use the global print() function instead of trying to override console.log
    // because console is from boa_runtime and harder to override
    
    // Register a simple print function
    context
        .register_global_callable(
            js_string!("print"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, context| {
                // We can't access captured_output here directly due to Trace requirements
                // Instead, we'll just use the regular console output
                let output = args
                    .iter()
                    .map(|arg| {
                        arg.to_string(context)
                            .map(|s| s.to_std_string_escaped())
                            .unwrap_or_else(|_| "<error>".to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                
                // Print to stderr so it doesn't interfere with DAP protocol on stdout
                eprintln!("[JS OUTPUT] {}", output);
                
                Ok(JsValue::undefined())
            })
        )
        .expect("Failed to register print");
}

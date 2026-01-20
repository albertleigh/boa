//! DAP server implementation
//!
//! This module implements the Debug Adapter Protocol server that handles
//! JSON-RPC communication with DAP clients (like VS Code).

use super::{Event, ProtocolMessage, Request, Response, messages::*, session::DebugSession};
use crate::{JsError, JsNativeError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

/// DAP server that handles protocol communication
pub struct DapServer {
    /// The debug session
    session: Arc<Mutex<DebugSession>>,

    /// Sequence number for responses and events
    seq: i64,

    /// Whether the server has been initialized
    initialized: bool,
}

impl DapServer {
    /// Creates a new DAP server
    pub fn new(session: Arc<Mutex<DebugSession>>) -> Self {
        Self {
            session,
            seq: 1,
            initialized: false,
        }
    }

    /// Gets the next sequence number
    fn next_seq(&mut self) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    /// Runs the DAP server on stdin/stdout
    pub fn run(&mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut stdout = io::stdout();

        loop {
            // Read the Content-Length header
            let mut header = String::new();
            reader.read_line(&mut header)?;

            if header.trim().is_empty() {
                continue;
            }

            let content_length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid Content-Length")
                })?;

            // Read the empty line
            let mut empty = String::new();
            reader.read_line(&mut empty)?;

            // Read the message body
            let mut buffer = vec![0u8; content_length];
            reader.read_exact(&mut buffer)?;

            // Parse the message
            let message: ProtocolMessage = serde_json::from_slice(&buffer)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            // Handle the message
            if let ProtocolMessage::Request(request) = message {
                let responses = self.handle_request(request);

                // Send all responses
                for response in responses {
                    self.send_message(&response, &mut stdout)?;
                }
            }
        }
    }

    /// Handles a DAP request and returns responses/events
    fn handle_request(&mut self, request: Request) -> Vec<ProtocolMessage> {
        let command = request.command.clone();
        let request_seq = request.seq;

        let result = match command.as_str() {
            "initialize" => self.handle_initialize(request),
            "launch" => self.handle_launch(request),
            "attach" => self.handle_attach(request),
            "configurationDone" => self.handle_configuration_done(request),
            "setBreakpoints" => self.handle_set_breakpoints(request),
            "continue" => self.handle_continue(request),
            "next" => self.handle_next(request),
            "stepIn" => self.handle_step_in(request),
            "stepOut" => self.handle_step_out(request),
            "stackTrace" => self.handle_stack_trace(request),
            "scopes" => self.handle_scopes(request),
            "variables" => self.handle_variables(request),
            "evaluate" => self.handle_evaluate(request),
            "threads" => self.handle_threads(request),
            "disconnect" => {
                return vec![self.create_response(request_seq, &command, true, None, None)];
            }
            _ => {
                return vec![self.create_response(
                    request_seq,
                    &command,
                    false,
                    Some(format!("Unknown command: {}", command)),
                    None,
                )];
            }
        };

        match result {
            Ok(messages) => messages,
            Err(err) => {
                vec![self.create_response(
                    request_seq,
                    &command,
                    false,
                    Some(err.to_string()),
                    None,
                )]
            }
        }
    }

    fn handle_initialize(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: InitializeRequestArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let capabilities = self.session.lock().unwrap().handle_initialize(args)?;
        self.initialized = true;

        let body = serde_json::to_value(capabilities).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_launch(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: LaunchRequestArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        self.session.lock().unwrap().handle_launch(args)?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_attach(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: AttachRequestArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        self.session.lock().unwrap().handle_attach(args)?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_configuration_done(
        &mut self,
        request: Request,
    ) -> Result<Vec<ProtocolMessage>, JsError> {
        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_set_breakpoints(
        &mut self,
        request: Request,
    ) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: SetBreakpointsArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let response_body = self.session.lock().unwrap().handle_set_breakpoints(args)?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_continue(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: ContinueArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let response_body = self.session.lock().unwrap().handle_continue(args)?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_next(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: NextArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        // TODO: Get actual frame depth from context
        self.session.lock().unwrap().handle_next(args, 0)?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_step_in(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: StepInArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        self.session.lock().unwrap().handle_step_in(args)?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_step_out(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: StepOutArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        // TODO: Get actual frame depth from context
        self.session.lock().unwrap().handle_step_out(args, 0)?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            None,
        )])
    }

    fn handle_stack_trace(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        // This would need a context reference - for now, return empty
        // TODO: Implement proper context passing
        Err(JsNativeError::error()
            .with_message("Stack trace not yet implemented")
            .into())
    }

    fn handle_scopes(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: ScopesArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let response_body = self.session.lock().unwrap().handle_scopes(args)?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_variables(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: VariablesArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let response_body = self.session.lock().unwrap().handle_variables(args)?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_evaluate(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let args: EvaluateArguments = serde_json::from_value(
            request.arguments.unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("Invalid arguments: {}", e)))?;

        let response_body = self.session.lock().unwrap().handle_evaluate(args)?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    fn handle_threads(&mut self, request: Request) -> Result<Vec<ProtocolMessage>, JsError> {
        let response_body = self.session.lock().unwrap().handle_threads()?;

        let body = serde_json::to_value(response_body).map_err(|e| {
            JsNativeError::typ().with_message(format!("Failed to serialize: {}", e))
        })?;

        Ok(vec![self.create_response(
            request.seq,
            &request.command,
            true,
            None,
            Some(body),
        )])
    }

    /// Creates a response message
    fn create_response(
        &mut self,
        request_seq: i64,
        command: &str,
        success: bool,
        message: Option<String>,
        body: Option<serde_json::Value>,
    ) -> ProtocolMessage {
        ProtocolMessage::Response(Response {
            seq: self.next_seq(),
            request_seq,
            success,
            command: command.to_string(),
            message,
            body,
        })
    }

    /// Creates an event message
    pub fn create_event(
        &mut self,
        event: &str,
        body: Option<serde_json::Value>,
    ) -> ProtocolMessage {
        ProtocolMessage::Event(Event {
            seq: self.next_seq(),
            event: event.to_string(),
            body,
        })
    }

    /// Sends a protocol message
    fn send_message<W: Write>(&self, message: &ProtocolMessage, writer: &mut W) -> io::Result<()> {
        let json = serde_json::to_string(message)?;
        let content_length = json.len();

        write!(writer, "Content-Length: {}\r\n\r\n{}", content_length, json)?;
        writer.flush()?;

        Ok(())
    }
}

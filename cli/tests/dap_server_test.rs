//! Integration tests for the DAP (Debug Adapter Protocol) server
//!
//! This test suite validates that the CLI DAP server correctly:
//! - Starts and accepts connections
//! - Handles DAP protocol messages
//! - Uses the proper typed structs from boa_engine
//! - Executes JavaScript and returns output

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::json;

/// Helper to send a DAP message with proper Content-Length header
fn send_dap_message(stdin: &mut std::process::ChildStdin, message: &serde_json::Value) {
    let message_json = serde_json::to_string(message).expect("Failed to serialize message");
    let header = format!("Content-Length: {}\r\n\r\n", message_json.len());

    stdin
        .write_all(header.as_bytes())
        .expect("Failed to write header");
    stdin
        .write_all(message_json.as_bytes())
        .expect("Failed to write message");
    stdin.flush().expect("Failed to flush");

    eprintln!("→ Sent: {}", message_json);
}

/// Helper to read a DAP response
fn read_dap_response(
    reader: &mut BufReader<std::process::ChildStdout>,
) -> Option<serde_json::Value> {
    // Read Content-Length header
    let mut header = String::new();
    if reader.read_line(&mut header).unwrap_or(0) == 0 {
        return None;
    }

    if !header.starts_with("Content-Length:") {
        eprintln!("Invalid header: {}", header);
        return None;
    }

    let length: usize = header
        .trim()
        .strip_prefix("Content-Length:")
        .and_then(|s| s.trim().parse().ok())?;

    // Read empty line
    let mut empty = String::new();
    reader.read_line(&mut empty).ok()?;

    // Read body
    let mut body = vec![0u8; length];
    std::io::Read::read_exact(reader, &mut body).ok()?;

    let response: serde_json::Value = serde_json::from_slice(&body).ok()?;
    eprintln!(
        "← Received: {}",
        serde_json::to_string_pretty(&response).unwrap()
    );

    Some(response)
}

#[test]
fn test_dap_server_initialize() {
    eprintln!("\n=== Testing DAP Server Initialize ===\n");

    // Start the DAP server
    let mut child = Command::new("cargo")
        .args(&["run", "--package", "boa_cli", "--", "--dap"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start DAP server");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Send initialize request
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {
                "clientID": "test",
                "clientName": "Test Client",
                "adapterID": "boa",
                "locale": "en-US",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path"
            }
        }),
    );

    // Read initialize response
    let response = read_dap_response(&mut reader).expect("Failed to read initialize response");

    assert_eq!(response["type"], "response");
    assert_eq!(response["command"], "initialize");
    assert_eq!(response["success"], true);

    // Verify capabilities are returned
    let body = response["body"]
        .as_object()
        .expect("Body should be an object");
    assert!(
        body.contains_key("supports_configuration_done_request")
            || body.contains_key("supportsConfigurationDoneRequest"),
        "Should have configurationDone support"
    );

    eprintln!("✓ Initialize response received with capabilities");

    // Read initialized event
    let event = read_dap_response(&mut reader).expect("Failed to read initialized event");

    assert_eq!(event["type"], "event");
    assert_eq!(event["event"], "initialized");

    eprintln!("✓ Initialized event received");

    // Clean up
    child.kill().ok();
    child.wait().ok();

    eprintln!("\n=== Initialize Test Passed ===\n");
}

#[test]
fn test_dap_server_threads() {
    eprintln!("\n=== Testing DAP Server Threads ===\n");

    // Start the DAP server
    let mut child = Command::new("cargo")
        .args(&["run", "--package", "boa_cli", "--", "--dap"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start DAP server");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Send initialize first
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {}
        }),
    );

    // Skip initialize response and event
    read_dap_response(&mut reader);
    read_dap_response(&mut reader);

    // Send threads request
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 2,
            "type": "request",
            "command": "threads",
            "arguments": {}
        }),
    );

    // Read threads response
    let response = read_dap_response(&mut reader).expect("Failed to read threads response");

    assert_eq!(response["type"], "response");
    assert_eq!(response["command"], "threads");
    assert_eq!(response["success"], true);

    // Verify threads are returned
    let threads = response["body"]["threads"]
        .as_array()
        .expect("Threads should be an array");

    assert!(!threads.is_empty(), "Should have at least one thread");
    assert_eq!(threads[0]["id"], 1);
    assert_eq!(threads[0]["name"], "Main Thread");

    eprintln!("✓ Threads response received with main thread");

    // Clean up
    child.kill().ok();
    child.wait().ok();

    eprintln!("\n=== Threads Test Passed ===\n");
}

#[test]
fn test_dap_server_unknown_command() {
    eprintln!("\n=== Testing DAP Server Unknown Command ===\n");

    // Start the DAP server
    let mut child = Command::new("cargo")
        .args(&["run", "--package", "boa_cli", "--", "--dap"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start DAP server");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Send unknown command
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 1,
            "type": "request",
            "command": "unknownCommand",
            "arguments": {}
        }),
    );

    // Read response
    let response = read_dap_response(&mut reader).expect("Failed to read response");

    assert_eq!(response["type"], "response");
    assert_eq!(response["command"], "unknownCommand");
    assert_eq!(response["success"], false);

    // Should have an error message
    let message = response["message"]
        .as_str()
        .expect("Should have error message");
    assert!(
        message.contains("not implemented"),
        "Error message should mention 'not implemented'"
    );

    eprintln!("✓ Unknown command properly rejected with error message");

    // Clean up
    child.kill().ok();
    child.wait().ok();

    eprintln!("\n=== Unknown Command Test Passed ===\n");
}

#[test]
#[ignore] // This test takes longer as it executes JavaScript
fn test_dap_server_launch_program() {
    eprintln!("\n=== Testing DAP Server Launch Program ===\n");

    // Create a test JavaScript file
    let test_file = "test_dap_temp.js";
    std::fs::write(
        test_file,
        r#"
        console.log("Hello from Boa!");
        console.log("Test output");
        42 + 58;
    "#,
    )
    .expect("Failed to write test file");

    // Start the DAP server
    let mut child = Command::new("cargo")
        .args(&["run", "--package", "boa_cli", "--", "--dap"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start DAP server");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Send initialize
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {}
        }),
    );

    // Skip initialize responses
    read_dap_response(&mut reader);
    read_dap_response(&mut reader);

    // Send launch request
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": test_file
            }
        }),
    );

    // Read responses until we get terminated event
    let mut got_output = false;
    let mut got_exited = false;
    let mut got_terminated = false;

    for _ in 0..20 {
        if let Some(message) = read_dap_response(&mut reader) {
            let msg_type = message["type"].as_str().unwrap_or("");

            if msg_type == "event" {
                let event = message["event"].as_str().unwrap_or("");

                match event {
                    "output" => {
                        got_output = true;
                        let output = message["body"]["output"].as_str().unwrap_or("");
                        eprintln!("  Output: {}", output);
                    }
                    "exited" => {
                        got_exited = true;
                        eprintln!("  ✓ Exited event");
                    }
                    "terminated" => {
                        got_terminated = true;
                        eprintln!("  ✓ Terminated event");
                        break;
                    }
                    _ => {}
                }
            }
        } else {
            break;
        }
    }

    assert!(got_output, "Should receive output events");
    assert!(got_exited, "Should receive exited event");
    assert!(got_terminated, "Should receive terminated event");

    eprintln!("✓ Launch program executed successfully");

    // Clean up
    std::fs::remove_file(test_file).ok();
    child.kill().ok();
    child.wait().ok();

    eprintln!("\n=== Launch Program Test Passed ===\n");
}

#[test]
fn test_dap_typed_structs() {
    // This test verifies that the DAP server is using typed structs
    // by checking the response structure matches what the structs would produce

    eprintln!("\n=== Testing DAP Typed Structs Usage ===\n");

    let mut child = Command::new("cargo")
        .args(&["run", "--package", "boa_cli", "--", "--dap"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start DAP server");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Send setBreakpoints request to test typed response
    send_dap_message(
        &mut stdin,
        &json!({
            "seq": 1,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": "test.js"
                },
                "breakpoints": []
            }
        }),
    );

    let response = read_dap_response(&mut reader).expect("Failed to read response");

    // Verify the response has the correct structure from typed structs
    assert_eq!(response["type"], "response");
    assert_eq!(response["command"], "setBreakpoints");
    assert_eq!(response["success"], true);
    assert!(response["seq"].is_number());
    assert!(response["request_seq"].is_number());

    // The body should have breakpoints array (from SetBreakpointsResponseBody)
    let body = response["body"]
        .as_object()
        .expect("Body should be present");
    assert!(
        body.contains_key("breakpoints"),
        "Should have breakpoints field from typed struct"
    );

    eprintln!("✓ Response structure matches typed structs");

    // Clean up
    child.kill().ok();
    child.wait().ok();

    eprintln!("\n=== Typed Structs Test Passed ===\n");
}

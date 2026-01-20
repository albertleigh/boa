//! Transport layer for DAP communication
//!
//! This module provides different transport mechanisms for the DAP protocol.

use std::io::{self, Read, Write};

/// A trait for transporting DAP messages
pub trait Transport: Read + Write {
    /// Reads a message from the transport
    fn read_message(&mut self) -> io::Result<String>;

    /// Writes a message to the transport
    fn write_message(&mut self, message: &str) -> io::Result<()>;
}

/// Standard I/O transport (stdin/stdout)
pub struct StdioTransport {
    stdin: io::Stdin,
    stdout: io::Stdout,
}

impl StdioTransport {
    pub fn new() -> Self {
        Self {
            stdin: io::stdin(),
            stdout: io::stdout(),
        }
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Read for StdioTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stdin.read(buf)
    }
}

impl Write for StdioTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

impl Transport for StdioTransport {
    fn read_message(&mut self) -> io::Result<String> {
        // Read Content-Length header
        let mut header = String::new();
        let mut buf = [0u8; 1];

        loop {
            self.stdin.read_exact(&mut buf)?;
            header.push(buf[0] as char);
            if header.ends_with("\r\n\r\n") {
                break;
            }
        }

        // Parse content length
        let content_length: usize = header
            .lines()
            .find(|line| line.starts_with("Content-Length:"))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length"))?;

        // Read message body
        let mut body = vec![0u8; content_length];
        self.stdin.read_exact(&mut body)?;

        String::from_utf8(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn write_message(&mut self, message: &str) -> io::Result<()> {
        write!(
            self.stdout,
            "Content-Length: {}\r\n\r\n{}",
            message.len(),
            message
        )?;
        self.stdout.flush()
    }
}

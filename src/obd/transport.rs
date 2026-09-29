//! Pluggable byte transports for the ELM327 engine.
//!
//! The protocol engine is transport-agnostic: tests and `--demo` use
//! [`MockTransport`] (real request/response framing, scripted vehicles), and
//! real hardware goes through `SerialTransport` (enabled by the `hardware`
//! cargo feature).

use anyhow::Result;

/// A line-oriented ELM327 link: write a command, read the reply up to `>`.
pub trait Transport {
    /// Send `command` (without the terminating CR) and return the raw reply
    /// (may include echo, whitespace, and the trailing `>` prompt).
    fn transact(&mut self, command: &str) -> Result<String>;
}

impl Transport for Box<dyn Transport> {
    fn transact(&mut self, command: &str) -> Result<String> {
        (**self).transact(command)
    }
}

/// Transport backed by a handler function — used for tests, `--demo` and
/// simulated drives. The handler receives the raw command and returns the raw
/// ELM327-style reply (the engine adds/strips the prompt).
pub struct MockTransport {
    handler: Box<dyn FnMut(&str) -> String>,
    /// Log of every command sent (for assertions and debugging).
    pub sent: Vec<String>,
}

impl MockTransport {
    pub fn new(handler: impl FnMut(&str) -> String + 'static) -> Self {
        Self { handler: Box::new(handler), sent: Vec::new() }
    }

    /// Fixed request → response map (unknown commands answer `?`).
    pub fn scripted(map: Vec<(String, String)>) -> Self {
        let map: std::collections::HashMap<String, String> = map.into_iter().collect();
        Self::new(move |cmd| {
            map.get(cmd.trim())
                .cloned()
                .unwrap_or_else(|| "?".to_string())
        })
    }
}

impl Transport for MockTransport {
    fn transact(&mut self, command: &str) -> Result<String> {
        self.sent.push(command.trim().to_string());
        let mut reply = (self.handler)(command.trim());
        // ELM327 replies terminate with the prompt character.
        if !reply.ends_with('>') {
            reply.push('>');
        }
        Ok(reply)
    }
}

/// Real serial-port transport (ELM327-compatible OBD dongles).
///
/// Enabled with `--features hardware` (pulls in the `serialport` crate).
#[cfg(feature = "hardware")]
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

#[cfg(feature = "hardware")]
impl SerialTransport {
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(std::time::Duration::from_millis(5000))
            .open()?;
        Ok(Self { port })
    }
}

#[cfg(feature = "hardware")]
impl Transport for SerialTransport {
    fn transact(&mut self, command: &str) -> Result<String> {
        use std::io::{Read, Write};
        self.port.write_all(command.as_bytes())?;
        self.port.write_all(b"\r")?;
        self.port.flush()?;
        // Read until the '>' prompt.
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = self.port.read(&mut buf)?;
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'>') || n == 0 {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&out).to_string())
    }
}

//! RS485 serial transport (tokio-serial), line-oriented on the `\r` terminator.

use crate::error::{Error, Result};
use crate::protocol::EOI;
use crate::transport::Transport;
use async_trait::async_trait;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

pub struct SerialTransport {
    path: String,
    baud: u32,
    port: Option<SerialStream>,
    buf: Vec<u8>,
}

impl SerialTransport {
    pub fn new(path: &str, baud: u32) -> Self {
        Self {
            path: path.to_string(),
            baud,
            port: None,
            buf: Vec::new(),
        }
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn open(&mut self) -> Result<()> {
        let port = tokio_serial::new(&self.path, self.baud)
            .open_native_async()
            .map_err(|e| Error::Transport(format!("serial open {}: {e}", self.path)))?;
        self.port = Some(port);
        self.buf.clear();
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.port = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let port = self.port.as_mut().ok_or(Error::NotFound)?;
        // Fresh request: drop any partial line left from before.
        self.buf.clear();
        port.write_all(data)
            .await
            .map_err(|e| Error::Transport(format!("serial write: {e}")))?;
        Ok(data.len())
    }

    async fn read_line(&mut self) -> Result<Vec<u8>> {
        let port = self.port.as_mut().ok_or(Error::NotFound)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut chunk = [0u8; 256];
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == EOI) {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                return Ok(line);
            }
            if Instant::now() >= deadline {
                return Ok(Vec::new());
            }
            match tokio::time::timeout(Duration::from_millis(500), port.read(&mut chunk)).await {
                Ok(Ok(0)) => return Err(Error::Transport("serial port closed".into())),
                Ok(Ok(n)) => self.buf.extend_from_slice(&chunk[..n]),
                Ok(Err(e)) => return Err(Error::Transport(format!("serial read: {e}"))),
                Err(_) => {} // idle; keep waiting until the deadline
            }
        }
    }
}

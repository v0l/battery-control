//! Serial/UART transport for JBD modules. Same frames as BLE.

use crate::error::{Error, Result};
use crate::transport::Transport;
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

pub struct SerialTransport {
    path: String,
    baud: u32,
    port: Option<SerialStream>,
}

impl SerialTransport {
    /// `target` is `"<path>"` or `"<path>,<baud>"` (default baud 9600).
    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let path = parts.next().unwrap_or(target).to_string();
        let baud = parts.next().and_then(|b| b.trim().parse().ok()).unwrap_or(9600);
        Self { path, baud, port: None }
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn open(&mut self) -> Result<()> {
        let port = tokio_serial::new(&self.path, self.baud)
            .open_native_async()
            .map_err(|e| Error::Transport(format!("serial open {}: {e}", self.path)))?;
        self.port = Some(port);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.port = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let port = self.port.as_mut().ok_or(Error::NotFound)?;
        port.write_all(data)
            .await
            .map_err(|e| Error::Transport(format!("serial write: {e}")))?;
        Ok(data.len())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let port = self.port.as_mut().ok_or(Error::NotFound)?;
        match tokio::time::timeout(std::time::Duration::from_secs(2), port.read(buf)).await {
            Ok(Ok(n)) => Ok(n),
            Ok(Err(e)) => Err(Error::Transport(format!("serial read: {e}"))),
            Err(_) => Ok(0),
        }
    }
}

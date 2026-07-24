use crate::{Transport, Result, JkError, async_trait};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

/// Serial (UART / USB-serial) transport backed by `tokio-serial`.
pub struct SerialTransport {
    port_name: String,
    baud_rate: u32,
    port: Option<SerialStream>,
}

impl SerialTransport {
    pub fn new(port_name: &str, baud_rate: u32) -> Self {
        Self {
            port_name: port_name.to_string(),
            baud_rate,
            port: None,
        }
    }

    /// Parse a `path[,baud]` target, e.g. `/dev/ttyUSB0,9600`.
    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let port = parts.next().unwrap_or("/dev/ttyUSB0");
        let baud = parts.next().and_then(|s| s.parse().ok()).unwrap_or(9600);
        Self::new(port, baud)
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn open(&mut self) -> Result<()> {
        let port = tokio_serial::new(&self.port_name, self.baud_rate)
            .open_native_async()
            .map_err(|e| JkError::TransportError(format!(
                "open {}: {}", self.port_name, e
            )))?;
        self.port = Some(port);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        // Dropping the stream closes the underlying fd/handle.
        self.port = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let port = self.port.as_mut().ok_or(JkError::TransportNotInitialized)?;
        port.write_all(data).await.map_err(|_| JkError::WriteFailed(-1))?;
        Ok(data.len())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let port = self.port.as_mut().ok_or(JkError::TransportNotInitialized)?;
        match tokio::time::timeout(Duration::from_secs(3), port.read(buf)).await {
            Ok(Ok(n)) => Ok(n),
            Ok(Err(_)) => Err(JkError::ReadFailed(-1)),
            Err(_) => Ok(0), // timed out with no data
        }
    }
}

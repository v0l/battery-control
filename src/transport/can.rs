use crate::{Transport, Result, JkError, async_trait};
use std::time::{Duration, Instant};

use socketcan::tokio::CanSocket;
use socketcan::{CanFrame, EmbeddedFrame, ExtendedId, Frame, Id};

/// 29-bit extended CAN identifier mask.
const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

/// Linux SocketCAN transport backed by the async `socketcan` tokio socket.
pub struct CanTransport {
    interface: String,
    rx_id: u32,
    tx_id: u32,
    socket: Option<CanSocket>,
}

impl CanTransport {
    pub fn new(interface: &str, rx_id: u32, tx_id: u32) -> Self {
        Self {
            interface: interface.to_string(),
            rx_id,
            tx_id,
            socket: None,
        }
    }

    /// Parse a `can:can0,rx_id,tx_id` (or `can0,rx_id,tx_id`) target.
    pub fn from_target(target: &str) -> Result<Self> {
        let parts: Vec<&str> = target.split(',').collect();
        if parts.len() < 3 {
            return Err(JkError::TransportError(
                "Invalid CAN target format. Use: can:can0,rx_id,tx_id".to_string(),
            ));
        }

        let interface = parts[0].trim_start_matches("can:");
        let rx_id = u32::from_str_radix(parts[1].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid RX CAN ID".to_string()))?;
        let tx_id = u32::from_str_radix(parts[2].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid TX CAN ID".to_string()))?;

        Ok(Self::new(interface, rx_id, tx_id))
    }
}

#[async_trait]
impl Transport for CanTransport {
    async fn open(&mut self) -> Result<()> {
        let socket = CanSocket::open(&self.interface).map_err(|e| {
            JkError::TransportError(format!("open CAN {}: {}", self.interface, e))
        })?;
        self.socket = Some(socket);
        log::info!(
            "CAN transport opened on {} with RX=0x{:07X}, TX=0x{:07X}",
            self.interface, self.rx_id, self.tx_id
        );
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.socket = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let socket = self.socket.as_ref().ok_or(JkError::TransportNotInitialized)?;

        let len = data.len().min(8);
        if data.len() > 8 {
            log::warn!("CAN frame too large ({} bytes), truncating to 8 bytes", data.len());
        }

        let id = ExtendedId::new(self.tx_id & CAN_EFF_MASK)
            .ok_or_else(|| JkError::TransportError("invalid TX CAN id".to_string()))?;
        let frame = CanFrame::new(Id::Extended(id), &data[..len])
            .ok_or(JkError::WriteFailed(-1))?;

        socket.write_frame(frame)
            .await
            .map_err(|_| JkError::WriteFailed(-1))?;
        Ok(len)
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let socket = self.socket.as_ref().ok_or(JkError::TransportNotInitialized)?;

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(0);
            }

            let frame = match tokio::time::timeout(remaining, socket.read_frame()).await {
                Ok(Ok(frame)) => frame,
                Ok(Err(_)) => return Err(JkError::ReadFailed(-1)),
                Err(_) => return Ok(0), // timed out
            };

            // Only accept frames from the BMS's response id.
            if frame.raw_id() & CAN_EFF_MASK != self.rx_id & CAN_EFF_MASK {
                continue;
            }

            let payload = frame.data();
            if payload.is_empty() {
                continue;
            }
            let copy_len = payload.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&payload[..copy_len]);
            log::debug!("CAN read {} bytes", copy_len);
            return Ok(copy_len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_transport_parsing() {
        let transport = CanTransport::from_target("can:can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "can0");
        assert_eq!(t.rx_id, 0x18ff0000);
        assert_eq!(t.tx_id, 0x18fe0000);
    }

    #[test]
    fn test_can_transport_parsing_without_dev() {
        let transport = CanTransport::from_target("can:/dev/can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "/dev/can0");
    }

    #[test]
    fn test_can_transport_invalid_format() {
        let transport = CanTransport::from_target("can:/dev/can0");
        assert!(transport.is_err());
    }

    #[test]
    fn test_can_transport_invalid_ids() {
        let transport = CanTransport::from_target("can:/dev/can0,invalid,0x18fe0000");
        assert!(transport.is_err());
    }
}

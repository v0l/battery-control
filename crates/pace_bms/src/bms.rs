//! High-level PACE device handle: read a pack over RS485 by its bus address.

use crate::error::{Error, Result};
use crate::protocol::{self, PaceData, READ_COUNT, READ_START};
use crate::transport::Transport;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A connected PACE BMS (one pack on the RS485 bus).
pub struct PaceBms {
    transport: Box<dyn Transport>,
    address: u8,
    data: PaceData,
}

impl PaceBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>, address: u8) -> Result<Self> {
        transport.open().await?;
        Ok(Self {
            transport,
            address,
            data: PaceData::default(),
        })
    }

    /// Open a PACE pack over RS485: `path` e.g. `/dev/ttyUSB0`, `address` 1..N.
    #[cfg(feature = "serial")]
    pub async fn open_serial(path: &str, baud: u32, address: u8) -> Result<Self> {
        Self::with_transport(
            Box::new(crate::transport::SerialTransport::new(path, baud)),
            address,
        )
        .await
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    pub fn model(&self) -> String {
        format!("PACE {} cells", self.data.cells.len().max(0))
    }

    /// Read a fresh snapshot (one Modbus read of registers 0..=36).
    pub async fn read(&mut self) -> Result<&PaceData> {
        let req = modbus_lite::build_read(self.address, READ_START, READ_COUNT);
        log::debug!("pace tx addr={}: {}", self.address, hex(&req));
        self.transport.write(&req).await?;

        let mut buf = Vec::new();
        for _ in 0..64 {
            let chunk = self.transport.read_frame().await?;
            if chunk.is_empty() {
                if buf.is_empty() {
                    continue;
                }
                break;
            }
            buf.extend_from_slice(&chunk);
            if let Some(total) = modbus_lite::response_len(&buf) {
                if buf.len() >= total {
                    log::debug!("pace rx addr={}: {}", self.address, hex(&buf[..total]));
                    let body = modbus_lite::verify(&buf[..total])
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    self.data = protocol::decode(body);
                    return Ok(&self.data);
                }
            }
        }
        Err(Error::Timeout)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::READ_COUNT;
    use async_trait::async_trait;
    use modbus_lite::crc16;
    use std::collections::VecDeque;

    struct Scripted {
        responses: VecDeque<Vec<u8>>,
        pending: VecDeque<Vec<u8>>,
    }

    #[async_trait]
    impl Transport for Scripted {
        async fn open(&mut self) -> Result<()> {
            Ok(())
        }
        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
        async fn write(&mut self, _data: &[u8]) -> Result<usize> {
            if let Some(r) = self.responses.pop_front() {
                // deliver fragmented across two reads
                let mid = r.len() / 2;
                self.pending.push_back(r[..mid].to_vec());
                self.pending.push_back(r[mid..].to_vec());
            }
            Ok(_data.len())
        }
        async fn read_frame(&mut self) -> Result<Vec<u8>> {
            Ok(self.pending.pop_front().unwrap_or_default())
        }
    }

    fn response(regs: &[(u16, u16)]) -> Vec<u8> {
        let mut data = vec![0u8; READ_COUNT as usize * 2];
        for &(r, v) in regs {
            let off = r as usize * 2;
            data[off] = (v >> 8) as u8;
            data[off + 1] = v as u8;
        }
        let mut f = vec![0x01u8, 0x03, (READ_COUNT as usize * 2) as u8];
        f.extend_from_slice(&data);
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[tokio::test]
    async fn reads_pack() {
        let resp = response(&[
            (1, 1329),
            (2, 88),
            (5, 10000),
            (15, 3320),
            (16, 3321),
            (31, 250),
        ]);
        let scripted = Scripted {
            responses: VecDeque::from(vec![resp]),
            pending: VecDeque::new(),
        };
        let mut bms = PaceBms::with_transport(Box::new(scripted), 1).await.unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.soc, 88);
        assert!((d.voltage - 13.29).abs() < 1e-2);
        assert_eq!(d.cells.len(), 2);
        assert_eq!(d.temps.len(), 1);
    }
}

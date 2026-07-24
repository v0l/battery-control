//! High-level Seplos V3 device handle: read a pack over RS485 by its address.

use crate::error::{Error, Result};
use crate::protocol::{self, SeplosData, PIA_COUNT, PIA_START, PIB_COUNT, PIB_START};
use crate::transport::Transport;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A connected Seplos V3 BMS (one pack on the RS485 bus).
///
/// Note: the Modbus slave id is `client_id - 1` in Seplos' numbering
/// (BMSStudio "client 1" == Modbus address 0).
pub struct SeplosBms {
    transport: Box<dyn Transport>,
    address: u8,
    data: SeplosData,
}

impl SeplosBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>, address: u8) -> Result<Self> {
        transport.open().await?;
        Ok(Self {
            transport,
            address,
            data: SeplosData::default(),
        })
    }

    /// Open a Seplos V3 pack over RS485 (`path`, `baud` 19200, Modbus `address`).
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
        format!("Seplos {}S", self.data.cells.len().max(0))
    }

    /// Read a fresh snapshot (PIA status + PIB cells/temps).
    pub async fn read(&mut self) -> Result<&SeplosData> {
        let mut d = SeplosData::default();
        let pia = self.read_block(PIA_START, PIA_COUNT).await?;
        protocol::decode_pia(modbus_lite::verify(&pia).map_err(|e| Error::Protocol(e.into()))?, &mut d);
        let pib = self.read_block(PIB_START, PIB_COUNT).await?;
        protocol::decode_pib(modbus_lite::verify(&pib).map_err(|e| Error::Protocol(e.into()))?, &mut d);
        self.data = d;
        Ok(&self.data)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    /// Send one input-register read and reassemble the response.
    async fn read_block(&mut self, start: u16, count: u16) -> Result<Vec<u8>> {
        let req = modbus_lite::build_read_input(self.address, start, count);
        log::debug!("seplos tx addr={} reg={start:#06x}: {}", self.address, hex(&req));
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
                    log::debug!("seplos rx reg={start:#06x}: {}", hex(&buf[..total]));
                    return Ok(buf[..total].to_vec());
                }
            }
        }
        Err(Error::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn frame(regs: &[u16]) -> Vec<u8> {
        let mut f = vec![0x00u8, 0x04, (regs.len() * 2) as u8];
        for &v in regs {
            f.push((v >> 8) as u8);
            f.push(v as u8);
        }
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[tokio::test]
    async fn reads_pia_and_pib() {
        let pia = frame(&[
            5236, 1301, 3800, 30400, 64, 125, 1000, 2, 3272, 2837, 3275, 3268, 2845, 2831, 0,
            180, 180, 1000,
        ]);
        let pib = frame(&[
            3273, 3273, 3268, 3272, 3274, 3274, 3275, 3275, 3274, 3273, 3273, 3270, 3271, 3271,
            3270, 3272, 2842, 2833, 2831, 2845, 2731, 2731, 2731, 2731, 2833, 2829,
        ]);
        let scripted = Scripted {
            responses: VecDeque::from(vec![pia, pib]),
            pending: VecDeque::new(),
        };
        let mut bms = SeplosBms::with_transport(Box::new(scripted), 0).await.unwrap();
        let d = bms.read().await.unwrap();
        assert!((d.voltage - 52.36).abs() < 1e-2);
        assert!((d.soc - 12.5).abs() < 1e-2);
        assert_eq!(d.cells.len(), 16);
        assert_eq!(d.cell_temps.len(), 4);
        assert_eq!(bms.model(), "Seplos 16S");
    }
}

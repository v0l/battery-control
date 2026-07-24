//! High-level Renogy device handle: connect and read a snapshot.

use crate::error::{Error, Result};
use modbus_lite as modbus;
use crate::protocol::{self, RenogyData, DEFAULT_UNIT, SECTIONS};
use crate::transport::Transport;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A connected Renogy smart battery.
pub struct RenogyBms {
    transport: Box<dyn Transport>,
    unit: u8,
    data: RenogyData,
    identity: ble_util::Identity,
}

impl RenogyBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>, unit: u8) -> Result<Self> {
        transport.open().await?;
        let identity = transport.identity();
        Ok(Self {
            transport,
            unit,
            data: RenogyData::default(),
            identity,
        })
    }

    /// Static device identity from the BLE Device Information Service.
    pub fn identity(&self) -> &ble_util::Identity {
        &self.identity
    }

    /// Connect over BLE using the default broadcast unit id (stand-alone pack).
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str) -> Result<Self> {
        Self::connect_ble_as(id, DEFAULT_UNIT).await
    }

    /// Connect over BLE with an explicit Modbus unit id (hub/daisy-chain: use
    /// 48/49/50 for batteries).
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble_as(id: &str, unit: u8) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::BluetoothTransport::new(id)), unit).await
    }

    pub fn model(&self) -> String {
        self.data.model.clone().unwrap_or_else(|| "Renogy".to_string())
    }

    /// Read a fresh snapshot (cells, temps, current/voltage/capacity, model).
    pub async fn read(&mut self) -> Result<&RenogyData> {
        let mut d = RenogyData::default();
        for (i, &(reg, words)) in SECTIONS.iter().enumerate() {
            let frame = self.read_section(reg, words).await?;
            let body = modbus::verify(&frame).map_err(|e| Error::Protocol(e.to_string()))?;
            match i {
                0 => d.cells = protocol::parse_cells(body),
                1 => d.temps = protocol::parse_temps(body),
                2 => {
                    let (current, voltage, remaining, capacity) = protocol::parse_info(body);
                    d.current = current;
                    d.voltage = voltage;
                    d.remaining_ah = remaining;
                    d.capacity_ah = capacity;
                }
                _ => d.model = protocol::parse_model(body),
            }
        }
        d.recompute();
        self.data = d;
        Ok(&self.data)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    /// Send one Modbus read and reassemble the response across notifications.
    async fn read_section(&mut self, reg: u16, words: u16) -> Result<Vec<u8>> {
        let req = modbus::build_read(self.unit, reg, words);
        log::debug!("renogy tx reg={reg}: {}", hex(&req));
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
            if let Some(total) = modbus::response_len(&buf) {
                if buf.len() >= total {
                    log::debug!("renogy rx reg={reg}: {}", hex(&buf[..total]));
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
    use modbus_lite::crc16;
    use async_trait::async_trait;
    use std::collections::VecDeque;

    struct Scripted {
        // queue of responses (one per section, in order)
        responses: VecDeque<Vec<u8>>,
        pending: VecDeque<Vec<u8>>, // fragments of the current response
    }

    impl Scripted {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                responses: responses.into(),
                pending: VecDeque::new(),
            }
        }
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
            // On each request, queue the next full response (split in two).
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

    fn frame(data: &[u8]) -> Vec<u8> {
        let mut f = vec![0x30u8, 0x03, data.len() as u8];
        f.extend_from_slice(data);
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[tokio::test]
    async fn reads_all_sections() {
        // cells: 4 × 3.6 V
        let mut cells = vec![0x00, 0x04];
        for _ in 0..4 {
            cells.extend_from_slice(&36u16.to_be_bytes());
        }
        // temps: 4 × 21.0 °C (raw 210)
        let mut temps = vec![0x00, 0x04];
        for _ in 0..4 {
            temps.extend_from_slice(&210u16.to_be_bytes());
        }
        // info: 1.4 A, 14.5 V, 99.941 / 100.0 Ah
        let mut info = Vec::new();
        info.extend_from_slice(&140i16.to_be_bytes());
        info.extend_from_slice(&145u16.to_be_bytes());
        info.extend_from_slice(&99_941u32.to_be_bytes());
        info.extend_from_slice(&100_000u32.to_be_bytes());
        let mut model = b"RBT100LFP12S-G\0\0".to_vec();
        model.truncate(16);

        let responses = vec![frame(&cells), frame(&temps), frame(&info), frame(&model)];
        let mut bms = RenogyBms::with_transport(Box::new(Scripted::new(responses)), 0x30)
            .await
            .unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.cells.len(), 4);
        assert_eq!(d.temps.len(), 4);
        assert!((d.voltage - 14.5).abs() < 1e-2);
        assert!((d.current - 1.4).abs() < 1e-2);
        assert!((d.soc - 99.941).abs() < 0.1);
        assert_eq!(bms.model(), "RBT100LFP12S-G");
    }
}

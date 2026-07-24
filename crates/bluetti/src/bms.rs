//! High-level Bluetti handle: read status + battery, toggle AC/DC output.

use crate::error::{Error, Result};
use crate::protocol::{
    self, BluettiData, BATTERY_COUNT, BATTERY_START, CORE_COUNT, CORE_START, CTRL_AC_OUTPUT,
    CTRL_DC_OUTPUT,
};
use crate::transport::Transport;

/// Modbus address Bluetti stations answer on.
const UNIT: u8 = 0x01;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A connected Bluetti power station.
pub struct Bluetti {
    transport: Box<dyn Transport>,
    identity: ble_util::Identity,
    data: BluettiData,
}

impl Bluetti {
    pub async fn with_transport(mut transport: Box<dyn Transport>) -> Result<Self> {
        transport.open().await?;
        let identity = transport.identity();
        Ok(Self {
            transport,
            identity,
            data: BluettiData::default(),
        })
    }

    /// Connect over BLE to the peripheral id from [`crate::scan`].
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::BluetoothTransport::new(id))).await
    }

    pub fn identity(&self) -> &ble_util::Identity {
        &self.identity
    }

    pub fn model(&self) -> String {
        self.data
            .device_type
            .clone()
            .or_else(|| self.identity.name.clone())
            .unwrap_or_else(|| "Bluetti".to_string())
    }

    /// Read a fresh snapshot (core status + battery/cells).
    pub async fn read(&mut self) -> Result<&BluettiData> {
        let mut d = BluettiData::default();
        let core = self.read_block(CORE_START, CORE_COUNT).await?;
        protocol::decode_core(
            modbus_lite::verify(&core).map_err(|e| Error::Protocol(e.into()))?,
            &mut d,
        );
        // The battery block is optional (some models lay it out differently).
        if let Ok(batt) = self.read_block(BATTERY_START, BATTERY_COUNT).await {
            if let Ok(body) = modbus_lite::verify(&batt) {
                protocol::decode_battery(body, &mut d);
            }
        }
        self.data = d;
        Ok(&self.data)
    }

    /// Turn the AC output on/off.
    pub async fn set_ac_output(&mut self, on: bool) -> Result<()> {
        self.write_register(CTRL_AC_OUTPUT, on as u16).await?;
        self.data.ac_output_on = on;
        Ok(())
    }

    /// Turn the DC output on/off.
    pub async fn set_dc_output(&mut self, on: bool) -> Result<()> {
        self.write_register(CTRL_DC_OUTPUT, on as u16).await?;
        self.data.dc_output_on = on;
        Ok(())
    }

    /// Control by name — `set("ac", false)` / `set("dc", true)`.
    pub async fn set(&mut self, id: &str, on: bool) -> Result<()> {
        match id {
            "ac" | "ac_output" => self.set_ac_output(on).await,
            "dc" | "dc_output" => self.set_dc_output(on).await,
            other => Err(Error::Protocol(format!("unknown control '{other}'"))),
        }
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    async fn read_block(&mut self, start: u16, count: u16) -> Result<Vec<u8>> {
        let req = modbus_lite::build_read(UNIT, start, count);
        log::debug!("bluetti tx reg={start:#06x}: {}", hex(&req));
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
                    log::debug!("bluetti rx reg={start:#06x}: {}", hex(&buf[..total]));
                    return Ok(buf[..total].to_vec());
                }
            }
        }
        Err(Error::Timeout)
    }

    /// Write a single register (function 0x06) and consume the echo.
    async fn write_register(&mut self, addr: u16, value: u16) -> Result<()> {
        let req = modbus_lite::build_write_single(UNIT, addr, value);
        log::debug!("bluetti tx write {addr}={value}: {}", hex(&req));
        self.transport.write(&req).await?;
        // Response echoes the request (fixed 8 bytes); read one frame to drain.
        let _ = self.transport.read_frame().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BATTERY_COUNT, CORE_COUNT};
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

    fn block(start: u16, regs: &[(u16, u16)], count: u16) -> Vec<u8> {
        let mut data = vec![0u8; count as usize * 2];
        for &(a, v) in regs {
            let off = (a - start) as usize * 2;
            data[off] = (v >> 8) as u8;
            data[off + 1] = v as u8;
        }
        let mut f = vec![0x01u8, 0x03, (count as usize * 2) as u8];
        f.extend_from_slice(&data);
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[tokio::test]
    async fn reads_snapshot() {
        let core = block(CORE_START, &[(38, 350), (43, 90), (48, 1)], CORE_COUNT);
        let batt = block(BATTERY_START, &[(92, 5236), (105, 327)], BATTERY_COUNT);
        let scripted = Scripted {
            responses: VecDeque::from(vec![core, batt]),
            pending: VecDeque::new(),
        };
        let mut b = Bluetti::with_transport(Box::new(scripted)).await.unwrap();
        let d = b.read().await.unwrap();
        assert_eq!(d.total_battery_percent, 90);
        assert_eq!(d.ac_output_power, 350);
        assert!(d.ac_output_on);
        assert!((d.total_battery_voltage - 52.36).abs() < 1e-2);
        assert_eq!(d.cells.len(), 1);
    }
}

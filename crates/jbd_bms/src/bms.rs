//! High-level JBD device handle: connect, read a snapshot, control the FETs.

use crate::error::{Error, Result};
use crate::protocol::{
    self, BasicInfo, FrameAssembler, Response, REG_BASIC, REG_CELLS,
};
use crate::transport::Transport;

/// A decoded pack snapshot.
#[derive(Debug, Clone, Default)]
pub struct JbdData {
    pub basic: BasicInfo,
    pub cells: Vec<f32>,
}

/// A connected JBD / Xiaoxiang / Overkill BMS.
pub struct JbdBms {
    transport: Box<dyn Transport>,
    asm: FrameAssembler,
    data: JbdData,
    identity: ble_util::Identity,
}

impl JbdBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>) -> Result<Self> {
        transport.open().await?;
        let identity = transport.identity();
        Ok(Self {
            transport,
            asm: FrameAssembler::new(),
            data: JbdData::default(),
            identity,
        })
    }

    /// Static device identity from the BLE Device Information Service.
    pub fn identity(&self) -> &ble_util::Identity {
        &self.identity
    }

    /// Connect over BLE to the peripheral id from [`crate::scan`].
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::BluetoothTransport::new(id))).await
    }

    /// Connect over serial: `"<path>"` or `"<path>,<baud>"`.
    #[cfg(feature = "serial")]
    pub async fn connect_serial(target: &str) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::SerialTransport::from_target(
            target,
        )))
        .await
    }

    /// Cell count reported by the last read (0 until first `read`).
    pub fn model(&self) -> String {
        if self.data.basic.cell_count > 0 {
            format!("JBD {}S", self.data.basic.cell_count)
        } else {
            "JBD".to_string()
        }
    }

    /// Read a fresh snapshot (basic info + per-cell voltages).
    pub async fn read(&mut self) -> Result<&JbdData> {
        let basic = self.request(&protocol::read_reg(REG_BASIC), REG_BASIC).await?;
        self.data.basic = protocol::parse_basic(&basic.data)
            .map_err(|e| Error::Protocol(e.to_string()))?;

        let cells = self.request(&protocol::read_reg(REG_CELLS), REG_CELLS).await?;
        self.data.cells = protocol::parse_cells(&cells.data);

        Ok(&self.data)
    }

    /// Enable/disable the charge and discharge MOSFETs (both are set at once).
    pub async fn set_mosfet(&mut self, charge: bool, discharge: bool) -> Result<()> {
        let frame = protocol::set_mosfet(charge, discharge);
        self.transport.write(&frame).await?;
        self.data.basic.charging = charge;
        self.data.basic.discharging = discharge;
        Ok(())
    }

    /// Control by name — `set("charge", false)` / `set("discharge", true)`.
    /// Combines with the last-known state of the other FET.
    pub async fn set(&mut self, id: &str, on: bool) -> Result<()> {
        let (mut charge, mut discharge) =
            (self.data.basic.charging, self.data.basic.discharging);
        match id {
            "charge" | "charging" => charge = on,
            "discharge" | "discharging" => discharge = on,
            other => return Err(Error::Unsupported(format!("unknown control '{other}'"))),
        }
        self.set_mosfet(charge, discharge).await
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    /// Write a command, then read/assemble frames until the wanted register
    /// arrives. Bounded so a silent device errors instead of hanging.
    async fn request(&mut self, cmd: &[u8], want: u8) -> Result<Response> {
        self.transport.write(cmd).await?;
        let mut buf = [0u8; 512];
        for _ in 0..16 {
            let n = self.transport.read(&mut buf).await?;
            if n == 0 {
                continue;
            }
            self.asm.push(&buf[..n]);
            while let Some(frame) = self.asm.next_frame() {
                match protocol::decode(&frame) {
                    Ok(resp) if resp.register == want => {
                        if resp.ok {
                            return Ok(resp);
                        }
                        return Err(Error::Protocol(format!(
                            "device reported error for register {want:#x}"
                        )));
                    }
                    _ => {} // unrelated frame, keep reading
                }
            }
        }
        Err(Error::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::checksum;
    use async_trait::async_trait;
    use std::collections::VecDeque;

    /// A transport that replays canned reads, optionally fragmented.
    struct Scripted {
        chunks: VecDeque<Vec<u8>>,
    }

    fn resp(reg: u8, data: &[u8]) -> Vec<u8> {
        let mut payload = vec![0x00, data.len() as u8];
        payload.extend_from_slice(data);
        let [hi, lo] = checksum(&payload);
        let mut f = vec![0xDD, reg];
        f.extend_from_slice(&payload);
        f.extend_from_slice(&[hi, lo, 0x77]);
        f
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
            Ok(_data.len())
        }
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    #[tokio::test]
    async fn reads_snapshot_across_fragments() {
        // basic-info payload: 4S/1NTC, 13.2 V, soc 55
        let mut d = vec![0u8; 25];
        d[0..2].copy_from_slice(&1320u16.to_be_bytes());
        d[19] = 55;
        d[20] = 0x03;
        d[21] = 4;
        d[22] = 1;
        d[23..25].copy_from_slice(&2981u16.to_be_bytes());
        let basic = resp(REG_BASIC, &d);

        let mut c = Vec::new();
        for mv in [3300u16, 3301, 3299, 3302] {
            c.extend_from_slice(&mv.to_be_bytes());
        }
        let cells = resp(REG_CELLS, &c);

        // Fragment the basic-info frame across two reads; a stray heartbeat first.
        let mut chunks = VecDeque::new();
        chunks.push_back(vec![0x00, 0xAA]); // junk / heartbeat
        chunks.push_back(basic[..5].to_vec());
        chunks.push_back(basic[5..].to_vec());
        chunks.push_back(cells);

        let mut bms = JbdBms::with_transport(Box::new(Scripted { chunks }))
            .await
            .unwrap();
        let data = bms.read().await.unwrap();
        assert_eq!(data.basic.soc, 55);
        assert!((data.basic.voltage - 13.2).abs() < 1e-3);
        assert_eq!(data.cells.len(), 4);
        assert_eq!(bms.model(), "JBD 4S");
    }
}

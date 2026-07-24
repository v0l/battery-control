//! High-level SOK device handle: connect and read a snapshot.

use crate::error::{Error, Result};
use crate::protocol::{
    self, SokData, CMD_DETAIL, CMD_INFO, HDR_CAP, HDR_CELLS, HDR_INFO, HDR_TEMP,
};
use crate::transport::Transport;
use std::collections::HashMap;

/// A connected SOK battery.
pub struct SokBms {
    transport: Box<dyn Transport>,
    frames: HashMap<u16, Vec<u8>>,
    data: SokData,
}

impl SokBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>) -> Result<Self> {
        transport.open().await?;
        Ok(Self {
            transport,
            frames: HashMap::new(),
            data: SokData::default(),
        })
    }

    /// Connect over BLE to the peripheral id from [`crate::scan`].
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::BluetoothTransport::new(id))).await
    }

    pub fn model(&self) -> &str {
        "SOK"
    }

    /// Read a fresh snapshot. `CMD_INFO` yields the info+temperature frames;
    /// `CMD_DETAIL` yields the capacity+cells frames.
    pub async fn read(&mut self) -> Result<&SokData> {
        self.collect(CMD_INFO, &[HDR_INFO, HDR_TEMP]).await?;
        self.collect(CMD_DETAIL, &[HDR_CAP, HDR_CELLS]).await?;

        self.data = SokData::from_frames(
            self.frame(HDR_INFO)?,
            self.frame(HDR_TEMP)?,
            self.frame(HDR_CAP)?,
            self.frame(HDR_CELLS)?,
        )
        .ok_or_else(|| Error::Protocol("failed to parse SOK frames".into()))?;
        Ok(&self.data)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    fn frame(&self, hdr: u16) -> Result<&[u8]> {
        self.frames
            .get(&hdr)
            .map(|v| v.as_slice())
            .ok_or_else(|| Error::Protocol(format!("missing response {hdr:#06x}")))
    }

    /// Send `cmd`, then read notification frames until every wanted header has
    /// been captured (or a bounded number of reads elapse).
    async fn collect(&mut self, cmd: u8, wanted: &[u16]) -> Result<()> {
        self.transport.write(&protocol::command(cmd)).await?;
        for _ in 0..16 {
            if wanted.iter().all(|h| self.frames.contains_key(h)) {
                return Ok(());
            }
            let frame = self.transport.read_frame().await?;
            if frame.is_empty() {
                continue;
            }
            if let Some(hdr) = protocol::header(&frame) {
                if wanted.contains(&hdr) {
                    self.frames.insert(hdr, frame);
                }
            }
        }
        if wanted.iter().all(|h| self.frames.contains_key(h)) {
            Ok(())
        } else {
            Err(Error::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;

    struct Scripted {
        // frames delivered per read, in order
        frames: VecDeque<Vec<u8>>,
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
        async fn read_frame(&mut self) -> Result<Vec<u8>> {
            Ok(self.frames.pop_front().unwrap_or_default())
        }
    }

    fn frame(hdr: u16, body: &[(usize, u8)]) -> Vec<u8> {
        let mut f = vec![0u8; 20];
        f[0] = (hdr >> 8) as u8;
        f[1] = hdr as u8;
        for &(i, v) in body {
            f[i] = v;
        }
        f
    }

    #[tokio::test]
    async fn reads_snapshot() {
        // info: soc 60 at offset 16
        let info = frame(HDR_INFO, &[(16, 60)]);
        let temp = frame(HDR_TEMP, &[(5, 21)]);
        let cap = frame(HDR_CAP, &[(6, 0x32)]); // 100 Ah
        let mut cells = frame(HDR_CELLS, &[]);
        for (x, (idx, mv)) in [(1u8, 3300u16), (2, 3300), (3, 3300), (4, 3300)]
            .into_iter()
            .enumerate()
        {
            cells[2 + x * 4] = idx;
            cells[3 + x * 4..5 + x * 4].copy_from_slice(&mv.to_le_bytes());
        }
        // Interleave an unrelated frame to prove header matching.
        let noise = frame(0xCCF1, &[]);
        let frames = VecDeque::from(vec![info, noise, temp, cap, cells]);

        let mut bms = SokBms::with_transport(Box::new(Scripted { frames }))
            .await
            .unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.soc, 60);
        assert_eq!(d.cells.len(), 4);
        assert!((d.voltage - 13.2).abs() < 1e-2);
        assert!((d.capacity - 100.0).abs() < 1e-2);
    }
}

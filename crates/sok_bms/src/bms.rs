//! High-level SOK device handle: connect and read a snapshot. Supports both
//! the `0xEE` command generation and the ABC-BMS Modbus generation, dispatching
//! on the variant the transport detected at connect time.

use crate::data::{SokData, Variant};
use crate::error::{Error, Result};
use crate::transport::Transport;
use crate::{abc, ee};
use std::collections::HashMap;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A connected SOK battery.
pub struct SokBms {
    transport: Box<dyn Transport>,
    variant: Variant,
    frames: HashMap<u16, Vec<u8>>, // EE: collected response frames by header
    data: SokData,
}

impl SokBms {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>) -> Result<Self> {
        transport.open().await?;
        let variant = transport.variant();
        Ok(Self {
            transport,
            variant,
            frames: HashMap::new(),
            data: SokData::default(),
        })
    }

    /// Connect over BLE to the peripheral id from [`crate::scan`].
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str) -> Result<Self> {
        Self::with_transport(Box::new(crate::transport::BluetoothTransport::new(id))).await
    }

    /// Which protocol generation this device speaks.
    pub fn variant(&self) -> Variant {
        self.variant
    }

    pub fn model(&self) -> String {
        self.data
            .model
            .clone()
            .unwrap_or_else(|| "SOK".to_string())
    }

    /// Read a fresh snapshot.
    pub async fn read(&mut self) -> Result<&SokData> {
        self.data = match self.variant {
            Variant::Ee => self.read_ee().await?,
            Variant::Abc => self.read_abc().await?,
        };
        Ok(&self.data)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    // --- EE: send C1/C2, collect header-tagged notification frames ---

    async fn read_ee(&mut self) -> Result<SokData> {
        use ee::{CMD_DETAIL, CMD_INFO, HDR_CAP, HDR_CELLS, HDR_INFO, HDR_TEMP};
        self.frames.clear();
        self.collect_ee(CMD_INFO, &[HDR_INFO, HDR_TEMP]).await?;
        self.collect_ee(CMD_DETAIL, &[HDR_CAP, HDR_CELLS]).await?;

        let f = |h: u16| -> Result<&[u8]> {
            self.frames
                .get(&h)
                .map(|v| v.as_slice())
                .ok_or_else(|| Error::Protocol(format!("missing response {h:#06x}")))
        };
        ee::from_frames(f(HDR_INFO)?, f(HDR_TEMP)?, f(HDR_CAP)?, f(HDR_CELLS)?)
            .ok_or_else(|| Error::Protocol("failed to parse SOK frames".into()))
    }

    async fn collect_ee(&mut self, cmd: u8, wanted: &[u16]) -> Result<()> {
        self.transport.write(&ee::command(cmd)).await?;
        for _ in 0..16 {
            if wanted.iter().all(|h| self.frames.contains_key(h)) {
                return Ok(());
            }
            let frame = self.transport.read_frame().await?;
            if frame.is_empty() {
                continue;
            }
            if let Some(hdr) = ee::header(&frame) {
                log::debug!("sok ee rx {hdr:#06x}: {}", hex(&frame));
                if wanted.contains(&hdr) {
                    self.frames.insert(hdr, frame);
                }
            }
        }
        wanted
            .iter()
            .all(|h| self.frames.contains_key(h))
            .then_some(())
            .ok_or(Error::Timeout)
    }

    // --- ABC: one Modbus read, reassembled across notifications ---

    async fn read_abc(&mut self) -> Result<SokData> {
        let req = abc::build_read(abc::TELEMETRY_START, abc::TELEMETRY_COUNT);
        log::debug!("sok abc tx: {}", hex(&req));
        self.transport.write(&req).await?;

        let mut buf = Vec::new();
        for _ in 0..64 {
            let chunk = self.transport.read_frame().await?;
            if chunk.is_empty() {
                if buf.is_empty() {
                    continue;
                }
                break; // silence after some data: give up on this attempt
            }
            buf.extend_from_slice(&chunk);
            if let Some(total) = abc::response_len(&buf) {
                if buf.len() >= total {
                    log::debug!("sok abc rx ({} bytes): {}", total, hex(&buf[..total]));
                    let regs = abc::parse_response(&buf[..total], abc::TELEMETRY_START)
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    return Ok(abc::decode_telemetry(&regs));
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
    use std::collections::VecDeque;

    struct Scripted {
        variant: Variant,
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
        fn variant(&self) -> Variant {
            self.variant
        }
    }

    fn ee_frame(hdr: u16, body: &[(usize, u8)]) -> Vec<u8> {
        let mut f = vec![0u8; 20];
        f[0] = (hdr >> 8) as u8;
        f[1] = hdr as u8;
        for &(i, v) in body {
            f[i] = v;
        }
        f
    }

    #[tokio::test]
    async fn reads_ee_snapshot() {
        use ee::{HDR_CAP, HDR_CELLS, HDR_INFO, HDR_TEMP};
        let info = ee_frame(HDR_INFO, &[(16, 60)]);
        let temp = ee_frame(HDR_TEMP, &[(5, 21)]);
        let cap = ee_frame(HDR_CAP, &[(6, 0x32)]);
        let mut cells = ee_frame(HDR_CELLS, &[]);
        for (x, idx) in [1u8, 2, 3, 4].into_iter().enumerate() {
            cells[2 + x * 4] = idx;
            cells[3 + x * 4..5 + x * 4].copy_from_slice(&3300u16.to_le_bytes());
        }
        let frames = VecDeque::from(vec![info, temp, cap, cells]);
        let mut bms = SokBms::with_transport(Box::new(Scripted {
            variant: Variant::Ee,
            frames,
        }))
        .await
        .unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.soc, 60);
        assert_eq!(d.cells.len(), 4);
        assert!((d.voltage - 13.2).abs() < 1e-2);
    }

    fn abc_response(start: u16, regs: &[(u16, u16)]) -> Vec<u8> {
        let max = regs.iter().map(|(a, _)| *a).max().unwrap();
        let count = (max - start + 1) as usize;
        let mut data = vec![0u8; count * 2];
        for &(a, v) in regs {
            let off = (a - start) as usize * 2;
            data[off] = (v >> 8) as u8;
            data[off + 1] = v as u8;
        }
        let mut frame = vec![0x01u8, 0x03, (count * 2) as u8];
        frame.extend_from_slice(&data);
        let crc = abc::crc16(&frame);
        frame.push(crc as u8);
        frame.push((crc >> 8) as u8);
        frame
    }

    #[tokio::test]
    async fn reads_abc_snapshot_across_fragments() {
        let full = abc_response(
            0x0080,
            &[
                (0x0080, 200),
                (0x0081, 1320),
                (0x0082, 77),
                (0x0085, 10000),
                (0x0091, 4),
                (0x0095, 240),
                (0x009b, 3300),
                (0x009c, 3300),
                (0x009d, 3300),
                (0x009e, 3300),
            ],
        );
        // Deliver the Modbus response split across three notifications.
        let mut frames = VecDeque::new();
        frames.push_back(full[..2].to_vec());
        frames.push_back(full[2..15].to_vec());
        frames.push_back(full[15..].to_vec());
        let mut bms = SokBms::with_transport(Box::new(Scripted {
            variant: Variant::Abc,
            frames,
        }))
        .await
        .unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.soc, 77);
        assert!((d.voltage - 13.2).abs() < 1e-2);
        assert!((d.current - 2.0).abs() < 1e-2);
        assert_eq!(d.cells.len(), 4);
        assert!((d.temperature - 24.0).abs() < 1e-2);
    }
}

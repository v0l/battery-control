//! High-level Pylontech RS485 console handle: read the module chain.

use crate::error::{Error, Result};
use crate::protocol::{self, PylontechData, CID2_ANALOG};
use crate::transport::Transport;

/// A connected Pylontech RS485 console (the whole module chain on the bus).
pub struct PylontechRs485 {
    transport: Box<dyn Transport>,
    address: u8,
    data: PylontechData,
}

impl PylontechRs485 {
    /// Wrap an already-constructed transport (mainly for tests).
    pub async fn with_transport(mut transport: Box<dyn Transport>, address: u8) -> Result<Self> {
        transport.open().await?;
        Ok(Self {
            transport,
            address,
            data: PylontechData::default(),
        })
    }

    /// Open over the RS485 console port (`path`, `baud` 115200, `address`).
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

    pub fn module_count(&self) -> usize {
        self.data.modules.len()
    }

    /// Read analog values for every module on the bus (CID2 `0x42`, info `FF`).
    pub async fn read(&mut self) -> Result<&PylontechData> {
        let req = protocol::encode(self.address, CID2_ANALOG, b"FF");
        log::debug!("pylontech tx: {}", String::from_utf8_lossy(&req));
        self.transport.write(&req).await?;

        // A stray line can precede the answer; retry a few reads.
        for _ in 0..4 {
            let line = self.transport.read_line().await?;
            if line.is_empty() {
                break;
            }
            log::debug!("pylontech rx: {}", String::from_utf8_lossy(&line));
            match protocol::decode(&line) {
                Ok(resp) => {
                    self.data = protocol::parse_analog(&resp.info)
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    return Ok(&self.data);
                }
                Err(_) => continue,
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
    use async_trait::async_trait;
    use std::collections::VecDeque;

    struct Scripted {
        lines: VecDeque<Vec<u8>>,
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
        async fn read_line(&mut self) -> Result<Vec<u8>> {
            Ok(self.lines.pop_front().unwrap_or_default())
        }
    }

    const FRAME: &[u8] = b"~20024600914211030F0CE70CE80CE60CE70CE80CE80CE80CE60CE50CE60CE80CE70CEA0CE50CE6050B910B870B870B870B87FFE6C18982DC02C350001F0F0CE20CE60CE60CE10CE50CE70CE60CE30CE20CE50CE30CE90CE70CE90CE9050B910B870B870B870B87FFE7C17082DC02C350001F0F0CE20CE50CE50CE20CE30CE30CE40CE50CE60CE60CE30CE40CE40CE60CE6050B910B7D0B7D0B7D0B7DFFE5C16082DC02C350001FB476\r";

    #[tokio::test]
    async fn reads_chain() {
        // A junk line precedes the real frame.
        let lines = VecDeque::from(vec![b"garbage\r".to_vec(), FRAME.to_vec()]);
        let mut bms = PylontechRs485::with_transport(Box::new(Scripted { lines }), 2)
            .await
            .unwrap();
        let d = bms.read().await.unwrap();
        assert_eq!(d.modules.len(), 3);
        assert!((d.soc() - 67.0).abs() < 1.0);
        assert_eq!(bms.module_count(), 3);
    }
}

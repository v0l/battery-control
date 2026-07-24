//! High-level Jackery handle: connect with the advertisement-derived key, read
//! status, toggle outputs. RC4 path (most portable models).

use crate::crypto::Rc4Cipher;
use crate::data::{self, JackeryData};
use crate::error::{Error, Result};
use crate::transport::Transport;
use crate::{command, model_name};

/// A connected Jackery power station.
pub struct Jackery {
    transport: Box<dyn Transport>,
    cipher: Rc4Cipher,
    identity: ble_util::Identity,
    model: u16,
    serial: String,
    data: JackeryData,
}

fn security_byte() -> u8 {
    rand::random::<u8>() | 1 // never 0
}

impl Jackery {
    /// Wrap an already-constructed transport with an explicit key (for tests).
    pub async fn with_transport(
        mut transport: Box<dyn Transport>,
        key: Vec<u8>,
        model: u16,
        serial: String,
    ) -> Result<Self> {
        transport.open().await?;
        let identity = transport.identity();
        Ok(Self {
            transport,
            cipher: Rc4Cipher::new(key),
            identity,
            model,
            serial,
            data: JackeryData::default(),
        })
    }

    /// Connect over BLE using the key derived from the advertisement (see
    /// [`crate::scan`]).
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str, key: Vec<u8>, model: u16, serial: String) -> Result<Self> {
        Self::with_transport(
            Box::new(crate::transport::BluetoothTransport::new(id)),
            key,
            model,
            serial,
        )
        .await
    }

    pub fn identity(&self) -> &ble_util::Identity {
        &self.identity
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
    pub fn model(&self) -> String {
        model_name(self.model)
    }

    /// Read the current device status.
    pub async fn read(&mut self) -> Result<&JackeryData> {
        let payload = self.request(&command::query_device_property()).await?;
        self.data = data::parse(&payload)
            .ok_or_else(|| Error::Protocol("no JSON in response".into()))?;
        Ok(&self.data)
    }

    /// Toggle an output — `set("ac", true)`, `set("dc", false)`, `set("usb", …)`,
    /// `set("car", …)`.
    pub async fn set(&mut self, id: &str, on: bool) -> Result<()> {
        let cmd = match id {
            "ac" => command::set_ac_output(on),
            "dc" => command::set_dc_output(on),
            "usb" => command::set_dc_usb_output(on),
            "car" => command::set_dc_car_output(on),
            other => return Err(Error::Protocol(format!("unknown control '{other}'"))),
        };
        let enc = self.cipher.encrypt(&cmd, security_byte());
        self.transport.write(&enc).await?;
        // Best-effort: drain one response frame (the ack).
        let _ = self.transport.read_frame().await;
        Ok(())
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }

    /// Encrypt+send a command and return the decrypted response payload,
    /// reassembling notifications until it decrypts (CRC-valid).
    async fn request(&mut self, command: &[u8]) -> Result<Vec<u8>> {
        let enc = self.cipher.encrypt(command, security_byte());
        self.transport.write(&enc).await?;

        let mut buf = Vec::new();
        for _ in 0..32 {
            let chunk = self.transport.read_frame().await?;
            if chunk.is_empty() {
                if buf.is_empty() {
                    continue;
                }
                break;
            }
            buf.extend_from_slice(&chunk);
            if let Some(payload) = self.cipher.decrypt(&buf) {
                return Ok(payload);
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

    /// A loopback transport that answers a query by RC4-wrapping a canned JSON
    /// status with the same cipher, proving the encrypt→decrypt→parse path.
    struct Loopback {
        cipher: Rc4Cipher,
        pending: VecDeque<Vec<u8>>,
    }

    #[async_trait]
    impl Transport for Loopback {
        async fn open(&mut self) -> Result<()> {
            Ok(())
        }
        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
        async fn write(&mut self, _data: &[u8]) -> Result<usize> {
            // Build a response: DFEC00 FC 03 <len> <json>, wrapped like the device.
            let json = br#"{"rb":72,"ip":120,"op":0,"bt":230,"oac":0,"odc":1}"#;
            let mut resp = vec![0xDF, 0xEC, 0x00, 0xFC, 0x03, json.len() as u8];
            resp.extend_from_slice(json);
            let enc = self.cipher.encrypt(&resp, 0x33);
            // deliver split across two notifications
            let mid = enc.len() / 2;
            self.pending.push_back(enc[..mid].to_vec());
            self.pending.push_back(enc[mid..].to_vec());
            Ok(_data.len())
        }
        async fn read_frame(&mut self) -> Result<Vec<u8>> {
            Ok(self.pending.pop_front().unwrap_or_default())
        }
    }

    #[tokio::test]
    async fn reads_status_over_rc4() {
        let key = b"a-22-byte-jackery-keyyy".to_vec();
        let transport = Loopback {
            cipher: Rc4Cipher::new(key.clone()),
            pending: VecDeque::new(),
        };
        let mut j = Jackery::with_transport(Box::new(transport), key, 9, "J1234567890ABCD".into())
            .await
            .unwrap();
        let d = j.read().await.unwrap();
        assert_eq!(d.rb, 72);
        assert!((d.temperature_c() - 23.0).abs() < 1e-9);
        assert_eq!(d.input_power() as i64, 120);
        assert!(!d.ac_on() && d.dc_on());
        assert_eq!(j.model(), "E240");
    }
}

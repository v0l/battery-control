//! High-level EcoFlow handle: run the auth handshake over a transport, then read
//! decoded telemetry packets. Encrypted models only (HD31 / Y711).

use crate::error::{Error, Result};
use crate::packet::split_enc_frames;
use crate::protobuf::{decode_hd31, Telemetry};
use crate::session::Handshake;
use crate::transport::{model_name, Transport};
use crate::Packet;

/// A connected, authenticated EcoFlow device.
pub struct Ecoflow {
    transport: Box<dyn Transport>,
    hs: Handshake,
    identity: ble_util::Identity,
    serial: String,
    buf: Vec<u8>,
    telem: Telemetry,
}

impl Ecoflow {
    /// Connect over BLE and complete the auth handshake. `user_id` is the account
    /// id (one-time, from the app); everything else is local.
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(id: &str, serial: &str, user_id: &str) -> Result<Self> {
        let transport = Box::new(crate::transport::BluetoothTransport::new(id));
        Self::with_transport(transport, serial, user_id).await
    }

    /// Run the handshake over an arbitrary transport (also used in tests).
    pub async fn with_transport(
        mut transport: Box<dyn Transport>,
        serial: &str,
        user_id: &str,
    ) -> Result<Self> {
        transport.open().await?;
        let identity = transport.identity();

        let (mut hs, first) = Handshake::start(serial, user_id);
        transport.write(&first).await?;

        let mut buf: Vec<u8> = Vec::new();
        // Drive the handshake: read → advance → write, until Ready.
        for _ in 0..64 {
            let chunk = transport.read_frame().await?;
            if chunk.is_empty() {
                if hs.is_ready() {
                    break;
                }
                return Err(Error::Timeout);
            }
            buf.extend_from_slice(&chunk);
            let (frames, rest) = split_enc_frames(&buf);
            if frames.is_empty() {
                continue; // wait for more bytes
            }
            match hs.on_notification(&buf) {
                Ok(Some(next)) => {
                    buf = rest;
                    transport.write(&next).await?;
                }
                Ok(None) => {
                    buf = rest;
                    if hs.is_ready() {
                        break;
                    }
                }
                Err(e) => return Err(Error::Protocol(e.to_string())),
            }
        }
        if !hs.is_ready() {
            return Err(Error::Protocol("handshake did not complete".into()));
        }

        Ok(Self {
            transport,
            hs,
            identity,
            serial: serial.to_string(),
            buf: Vec::new(),
            telem: Telemetry::default(),
        })
    }

    pub fn identity(&self) -> &ble_util::Identity {
        &self.identity
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
    pub fn model(&self) -> &'static str {
        model_name(&self.serial)
    }

    /// Read the next batch of telemetry packets, updating the cached status.
    /// Returns the decoded inner packets (for inspection / field mapping).
    pub async fn read_packets(&mut self) -> Result<Vec<Packet>> {
        let chunk = self.transport.read_frame().await?;
        if chunk.is_empty() {
            return Ok(Vec::new());
        }
        self.buf.extend_from_slice(&chunk);
        let (_frames, rest) = split_enc_frames(&self.buf);
        let packets = self.hs.decode_packets(&self.buf);
        self.buf = rest;

        for p in &packets {
            // HD31 push (cmd_set 0x0C, cmd_id 0x20/0x21) carries backup/SOC info.
            if p.cmd_set == 0x0C && (p.cmd_id == 0x20 || p.cmd_id == 0x21) {
                let t = decode_hd31(&p.payload);
                if t.soc.is_some() {
                    self.telem = t;
                }
            }
        }
        Ok(packets)
    }

    /// Poll until telemetry with an SOC arrives (or the read window elapses).
    pub async fn read(&mut self) -> Result<&Telemetry> {
        for _ in 0..8 {
            self.read_packets().await?;
            if self.telem.soc.is_some() {
                break;
            }
        }
        Ok(&self.telem)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.close().await
    }
}

//! Async BLE session with a SOLIX power station: discovery, the ECDH
//! negotiation handshake, encrypted command transmission and telemetry receive.

use crate::crypto::{
    decrypt, derive_shared_secret, encrypt, gcm_decrypt_noverify, gcm_decrypt_noverify_nonce, gcm_encrypt, gcm_encrypt_nonce, SECURE_KEY,
};
use crate::error::{AnkerError, Result};
use crate::model::{Model, Telemetry};
use crate::protocol::*;
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// A discovered SOLIX device advertisement.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub name: String,
    /// The BLE MAC address. **Note:** macOS does not expose the MAC and reports
    /// `00:00:00:00:00:00`; use [`Discovered::id`] for a stable identifier.
    pub address: String,
    /// A stable, per-host hardware identifier: the MAC on Linux, or the
    /// CoreBluetooth peripheral UUID on macOS (where the MAC is unavailable).
    /// Unique per device even when names collide.
    pub id: String,
    pub rssi: Option<i16>,
    pub model: Model,
    peripheral: Peripheral,
}

type NotifStream = Pin<Box<dyn Stream<Item = btleplug::api::ValueNotification> + Send>>;

/// A negotiated session with a power station.
pub struct Device {
    peripheral: Peripheral,
    cmd_char: btleplug::api::Characteristic,
    notifications: NotifStream,
    model: Model,
    name: String,
    shared_secret: [u8; 32],
    /// Reference instant for the replay-protection timestamp (set at stage 3).
    negotiated_at: Instant,
    fragmenter: Fragmenter,
    /// Session AES-128-GCM key once the *secure* (gen-2 settings) channel is
    /// negotiated. Present only after [`Device::negotiate_secure`].
    secure_key: Option<[u8; 16]>,
    /// Session GCM nonce (derived from the ECDH secret post-handshake).
    secure_nonce: [u8; 12],
}

async fn first_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    adapters.into_iter().next().ok_or(AnkerError::NoAdapter)
}

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("static UUID is valid")
}

/// Scan for nearby SOLIX / Prime devices for `secs` seconds.
pub async fn scan(secs: u64) -> Result<Vec<Discovered>> {
    let adapter = first_adapter().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(secs)).await;

    let identifier = uuid(UUID_IDENTIFIER);
    let mut found = Vec::new();
    for p in adapter.peripherals().await? {
        let props = match p.properties().await? {
            Some(props) => props,
            None => continue,
        };
        let advertises_anker = props.services.contains(&identifier);
        let name = props.local_name.clone().unwrap_or_default();
        // Some stacks don't surface the service UUID; fall back to name hints.
        let looks_anker = advertises_anker
            || name.to_ascii_uppercase().contains("ANKER")
            || name.to_ascii_uppercase().contains("SOLIX");
        if !looks_anker {
            continue;
        }
        let address = p.address().to_string();
        found.push(Discovered {
            model: Model::detect(&name),
            name,
            id: stable_id(&address, &p),
            address,
            rssi: props.rssi,
            peripheral: p,
        });
    }
    let _ = adapter.stop_scan().await;
    Ok(found)
}

/// Compute a stable hardware id: prefer a real MAC, else the platform peripheral
/// id (a UUID on macOS), stripped of btleplug's `PeripheralId(...)` wrapper.
fn stable_id(address: &str, peripheral: &Peripheral) -> String {
    if !address.is_empty() && address != "00:00:00:00:00:00" {
        return address.to_string();
    }
    let dbg = format!("{:?}", btleplug::api::Peripheral::id(peripheral));
    dbg.trim_start_matches("PeripheralId")
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim_matches('"')
        .to_string()
}

impl Discovered {
    /// Connect and run the full encryption handshake, returning a ready session.
    ///
    /// Connects to the exact discovered peripheral, so it works even on macOS
    /// where the MAC address is unavailable.
    pub async fn connect(self) -> Result<Device> {
        Device::connect(self.peripheral, self.name, self.model).await
    }

    /// Connect using the secure (AES-128-GCM) channel for settings writes.
    pub async fn connect_secure(self) -> Result<Device> {
        Device::connect_secure(self.peripheral, self.name, self.model).await
    }
}

impl Device {
    pub fn model(&self) -> Model {
        self.model
    }
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Find a device by name substring (case-insensitive) or MAC address and connect.
    pub async fn find_and_connect(target: &str, scan_secs: u64) -> Result<Device> {
        let want = target.to_ascii_lowercase();
        let devices = scan(scan_secs).await?;
        let chosen = devices
            .into_iter()
            .find(|d| {
                d.address.to_ascii_lowercase() == want
                    || d.name.to_ascii_lowercase().contains(&want)
            })
            .ok_or_else(|| AnkerError::DeviceNotFound(target.to_string()))?;
        chosen.connect().await
    }

    /// Like [`Device::find_and_connect`] but negotiates the secure GCM channel.
    pub async fn find_and_connect_secure(target: &str, scan_secs: u64) -> Result<Device> {
        let want = target.to_ascii_lowercase();
        let chosen = scan(scan_secs)
            .await?
            .into_iter()
            .find(|d| {
                d.address.to_ascii_lowercase() == want
                    || d.name.to_ascii_lowercase().contains(&want)
            })
            .ok_or_else(|| AnkerError::DeviceNotFound(target.to_string()))?;
        chosen.connect_secure().await
    }

    async fn connect(peripheral: Peripheral, name: String, model: Model) -> Result<Device> {
        let mut dev = Self::setup(peripheral, name, model).await?;
        dev.negotiate().await?;
        if let Some((cmd, payload)) = dev.model.subscribe_command() {
            dev.send_command(cmd, payload).await?;
        }
        Ok(dev)
    }

    /// Connect and negotiate the **secure** (AES-128-GCM) channel instead of the
    /// basic one — required for gen-2 settings writes.
    pub async fn connect_secure(
        peripheral: Peripheral,
        name: String,
        model: Model,
    ) -> Result<Device> {
        let mut dev = Self::setup(peripheral, name, model).await?;
        dev.negotiate_secure().await?;
        if let Some((cmd, payload)) = dev.model.subscribe_command() {
            dev.send_secure(cmd, payload).await?;
        }
        Ok(dev)
    }

    async fn setup(peripheral: Peripheral, name: String, model: Model) -> Result<Device> {
        if !peripheral.is_connected().await? {
            peripheral.connect().await?;
        }
        peripheral.discover_services().await?;

        let chars = peripheral.characteristics();
        let cmd_char = chars
            .iter()
            .find(|c| c.uuid == uuid(UUID_COMMAND))
            .cloned()
            .ok_or(AnkerError::CharacteristicNotFound("command"))?;
        let telemetry_char = chars
            .iter()
            .find(|c| c.uuid == uuid(UUID_TELEMETRY))
            .cloned()
            .ok_or(AnkerError::CharacteristicNotFound("telemetry"))?;

        peripheral.subscribe(&telemetry_char).await?;
        let notifications = peripheral.notifications().await?;

        let mut dev = Device {
            peripheral,
            cmd_char,
            notifications,
            model,
            name,
            shared_secret: [0u8; 32],
            negotiated_at: Instant::now(),
            fragmenter: Fragmenter::new(),
            secure_key: None,
            secure_nonce: crate::crypto::SECURE_NONCE,
        };

        Ok(dev)
    }

    // Takes `&mut self` (not `&self`) so the returned future captures `&mut
    // Device` rather than `&Device`; the latter would require `Device: Sync`,
    // which fails because the boxed notification stream is `Send` but not `Sync`.
    async fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.peripheral
            .write(&self.cmd_char, bytes, WriteType::WithResponse)
            .await?;
        Ok(())
    }

    /// Fire-and-forget write (no ATT response wait). Used for control commands:
    /// some SOLIX firmwares don't send a write-response for output-control
    /// writes, so a with-response write blocks forever. The command's effect is
    /// confirmed out-of-band via the telemetry stream instead.
    async fn write_raw_no_ack(&mut self, bytes: &[u8]) -> Result<()> {
        self.peripheral
            .write(&self.cmd_char, bytes, WriteType::WithoutResponse)
            .await?;
        Ok(())
    }

    /// Run the fixed ECDH handshake until an encrypted session is established.
    async fn negotiate(&mut self) -> Result<()> {
        self.write_raw(&hex::decode(NEGOTIATION_COMMAND_0).unwrap())
            .await?;

        let overall = Duration::from_secs(90);
        let start = Instant::now();

        loop {
            if start.elapsed() > overall {
                return Err(AnkerError::NegotiationTimeout);
            }
            let notif = match tokio::time::timeout(
                Duration::from_secs(15),
                self.notifications.next(),
            )
            .await
            {
                Ok(Some(n)) => n,
                Ok(None) => return Err(AnkerError::NegotiationTimeout),
                Err(_) => return Err(AnkerError::NegotiationTimeout),
            };

            let packet = match split_packet(&notif.value) {
                Ok(p) => p,
                Err(e) => {
                    log::debug!("ignoring malformed packet during negotiation: {e}");
                    continue;
                }
            };

            if packet.pattern != PATTERN_NEGOTIATION {
                log::debug!("non-negotiation packet {:02x?} ignored", packet.pattern);
                continue;
            }

            if self.handle_negotiation_stage(&packet).await? {
                log::info!("encrypted session negotiated with '{}'", self.name);
                return Ok(());
            }
        }
    }

    /// Handle one negotiation packet. Returns `true` once the session is ready.
    async fn handle_negotiation_stage(&mut self, packet: &Packet) -> Result<bool> {
        match packet.cmd {
            [0x08, 0x01] => {
                self.write_raw(&hex::decode(NEGOTIATION_COMMAND_1).unwrap())
                    .await?;
            }
            [0x08, 0x03] => {
                self.write_raw(&hex::decode(NEGOTIATION_COMMAND_2).unwrap())
                    .await?;
            }
            [0x08, 0x29] => {
                // Timestamp reference for replay protection is set here.
                self.negotiated_at = Instant::now();
                self.write_raw(&hex::decode(NEGOTIATION_COMMAND_3).unwrap())
                    .await?;
            }
            [0x08, 0x05] => {
                self.write_raw(&hex::decode(NEGOTIATION_COMMAND_4).unwrap())
                    .await?;
            }
            [0x08, 0x21] => {
                let params = parse_params(&packet.payload);
                let pubkey = params
                    .get("a1")
                    .ok_or_else(|| AnkerError::Crypto("stage 5 missing device pubkey".into()))?;
                self.shared_secret = derive_shared_secret(pubkey)?;
                self.write_raw(&hex::decode(NEGOTIATION_COMMAND_5).unwrap())
                    .await?;
                return Ok(true);
            }
            other => {
                log::debug!("unexpected negotiation cmd {:02x?}", other);
            }
        }
        Ok(false)
    }

    /// Send an encrypted command. The replay-protection timestamp and trailer
    /// are appended before encryption.
    ///
    /// Takes `&mut self` so that the returned future is `Send` even though the
    /// notification stream is not `Sync`; this lets `Device` be driven from
    /// `Send` async contexts (e.g. trait objects).
    pub async fn send_command(&mut self, cmd: [u8; 2], payload: &[u8]) -> Result<()> {
        let packet = self.build_command_packet(cmd, payload);
        self.write_raw(&packet).await
    }

    /// Like [`send_command`](Self::send_command) but fire-and-forget (no ATT
    /// response wait). For output-control commands the device confirms via
    /// telemetry, and some firmwares never send a write-response.
    pub async fn send_command_no_ack(&mut self, cmd: [u8; 2], payload: &[u8]) -> Result<()> {
        let packet = self.build_command_packet(cmd, payload);
        self.write_raw_no_ack(&packet).await
    }

    fn build_command_packet(&self, cmd: [u8; 2], payload: &[u8]) -> Vec<u8> {
        let elapsed = self.negotiated_at.elapsed().as_secs() as u32;
        let timestamp = BASE_TIMESTAMP_LE.wrapping_add(elapsed);

        let mut full = Vec::with_capacity(payload.len() + 7);
        full.extend_from_slice(payload);
        full.extend_from_slice(&[0xfe, 0x05, 0x03]);
        full.extend_from_slice(&timestamp.to_le_bytes());

        let ciphertext = encrypt(&self.shared_secret, &full);
        build_packet(PATTERN_ENCRYPTED_TX, cmd, &ciphertext)
    }

    /// Read the next framed packet from the notification stream (any pattern).
    pub async fn recv_packet(&mut self, timeout: Duration) -> Result<Packet> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AnkerError::ResponseTimeout);
            }
            let notif = match tokio::time::timeout(remaining, self.notifications.next()).await {
                Ok(Some(n)) => n,
                _ => return Err(AnkerError::ResponseTimeout),
            };
            match split_packet(&notif.value) {
                Ok(p) => return Ok(p),
                Err(e) => {
                    log::debug!("ignoring malformed packet: {e}");
                    continue;
                }
            }
        }
    }

    /// Negotiate the gen-2 **secure** (AES-128-GCM) channel required for
    /// settings writes (charge/discharge limits, etc.).
    ///
    /// The ECDH handshake frames (cmd `40xx`) are GCM-encrypted with the fixed
    /// bootstrap key; after the pubkey exchange the session key = ECDH shared
    /// secret, used for all subsequent `send_secure` commands.
    pub async fn negotiate_secure(&mut self) -> Result<()> {
        // Handshake step plaintexts (guest / no-login).
        let steps: &[(u16, &str)] = &[
            (0x4001, "a1047de5606a"),
            (0x4003, "a1047de5606aa30120a40200f0"),
            (0x4029, "a1047de5606a"),
            (0x4005, "a1047ee5606aa30120a402fd00a50144a60102"),
        ];
        for (cmd, pt_hex) in steps {
            self.send_secure_neg(*cmd, &hex::decode(pt_hex).unwrap()).await?;
            let _resp = self.recv_packet(Duration::from_secs(10)).await?;
            log::debug!("secure handshake {cmd:#06x} -> resp {:02x?}", _resp.cmd);
        }

        // Send our ECDH public key (cmd 4021), read the device's pubkey (4821).
        let pubkey = crate::crypto::client_public_key();
        let mut body = vec![0xa1, 0x40];
        body.extend_from_slice(&pubkey);
        self.send_secure_neg(0x4021, &body).await?;

        let resp = self.recv_packet(Duration::from_secs(10)).await?;
        // Device response tag isn't verifiable; recover the payload via CTR.
        let pt = gcm_decrypt_noverify(&SECURE_KEY, &resp.payload);
        let params = parse_params(pt.strip_prefix(&[0x00]).unwrap_or(&pt));
        let dev_pub = params
            .get("a1")
            .ok_or_else(|| AnkerError::Crypto("secure stage missing device pubkey".into()))?;
        self.shared_secret = derive_shared_secret(dev_pub)?;
        // Session GCM key = low 16 bytes of the ECDH shared secret (same split
        // as the basic CBC channel); nonce + AAD stay the secure constants.
        let key: [u8; 16] = self.shared_secret[..16].try_into().unwrap();
        // Session nonce = next 12 bytes of the ECDH secret (GCM nonce length).
        self.secure_nonce = self.shared_secret[16..28].try_into().unwrap();
        self.secure_key = Some(key);
        self.negotiated_at = Instant::now();
        log::info!("secure GCM session established with '{}'", self.name);
        Ok(())
    }

    /// Build + send a GCM negotiation frame (pattern `030001`) with the
    /// bootstrap key.
    async fn send_secure_neg(&mut self, cmd: u16, plaintext: &[u8]) -> Result<()> {
        let ct = gcm_encrypt(&SECURE_KEY, plaintext);
        let packet = build_packet(PATTERN_NEGOTIATION, cmd.to_be_bytes(), &ct);
        self.write_raw(&packet).await
    }

    /// True once [`Device::negotiate_secure`] has established the session key.
    pub fn is_secure(&self) -> bool {
        self.secure_key.is_some()
    }

    /// Send a **session-key GCM** frame on the *negotiation* channel (pattern
    /// `030001`) and return the device's decrypted `(cmd, plaintext)` response.
    ///
    /// Distinct from [`Device::send_secure`], which uses the
    /// encrypted-application channel (`03000f`).
    pub async fn send_secure_neg_session(
        &mut self,
        cmd: [u8; 2],
        plaintext: &[u8],
        timeout: Duration,
    ) -> Result<([u8; 2], Vec<u8>)> {
        let key = self
            .secure_key
            .ok_or_else(|| AnkerError::Crypto("secure session not negotiated".into()))?;
        let ct = gcm_encrypt_nonce(&key, &self.secure_nonce, plaintext);
        let packet = build_packet(PATTERN_NEGOTIATION, cmd, &ct);
        self.write_raw(&packet).await?;
        let resp = self.recv_packet(timeout).await?;
        let pt = gcm_decrypt_noverify_nonce(&key, &self.secure_nonce, &resp.payload);
        Ok((resp.cmd, pt))
    }

    /// Send an encrypted command over the **secure** GCM session (call
    /// [`Device::negotiate_secure`] first). Payload is GCM-encrypted with the
    /// session key.
    pub async fn send_secure(&mut self, cmd: [u8; 2], payload: &[u8]) -> Result<()> {
        let key = self
            .secure_key
            .ok_or_else(|| AnkerError::Crypto("secure session not negotiated".into()))?;
        let ct = gcm_encrypt_nonce(&key, &self.secure_nonce, payload);
        let packet = build_packet(PATTERN_ENCRYPTED_TX, cmd, &ct);
        self.write_raw(&packet).await
    }

    /// Send a secure command and return the device's decrypted response
    /// `(cmd, plaintext)` (the ack echoes cmd with the `0x08` response bit).
    pub async fn send_secure_recv(
        &mut self,
        cmd: [u8; 2],
        payload: &[u8],
        timeout: Duration,
    ) -> Result<([u8; 2], Vec<u8>)> {
        let key = self
            .secure_key
            .ok_or_else(|| AnkerError::Crypto("secure session not negotiated".into()))?;
        self.send_secure(cmd, payload).await?;
        // The ack echoes our cmd with the 0x08 response bit set on the high byte.
        let want = [cmd[0] | 0x08, cmd[1]];
        let deadline = Instant::now() + timeout;
        loop {
            let resp = self
                .recv_packet(deadline.saturating_duration_since(Instant::now()))
                .await?;
            if resp.cmd == want {
                return Ok((resp.cmd, gcm_decrypt_noverify_nonce(&key, &self.secure_nonce, &resp.payload)));
            }
            log::debug!("skip non-ack {:02x?} waiting for {:02x?}", resp.cmd, want);
        }
    }

    /// Wait for the next decoded telemetry snapshot (up to `timeout`).
    pub async fn next_telemetry(&mut self, timeout: Duration) -> Result<Telemetry> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AnkerError::ResponseTimeout);
            }
            let notif =
                match tokio::time::timeout(remaining, self.notifications.next()).await {
                    Ok(Some(n)) => n,
                    _ => return Err(AnkerError::ResponseTimeout),
                };
            if let Some(t) = self.process_session_packet(&notif.value)? {
                return Ok(t);
            }
        }
    }

    /// Decode a session (telemetry) notification, returning telemetry when a
    /// complete, decrypted snapshot is available.
    fn process_session_packet(&mut self, raw: &[u8]) -> Result<Option<Telemetry>> {
        let packet = match split_packet(raw) {
            Ok(p) => p,
            Err(e) => {
                log::debug!("ignoring malformed telemetry packet: {e}");
                return Ok(None);
            }
        };

        log::debug!(
            "rx pattern={:02x?} cmd={:02x?} len={}",
            packet.pattern,
            packet.cmd,
            packet.payload.len()
        );
        let is_session =
            packet.pattern == PATTERN_SESSION_A || packet.pattern == PATTERN_SESSION_B;
        if !is_session {
            return Ok(None);
        }

        // Plaintext telemetry (older firmware).
        if packet.cmd == [0x03, 0x00] {
            let params = parse_params(&packet.payload);
            return Ok(Some(Telemetry::from_params(self.model, &params)));
        }

        // Encrypted telemetry. On the secure channel the device uses its own
        // (GCM) command codes and cipher; accept any session cmd and GCM-decrypt.
        let secure = self.secure_key.is_some();
        if secure || self.model.telemetry_commands().contains(&packet.cmd) {
            if let Some(body) = self.fragmenter.push(packet.cmd, &packet.payload) {
                let plaintext = match self.secure_key {
                    Some(key) => gcm_decrypt_noverify_nonce(&key, &self.secure_nonce, &body),
                    None => decrypt(&self.shared_secret, &body)?,
                };
                let params = parse_params(&plaintext);
                if std::env::var("ANKER_DUMP").is_ok() {
                    let mut keys: Vec<_> = params.keys().cloned().collect();
                    keys.sort();
                    for k in keys {
                        eprintln!("RAW {k} = {}", hex::encode(&params[&k]));
                    }
                    eprintln!("---");
                }
                return Ok(Some(Telemetry::from_params(self.model, &params)));
            }
        } else {
            log::debug!("unhandled session cmd {:02x?}", packet.cmd);
        }
        Ok(None)
    }

    /// Turn the AC output on or off.
    pub async fn set_ac(&mut self, on: bool) -> Result<()> {
        self.send_command_no_ack(self.model.cmd_ac_output(), crate::model::on_off_payload(on))
            .await
    }

    /// Turn the DC (12 V) output on or off.
    pub async fn set_dc(&mut self, on: bool) -> Result<()> {
        self.send_command_no_ack(self.model.cmd_dc_output(), crate::model::on_off_payload(on))
            .await
    }

    /// Set the LED light-bar brightness/mode.
    ///
    /// Supported on gen-1 models; the gen-2 command code is not yet known, so
    /// this returns [`AnkerError::UnsupportedModel`] on gen-2.
    pub async fn set_light(&mut self, brightness: crate::model::Brightness) -> Result<()> {
        let cmd = self.model.cmd_light_mode().ok_or_else(|| {
            AnkerError::UnsupportedModel(format!(
                "light control not known for {:?}",
                self.model
            ))
        })?;
        self.send_command_no_ack(cmd, &crate::model::light_mode_payload(brightness))
            .await
    }

    /// Turn the LCD display on or off.
    ///
    /// Supported on gen-1 models; the gen-2 command code is not yet known, so
    /// this returns [`AnkerError::UnsupportedModel`] on gen-2.
    pub async fn set_display(&mut self, on: bool) -> Result<()> {
        let cmd = self.model.cmd_display_on_off().ok_or_else(|| {
            AnkerError::UnsupportedModel(format!(
                "display control not known for {:?}",
                self.model
            ))
        })?;
        self.send_command_no_ack(cmd, crate::model::on_off_payload(on))
            .await
    }

    /// Disconnect from the device.
    pub async fn disconnect(&mut self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }
}

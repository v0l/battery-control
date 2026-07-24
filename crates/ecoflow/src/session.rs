//! The BLE auth handshake as a sans-IO state machine, so it can be driven by any
//! transport and unit-tested without Bluetooth. Sequence (from `ef-ble-reverse`):
//!
//! 1. **pubkey exchange** — send our secp160r1 public key; receive the device's,
//!    derive the ECDH shared key + IV.
//! 2. **key info** — request the session material; decrypt it with the shared key
//!    and derive the AES **session key**.
//! 3. **auth status** — a session-encrypted status probe.
//! 4. **auth** — send `md5(user_id + serial)`; the device replies `00` on success.
//!
//! After [`Handshake`] reaches [`Stage::Ready`], telemetry packets arrive
//! session-encrypted and are decoded with [`Handshake::decode_packets`].

use crate::crypto::{
    aes_cbc_decrypt, aes_cbc_encrypt, auth_secret, gen_session_key, md5,
};
use crate::packet::{enc_packet, split_enc_frames, FrameType, Packet};
use crate::secp160r1::KeyPair;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for the device public key (response to our pubkey).
    PubKey,
    /// Waiting for the session-key material.
    KeyInfo,
    /// Waiting for the auth-status reply.
    AuthStatus,
    /// Waiting for the auth reply.
    Auth,
    /// Handshake complete — telemetry flows.
    Ready,
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("unexpected/short response in stage {0:?}")]
    BadResponse(Stage),
    #[error("ecdh shared-secret failed")]
    Ecdh,
    #[error("session-key derivation failed")]
    SessionKey,
    #[error("auth rejected by device")]
    AuthRejected,
}

pub struct Handshake {
    keypair: KeyPair,
    serial: String,
    user_id: String,
    is_xor: bool,
    shared_key: Option<[u8; 16]>,
    iv: Option<[u8; 16]>,
    session_key: Option<[u8; 16]>,
    stage: Stage,
}

impl Handshake {
    /// Begin a handshake for `serial`, authenticating with account `user_id`.
    /// Returns the state machine and the first frame to write.
    pub fn start(serial: &str, user_id: &str) -> (Self, Vec<u8>) {
        Self::start_with(KeyPair::generate(), serial, user_id)
    }

    /// Same as [`start`](Self::start) but with a supplied keypair (for tests).
    pub fn start_with(keypair: KeyPair, serial: &str, user_id: &str) -> (Self, Vec<u8>) {
        // Payload: 0x01 0x00 prefix + our raw public key.
        let mut payload = vec![0x01, 0x00];
        payload.extend_from_slice(&keypair.public_bytes());
        let frame = enc_packet(FrameType::Command, &payload);
        let hs = Self {
            keypair,
            serial: serial.to_string(),
            user_id: user_id.to_string(),
            is_xor: serial.starts_with("Y711"),
            shared_key: None,
            iv: None,
            session_key: None,
            stage: Stage::PubKey,
        };
        (hs, frame)
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn is_ready(&self) -> bool {
        self.stage == Stage::Ready
    }

    /// Wrap an inner [`Packet`] as a session-encrypted outer frame ready to send.
    pub fn seal(&self, packet: &Packet) -> Option<Vec<u8>> {
        let key = self.session_key.as_ref()?;
        let iv = self.iv.as_ref()?;
        let enc = aes_cbc_encrypt(key, iv, &packet.to_bytes());
        Some(enc_packet(FrameType::Protocol, &enc))
    }

    /// Feed a raw BLE notification. Returns the next frame to write (if any). When
    /// `Ok(None)` is returned in [`Stage::Ready`] there is nothing more to send.
    pub fn on_notification(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, HandshakeError> {
        let (frames, _rest) = split_enc_frames(data);
        let payload = frames
            .into_iter()
            .next()
            .ok_or(HandshakeError::BadResponse(self.stage))?;
        match self.stage {
            Stage::PubKey => self.on_pubkey(&payload).map(Some),
            Stage::KeyInfo => self.on_keyinfo(&payload).map(Some),
            Stage::AuthStatus => self.on_auth_status().map(Some),
            Stage::Auth => self.on_auth(data).map(|()| None),
            Stage::Ready => Ok(None),
        }
    }

    fn on_pubkey(&mut self, payload: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        // payload = [type, status, curve, dev_pubkey(40)...]
        if payload.len() < 3 + 40 {
            return Err(HandshakeError::BadResponse(Stage::PubKey));
        }
        let dev_pub = &payload[3..3 + 40];
        let shared = self
            .keypair
            .shared_secret(dev_pub)
            .ok_or(HandshakeError::Ecdh)?;
        self.iv = Some(md5(&shared));
        let mut sk = [0u8; 16];
        sk.copy_from_slice(&shared[..16]);
        self.shared_key = Some(sk);

        self.stage = Stage::KeyInfo;
        Ok(enc_packet(FrameType::Command, &[0x02]))
    }

    fn on_keyinfo(&mut self, payload: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        if payload.first() != Some(&0x02) || payload.len() < 2 {
            return Err(HandshakeError::BadResponse(Stage::KeyInfo));
        }
        let shared = self.shared_key.ok_or(HandshakeError::SessionKey)?;
        let iv = self.iv.ok_or(HandshakeError::SessionKey)?;
        let data = aes_cbc_decrypt(&shared, &iv, &payload[1..])
            .ok_or(HandshakeError::BadResponse(Stage::KeyInfo))?;
        if data.len() < 18 {
            return Err(HandshakeError::BadResponse(Stage::KeyInfo));
        }
        // srand = data[..16], seed = data[16..18]
        let session_key =
            gen_session_key(&data[16..18], &data[..16]).ok_or(HandshakeError::SessionKey)?;
        self.session_key = Some(session_key);

        self.stage = Stage::AuthStatus;
        let packet = Packet {
            version: 3,
            ..Packet::new(0x21, 0x35, 0x35, 0x89, Vec::new())
        };
        self.seal(&packet).ok_or(HandshakeError::SessionKey)
    }

    fn on_auth_status(&mut self) -> Result<Vec<u8>, HandshakeError> {
        // Any response advances us; send the auth secret.
        let payload = auth_secret(&self.user_id, &self.serial);
        let packet = Packet {
            version: 3,
            ..Packet::new(0x21, 0x35, 0x35, 0x86, payload)
        };
        self.stage = Stage::Auth;
        self.seal(&packet).ok_or(HandshakeError::SessionKey)
    }

    fn on_auth(&mut self, data: &[u8]) -> Result<(), HandshakeError> {
        for packet in self.decode_packets(data) {
            if packet.src == 0x35 && packet.cmd_set == 0x35 && packet.cmd_id == 0x86 {
                if packet.payload == [0x00] {
                    self.stage = Stage::Ready;
                    return Ok(());
                }
                return Err(HandshakeError::AuthRejected);
            }
        }
        // Some devices interleave other packets first; stay in Auth until we see it.
        Ok(())
    }

    /// Decrypt + parse all inner packets from a raw session-encrypted notification.
    pub fn decode_packets(&self, data: &[u8]) -> Vec<Packet> {
        let (Some(key), Some(iv)) = (self.session_key, self.iv) else {
            return Vec::new();
        };
        let (frames, _rest) = split_enc_frames(data);
        let mut out = Vec::new();
        for frame in frames {
            if let Some(plain) = aes_cbc_decrypt(&key, &iv, &frame) {
                if let Some(p) = Packet::from_bytes(&plain, self.is_xor) {
                    out.push(p);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigUint;

    /// A minimal loopback "device" that mirrors the real crypto so we can drive
    /// the whole handshake end-to-end without hardware.
    struct FakeDevice {
        keypair: KeyPair,
        shared: [u8; 16],
        iv: [u8; 16],
        session_key: [u8; 16],
        serial: String,
        user_id: String,
    }

    impl FakeDevice {
        fn new(serial: &str, user_id: &str) -> Self {
            Self {
                keypair: KeyPair::from_private(BigUint::from(0x9999u32)),
                shared: [0; 16],
                iv: [0; 16],
                session_key: [0; 16],
                serial: serial.into(),
                user_id: user_id.into(),
            }
        }

        /// Respond to our public key with the device's, matching the ECDH derivation.
        fn pubkey_reply(&mut self, our_pub: &[u8]) -> Vec<u8> {
            let shared = self.keypair.shared_secret(our_pub).unwrap();
            self.iv = md5(&shared);
            self.shared.copy_from_slice(&shared[..16]);
            // payload = [type, status, curve, dev_pub(40)]
            let mut payload = vec![0x01, 0x00, 0x00];
            payload.extend_from_slice(&self.keypair.public_bytes());
            enc_packet(FrameType::Command, &payload)
        }

        /// Respond to the key-info request with encrypted srand+seed.
        fn keyinfo_reply(&mut self) -> Vec<u8> {
            let srand = [0x42u8; 16];
            let seed = [0x0c, 0x01];
            self.session_key = gen_session_key(&seed, &srand).unwrap();
            let mut plain = srand.to_vec();
            plain.extend_from_slice(&seed);
            let enc = aes_cbc_encrypt(&self.shared, &self.iv, &plain);
            let mut payload = vec![0x02];
            payload.extend_from_slice(&enc);
            enc_packet(FrameType::Command, &payload)
        }

        fn seal(&self, packet: &Packet) -> Vec<u8> {
            let enc = aes_cbc_encrypt(&self.session_key, &self.iv, &packet.to_bytes());
            enc_packet(FrameType::Protocol, &enc)
        }

        fn auth_status_reply(&self) -> Vec<u8> {
            self.seal(&Packet::new(0x35, 0x21, 0x35, 0x89, vec![0x00]))
        }

        /// Verify the auth secret and reply success.
        fn auth_reply(&self, expect_secret: &[u8]) -> Vec<u8> {
            assert_eq!(expect_secret, auth_secret(&self.user_id, &self.serial));
            self.seal(&Packet::new(0x35, 0x21, 0x35, 0x86, vec![0x00]))
        }
    }

    #[test]
    fn full_handshake_roundtrip() {
        let serial = "HD31TESTSERIAL01";
        let user_id = "1234567890";
        let mut device = FakeDevice::new(serial, user_id);

        // Client starts with a fixed key so the test is deterministic.
        let (mut hs, first) =
            Handshake::start_with(KeyPair::from_private(BigUint::from(0x1234u32)), serial, user_id);
        assert_eq!(hs.stage(), Stage::PubKey);

        // Extract our public key from the first (Command) frame: 5A5A hdr(6) + [01 00] + pub.
        let (frames, _) = split_enc_frames(&first);
        let our_pub = &frames[0][2..2 + 40];

        // 1. pubkey → device replies with its pubkey
        let r = device.pubkey_reply(our_pub);
        let next = hs.on_notification(&r).unwrap().unwrap();
        assert_eq!(hs.stage(), Stage::KeyInfo);
        // client should now shared-match the device
        assert_eq!(hs.shared_key.unwrap(), device.shared);

        // 2. key info request → device replies with session material
        let _ = next; // the frame we'd send; device just answers the stage
        let r = device.keyinfo_reply();
        let _authstat_frame = hs.on_notification(&r).unwrap().unwrap();
        assert_eq!(hs.stage(), Stage::AuthStatus);
        assert_eq!(hs.session_key.unwrap(), device.session_key);

        // 3. auth status → device replies
        let r = device.auth_status_reply();
        let auth_frame = hs.on_notification(&r).unwrap().unwrap();
        assert_eq!(hs.stage(), Stage::Auth);

        // the auth frame we send must carry the correct secret; decrypt to verify
        let sent = hs.decode_packets(&auth_frame);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].cmd_id, 0x86);

        // 4. auth → success
        let r = device.auth_reply(&sent[0].payload);
        let done = hs.on_notification(&r).unwrap();
        assert!(done.is_none());
        assert!(hs.is_ready());

        // telemetry decodes with the shared session key
        let telem = device.seal(&Packet::new(0x0B, 0x21, 0x0C, 0x20, vec![9, 8, 7]));
        let packets = hs.decode_packets(&telem);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].cmd_set, 0x0C);
        assert_eq!(packets[0].payload, vec![9, 8, 7]);
    }
}

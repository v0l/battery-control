//! EcoFlow power station protocol — **local BLE, encrypted, no cloud**.
//!
//! This crate is being built in stages (ported from `rabits/ef-ble-reverse`):
//!
//! - **Stage 1 (this):** the deterministic protocol core — [`crc`] (CRC-16/ARC +
//!   CRC-8/SMBUS), [`packet`] framing ([`Packet`] + `0x5A5A` [`EncPacket`]),
//!   [`crypto`] (AES-128-CBC, MD5, session-key derivation, embedded login key),
//!   and [`secp160r1`] ECDH (EcoFlow's key-exchange curve, hand-implemented
//!   because no Rust crate provides it). All pure and unit-tested.
//! - **Stage 2 (planned):** BLE transport (write `0002`, notify `0003`), the
//!   full auth handshake (pubkey exchange → shared key → session key → auth via
//!   `md5(user_id + serial)`), then protobuf telemetry per device.
//!
//! Only the encrypted high-end models are in the reference: Smart Home Panel 2
//! (`HD31`) and Delta Pro Ultra (`Y711`). Auth also needs the account
//! `user_id` (one-time, extracted from the app/site); operation is fully local.

pub mod bms;
pub mod crc;
pub mod crypto;
pub mod error;
pub mod packet;
pub mod protobuf;
pub mod secp160r1;
pub mod session;
pub mod transport;

pub use bms::Ecoflow;
pub use error::{Error, Result};
pub use packet::{enc_packet, split_enc_frames, FrameType, Packet};
pub use protobuf::Telemetry;
pub use secp160r1::KeyPair;
pub use session::{Handshake, HandshakeError, Stage};
pub use transport::model_name;

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};

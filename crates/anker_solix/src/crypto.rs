//! Cryptography for the SOLIX BLE session.
//!
//! Anker firmware negotiates an ECDH (secp256r1 / NIST P-256) shared secret
//! and then encrypts all telemetry and command payloads with AES-128-CBC.
//!
//! The client's private key is a *fixed*, well-known scalar (the same one the
//! app ships with). Only the power station's ephemeral public key varies per
//! session, so the resulting shared secret is unique to each connection.
//!
//! The 32-byte shared secret is split: the first 16 bytes are the AES key and
//! the last 16 bytes are the IV.

use crate::error::{AnkerError, Result};
use aes::Aes128;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce, Tag};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use p256::ecdh::diffie_hellman;
use p256::{PublicKey, SecretKey};

type Aes128CbcEnc = cbc::Encryptor<Aes128>;
type Aes128CbcDec = cbc::Decryptor<Aes128>;

// --- Secure (gen-2) AES-128-GCM channel --------------------------------------
// The gen-2 negotiation handshake is GCM-encrypted with the fixed bootstrap
// constants below; after ECDH the derived session key replaces `SECURE_KEY`
// (nonce + AAD stay the same). Frame payload = ciphertext || 16-byte tag.

/// Hardcoded bootstrap key for the secure ECDH handshake.
pub const SECURE_KEY: [u8; 16] =
    [0xb8, 0xff, 0x74, 0x22, 0x95, 0x5d, 0x4e, 0xb6, 0xd5, 0x54, 0xa2, 0xc4, 0x70, 0x28, 0x05, 0x59];
/// Hardcoded 12-byte GCM nonce.
pub const SECURE_NONCE: [u8; 12] =
    [0x6b, 0xa3, 0xe3, 0xf2, 0xf3, 0xa6, 0x0f, 0x29, 0x71, 0xce, 0x5d, 0x1f];
/// Hardcoded 16-byte GCM associated data.
pub const SECURE_AAD: [u8; 16] =
    [0x33, 0x22, 0x11, 0x00, 0x77, 0x66, 0x55, 0x44, 0xbb, 0xaa, 0x99, 0x88, 0xff, 0xee, 0xdd, 0xcc];

/// AES-128-GCM encrypt with the hardcoded bootstrap nonce (handshake frames).
pub fn gcm_encrypt(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    gcm_encrypt_nonce(key, &SECURE_NONCE, plaintext)
}

/// AES-128-GCM encrypt with an explicit 12-byte nonce (AAD fixed). Returns
/// `ciphertext || tag(16)`.
pub fn gcm_encrypt_nonce(key: &[u8; 16], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes128Gcm::new(key.into());
    let mut buf = plaintext.to_vec();
    let tag = cipher
        .encrypt_in_place_detached(Nonce::from_slice(nonce), &SECURE_AAD, &mut buf)
        .expect("gcm encrypt");
    buf.extend_from_slice(&tag);
    buf
}

/// CTR-decrypt `ciphertext || tag(16)` **without** verifying the tag, returning
/// the plaintext. Device-to-host response frames carry a tag we don't verify;
/// the payload is still recoverable from the CTR keystream (GCM encryption is
/// AES-CTR starting at counter `nonce || 0x00000002`).
pub fn gcm_decrypt_noverify(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    gcm_decrypt_noverify_nonce(key, &SECURE_NONCE, data)
}

/// As [`gcm_decrypt_noverify`] but with an explicit nonce.
pub fn gcm_decrypt_noverify_nonce(key: &[u8; 16], nonce: &[u8; 12], data: &[u8]) -> Vec<u8> {
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockEncrypt, KeyInit};
    let ct = &data[..data.len().saturating_sub(16)];
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut out = Vec::with_capacity(ct.len());
    let mut counter: u32 = 2; // GCM encryption starts at J0+1 = nonce||0x00000002
    for chunk in ct.chunks(16) {
        let mut ks = [0u8; 16];
        ks[..12].copy_from_slice(nonce);
        ks[12..].copy_from_slice(&counter.to_be_bytes());
        let mut block = GenericArray::clone_from_slice(&ks);
        cipher.encrypt_block(&mut block);
        for (o, k) in chunk.iter().zip(block.iter()) {
            out.push(o ^ k);
        }
        counter += 1;
    }
    out
}

/// AES-128-GCM decrypt of `ciphertext || tag(16)` (nonce + AAD fixed).
pub fn gcm_decrypt(key: &[u8; 16], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 16 {
        return Err(AnkerError::Crypto("gcm frame shorter than tag".into()));
    }
    let (ct, tag) = data.split_at(data.len() - 16);
    let cipher = Aes128Gcm::new(key.into());
    let mut buf = ct.to_vec();
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&SECURE_NONCE),
            &SECURE_AAD,
            &mut buf,
            Tag::from_slice(tag),
        )
        .map_err(|_| AnkerError::Crypto("gcm tag verification failed".into()))?;
    Ok(buf)
}

/// The fixed client private key (secp256r1 scalar, big-endian) used for the
/// ECDH exchange. Lifted from the Anker app; hardcoding it is fine because the
/// "security" here only guards a ~10 m Bluetooth link.
pub const PRIVATE_KEY_HEX: &str =
    "7dfbea61cd95cee49c458ad7419e817f1ade9a66136de3c7d5787af1458e39f4";

/// XOR checksum over all bytes (used as the trailing packet byte).
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc ^ b)
}

/// Our client ECDH public key as the raw 64-byte `X||Y` point (no `0x04`
/// prefix), derived from the fixed private key. Sent in the secure handshake
/// (`4021`, parameter `a1`).
pub fn client_public_key() -> [u8; 64] {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let priv_bytes = hex::decode(PRIVATE_KEY_HEX).expect("valid priv hex");
    let secret = SecretKey::from_slice(&priv_bytes).expect("valid secret");
    let point = secret.public_key().to_encoded_point(false);
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(point.x().expect("x"));
    out[32..].copy_from_slice(point.y().expect("y"));
    out
}

/// Derive the 32-byte ECDH shared secret from the device's public key.
///
/// `device_pubkey_xy` is the raw 64-byte X||Y point as sent by the device in
/// negotiation stage 5 (parameter `a1`), *without* the `0x04` prefix.
pub fn derive_shared_secret(device_pubkey_xy: &[u8]) -> Result<[u8; 32]> {
    if device_pubkey_xy.len() != 64 {
        return Err(AnkerError::Crypto(format!(
            "expected 64-byte public key, got {}",
            device_pubkey_xy.len()
        )));
    }

    let priv_bytes =
        hex::decode(PRIVATE_KEY_HEX).map_err(|e| AnkerError::Crypto(e.to_string()))?;
    let secret =
        SecretKey::from_slice(&priv_bytes).map_err(|e| AnkerError::Crypto(e.to_string()))?;

    // Reassemble the uncompressed SEC1 point: 0x04 || X || Y.
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(device_pubkey_xy);
    let public = PublicKey::from_sec1_bytes(&sec1)
        .map_err(|e| AnkerError::Crypto(format!("bad device point: {e}")))?;

    let shared = diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
    let raw = shared.raw_secret_bytes();

    let mut out = [0u8; 32];
    out.copy_from_slice(raw.as_slice());
    Ok(out)
}

/// AES-128-CBC encrypt with PKCS7 padding. Key = `secret[..16]`, IV = `secret[16..]`.
pub fn encrypt(secret: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    Aes128CbcEnc::new(secret[..16].into(), secret[16..].into())
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

/// AES-128-CBC decrypt with PKCS7 unpadding.
pub fn decrypt(secret: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>> {
    Aes128CbcDec::new(secret[..16].into(), secret[16..].into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|e| AnkerError::Crypto(format!("decrypt/unpad failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_xor() {
        assert_eq!(checksum(&[0xff, 0x09, 0x00]), 0xf6);
        assert_eq!(checksum(&[]), 0x00);
    }

    #[test]
    fn aes_roundtrip() {
        let secret = [7u8; 32];
        let msg = b"a10121fe0503deadbeef";
        let ct = encrypt(&secret, msg);
        assert_eq!(ct.len() % 16, 0);
        let pt = decrypt(&secret, &ct).unwrap();
        assert_eq!(pt, msg);
    }

    #[test]
    fn ecdh_rejects_bad_length() {
        assert!(derive_shared_secret(&[0u8; 10]).is_err());
    }
}

#[cfg(test)]
mod gcm_tests {
    use super::*;

    #[test]
    fn gcm_matches_negotiation_frame() {
        // Plaintext of negotiation cmd 4001.
        let pt = hex::decode("a1047de5606a").unwrap();
        let out = gcm_encrypt(&SECURE_KEY, &pt);
        // Expected on-wire payload (ciphertext || tag).
        assert_eq!(hex::encode(out), "0a8242378650404c5437e809e45236456b0fb0870baa");
    }

    #[test]
    fn gcm_roundtrip() {
        let pt = b"hello secure world";
        let ct = gcm_encrypt(&SECURE_KEY, pt);
        assert_eq!(gcm_decrypt(&SECURE_KEY, &ct).unwrap(), pt);
    }
}

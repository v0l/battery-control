//! Minimal ECDH on the **secp160r1** curve — EcoFlow's key-exchange curve,
//! which no maintained Rust crate implements. Short-Weierstrass `y² = x³ + ax +
//! b (mod p)`, affine coordinates via `num-bigint`. Not constant-time; only used
//! for a one-time local BLE handshake, never for signing.

use num_bigint::{BigInt, BigUint, RandBigInt, Sign};
use num_traits::{One, Zero};

fn hex(s: &str) -> BigUint {
    BigUint::parse_bytes(s.as_bytes(), 16).unwrap()
}

// SEC 2 domain parameters for secp160r1.
fn p() -> BigUint {
    hex("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFF")
}
fn a() -> BigUint {
    hex("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFC")
}
#[cfg_attr(not(test), allow(dead_code))] // curve `b` used for on-curve checks
fn b() -> BigUint {
    hex("1C97BEFC54BD7A8B65ACF89F81D4D4ADC565FA45")
}
fn gx() -> BigUint {
    hex("4A96B5688EF573284664698968C38BB913CBFC82")
}
fn gy() -> BigUint {
    hex("23A628553168947D59DCC912042351377AC5FB32")
}
fn n() -> BigUint {
    hex("0100000000000000000001F4C8F927AED3CA752257")
}

/// The 160-bit field element size in bytes.
pub const FIELD_BYTES: usize = 20;

fn modp(x: BigInt, p: &BigUint) -> BigUint {
    let pi = BigInt::from_biguint(Sign::Plus, p.clone());
    let r = ((x % &pi) + &pi) % &pi;
    r.to_biguint().unwrap()
}

fn mod_inv(x: &BigUint, p: &BigUint) -> BigUint {
    // p is prime → x^(p-2) mod p
    x.modpow(&(p - 2u32), p)
}

/// An affine point (or the identity, `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point(pub Option<(BigUint, BigUint)>);

impl Point {
    pub fn identity() -> Self {
        Point(None)
    }
    pub fn generator() -> Self {
        Point(Some((gx(), gy())))
    }

    fn add(&self, other: &Point) -> Point {
        let p = p();
        let (x1, y1) = match &self.0 {
            None => return other.clone(),
            Some(v) => v.clone(),
        };
        let (x2, y2) = match &other.0 {
            None => return self.clone(),
            Some(v) => v.clone(),
        };
        if x1 == x2 && (y1.clone() + &y2) % &p == BigUint::zero() {
            return Point::identity();
        }
        let lambda = if x1 == x2 && y1 == y2 {
            // slope = (3 x1² + a) / (2 y1)
            let num = modp(
                BigInt::from(3u32) * BigInt::from_biguint(Sign::Plus, x1.modpow(&BigUint::from(2u32), &p))
                    + BigInt::from_biguint(Sign::Plus, a()),
                &p,
            );
            let den = mod_inv(&modp(BigInt::from(2u32) * BigInt::from_biguint(Sign::Plus, y1.clone()), &p), &p);
            (num * den) % &p
        } else {
            // slope = (y2 - y1) / (x2 - x1)
            let num = modp(
                BigInt::from_biguint(Sign::Plus, y2.clone()) - BigInt::from_biguint(Sign::Plus, y1.clone()),
                &p,
            );
            let den = mod_inv(
                &modp(
                    BigInt::from_biguint(Sign::Plus, x2.clone()) - BigInt::from_biguint(Sign::Plus, x1.clone()),
                    &p,
                ),
                &p,
            );
            (num * den) % &p
        };
        // x3 = λ² - x1 - x2 ; y3 = λ (x1 - x3) - y1
        let x3 = modp(
            BigInt::from_biguint(Sign::Plus, lambda.modpow(&BigUint::from(2u32), &p))
                - BigInt::from_biguint(Sign::Plus, x1.clone())
                - BigInt::from_biguint(Sign::Plus, x2),
            &p,
        );
        let y3 = modp(
            BigInt::from_biguint(Sign::Plus, lambda.clone())
                * (BigInt::from_biguint(Sign::Plus, x1) - BigInt::from_biguint(Sign::Plus, x3.clone()))
                - BigInt::from_biguint(Sign::Plus, y1),
            &p,
        );
        Point(Some((x3, y3)))
    }

    /// Scalar multiplication `k · self` (double-and-add).
    pub fn mul(&self, k: &BigUint) -> Point {
        let mut result = Point::identity();
        let mut addend = self.clone();
        let mut k = k.clone();
        while !k.is_zero() {
            if &k & BigUint::one() == BigUint::one() {
                result = result.add(&addend);
            }
            addend = addend.add(&addend);
            k >>= 1;
        }
        result
    }
}

fn to_field_bytes(v: &BigUint) -> [u8; FIELD_BYTES] {
    let mut out = [0u8; FIELD_BYTES];
    let bytes = v.to_bytes_be();
    let start = FIELD_BYTES.saturating_sub(bytes.len());
    out[start..].copy_from_slice(&bytes[bytes.len().saturating_sub(FIELD_BYTES)..]);
    out
}

/// An ECDH key pair.
pub struct KeyPair {
    pub private: BigUint,
    pub public: Point,
}

impl KeyPair {
    /// Generate a fresh key pair.
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let private = rng.gen_biguint_range(&BigUint::one(), &n());
        let public = Point::generator().mul(&private);
        Self { private, public }
    }

    /// The public key encoded as `x‖y` (40 bytes), matching `ecdsa`'s
    /// `VerifyingKey.to_string()`.
    pub fn public_bytes(&self) -> [u8; FIELD_BYTES * 2] {
        let (x, y) = self.public.0.clone().expect("non-identity public key");
        let mut out = [0u8; FIELD_BYTES * 2];
        out[..FIELD_BYTES].copy_from_slice(&to_field_bytes(&x));
        out[FIELD_BYTES..].copy_from_slice(&to_field_bytes(&y));
        out
    }

    /// Derive the ECDH shared secret with a peer public key (`x‖y`, 40 bytes).
    /// Returns the x-coordinate of `private · peer` as 20 big-endian bytes,
    /// matching `ecdsa.ECDH.generate_sharedsecret_bytes()`.
    pub fn shared_secret(&self, peer: &[u8]) -> Option<[u8; FIELD_BYTES]> {
        if peer.len() < FIELD_BYTES * 2 {
            return None;
        }
        let px = BigUint::from_bytes_be(&peer[..FIELD_BYTES]);
        let py = BigUint::from_bytes_be(&peer[FIELD_BYTES..FIELD_BYTES * 2]);
        let shared = Point(Some((px, py))).mul(&self.private);
        shared.0.map(|(x, _)| to_field_bytes(&x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_on_curve() {
        // y² == x³ + a·x + b (mod p)
        let p = p();
        let (x, y) = Point::generator().0.unwrap();
        let lhs = y.modpow(&BigUint::from(2u32), &p);
        let rhs = (x.modpow(&BigUint::from(3u32), &p) + a() * &x + b()) % &p;
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn ecdh_is_symmetric() {
        // The whole point: alice·B == bob·A
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();
        let s1 = alice.shared_secret(&bob.public_bytes()).unwrap();
        let s2 = bob.shared_secret(&alice.public_bytes()).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn order_kills_generator() {
        // n · G == identity
        assert_eq!(Point::generator().mul(&n()), Point::identity());
    }
}

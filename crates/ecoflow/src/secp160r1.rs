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
        Self::from_private(private)
    }

    /// Build a key pair from a fixed private scalar (`public = private·G`).
    pub fn from_private(private: BigUint) -> Self {
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

    fn hb(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Known-answer: scalar multiples `k·G` from an independent reference
    /// (python-ecdsa SECP160r1). Validates point add + double + double-and-add.
    #[test]
    fn scalar_multiples_kat() {
        // (k, x, y)
        let vectors: &[(u64, &str, &str)] = &[
            (2, "02f997f33c5ed04c55d3edf8675d3e92e8f46686", "f083a323482993e9440e817e21cfb7737df8797b"),
            (3, "7b76ff541ef363f2df13de1650bd48daa958bc59", "c915ca790d8c8877b55be0079d12854ffe9f6f5a"),
            (4, "b4041d8683be99f0afe01c307b1ad4c100cf2a88", "3f32caed841f08c00660cc74caf4a5bcf9beed08"),
            (5, "e705b180e41192ed772d1e2d424c171303ad6c4e", "933fbe35078c8c01465dbf40a12b583364b2a59c"),
            (7, "7a7f99d56472f619577c4e8c9b3a35e961472188", "8955c17a4aa7b3ca673c6d55ee00fae62552e356"),
            (255, "37185ec8fa9a39b4f72f11a2d0644ea39c217a00", "10ca9e4678fc07a8dec5ee6ca41a820887c9bfa9"),
            (65537, "63885dd8a634285d1a4bef41f070444cb8aff6d1", "cb0cfa766214439f8965d85a4b135bfb99751cdd"),
        ];
        for &(k, x, y) in vectors {
            let p = Point::generator().mul(&BigUint::from(k));
            let (px, py) = p.0.expect("finite point");
            assert_eq!(to_field_bytes(&px), hb(x)[..], "x mismatch for k={k}");
            assert_eq!(to_field_bytes(&py), hb(y)[..], "y mismatch for k={k}");
        }
    }

    // GEC 2 "Test Vectors for SEC 1" secp160r1 ECDH example, as embedded in
    // Nordic's nRF5 SDK (independent of python-ecdsa).
    const DU: &str = "aa374ffc3ce144e6b073307972cb6d57b2a4e982";
    const DV: &str = "45fb58a92a17ad4b15101c66e74f277e2b460866";
    const QU_X: &str = "51b4496fecc406ed0e75a24a3c03206251419dc0";
    const QU_Y: &str = "c28dcb4b73a514b468d793894f381ccc1756aa6c";
    const QV_X: &str = "49b41e0e9c0369c2328739d90f63d56707c6e5bc";
    const QV_Y: &str = "26e008b567015ed96d232a03111c3edc0e9c8f83";
    const Z: &str = "ca7c0f8c3ffa87a96e1b74ac8e6af594347bb40a";

    /// Known-answer: private keys derive the reference public points.
    #[test]
    fn gec2_pubkey_kat() {
        let u = KeyPair::from_private(hex(DU));
        assert_eq!(u.public_bytes()[..], [hb(QU_X), hb(QU_Y)].concat()[..]);
        let v = KeyPair::from_private(hex(DV));
        assert_eq!(v.public_bytes()[..], [hb(QV_X), hb(QV_Y)].concat()[..]);
    }

    /// Known-answer: the ECDH shared secret matches GEC 2, both directions.
    #[test]
    fn gec2_ecdh_kat() {
        let u = KeyPair::from_private(hex(DU));
        let v = KeyPair::from_private(hex(DV));
        let qv = [hb(QV_X), hb(QV_Y)].concat();
        let qu = [hb(QU_X), hb(QU_Y)].concat();
        assert_eq!(u.shared_secret(&qv).unwrap()[..], hb(Z)[..]);
        assert_eq!(v.shared_secret(&qu).unwrap()[..], hb(Z)[..]);
    }
}

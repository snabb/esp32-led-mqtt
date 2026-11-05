//! `mai-coap`'s crypto provider with the expensive parts done by an accelerator.
//!
//! P-256 point multiplication dominates an EDHOC handshake, and in software it stalls
//! everything sharing the executor, the LED animation included. AES-CCM stays in software
//! because the hardware has no such mode. HKDF does too: its inputs are short, and hardware
//! would need a hand-written HMAC holding key material for almost no gain.

use mai_coap::{
    Error,
    crypto::{CryptoProvider, Sha256State, SoftwareCrypto},
};
use rand_core::CryptoRng;
use zeroize::Zeroize;

/// The P-256 group order, big-endian, so that arrays compare as numbers.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];
const GENERATOR_X: [u8; 32] = [
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2,
    0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
];
const GENERATOR_Y: [u8; 32] = [
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16,
    0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];
/// `SoftwareCrypto` gives up after as many draws, and a miss has a chance of about 2^-32.
const KEY_DRAWS: usize = 128;

/// What the hardware does, using big-endian numbers as the provider trait does.
pub trait Accelerator {
    /// Must meet the erasure contract of `Sha256State`.
    type Sha256: Sha256State;

    fn sha256_start(&mut self) -> Result<Self::Sha256, Error>;

    /// `k` × (`x`, `y`) in affine coordinates. `k` is known to be in 1..n. Must reject a point
    /// that isn't on the curve, and shouldn't leave copies of `k` behind.
    fn multiply(
        &mut self,
        k: &[u8; 32],
        x: &[u8; 32],
        y: &[u8; 32],
    ) -> Result<([u8; 32], [u8; 32]), Error>;
}

pub struct AcceleratedCrypto<A, R> {
    accelerator: A,
    software: SoftwareCrypto<R>,
}

impl<A, R: CryptoRng> AcceleratedCrypto<A, R> {
    pub const fn new(accelerator: A, rng: R) -> Self {
        Self {
            accelerator,
            software: SoftwareCrypto::new(rng),
        }
    }
}

/// Zero and values of n or more aren't private keys, and hardware behavior on them is undefined.
fn in_range(k: &[u8; 32]) -> bool {
    *k != [0; 32] && *k < ORDER
}

impl<A: Accelerator, R: CryptoRng> CryptoProvider for AcceleratedCrypto<A, R> {
    type Sha256<'a>
        = A::Sha256
    where
        Self: 'a;

    fn sha256_start(&mut self) -> Result<Self::Sha256<'_>, Error> {
        self.accelerator.sha256_start()
    }
    fn hkdf_extract(&mut self, salt: Option<&[u8]>, input: &[u8]) -> Result<[u8; 32], Error> {
        self.software.hkdf_extract(salt, input)
    }
    fn hkdf_expand(&mut self, prk: &[u8; 32], info: &[u8], output: &mut [u8]) -> Result<(), Error> {
        self.software.hkdf_expand(prk, info, output)
    }
    fn encrypt(
        &mut self,
        key: &[u8; 16],
        nonce: &[u8; 13],
        aad: &[u8],
        plaintext: &mut [u8],
    ) -> Result<[u8; 8], Error> {
        self.software.encrypt(key, nonce, aad, plaintext)
    }
    fn decrypt(
        &mut self,
        key: &[u8; 16],
        nonce: &[u8; 13],
        aad: &[u8],
        ciphertext: &mut [u8],
        tag: &[u8; 8],
    ) -> Result<(), Error> {
        self.software.decrypt(key, nonce, aad, ciphertext, tag)
    }
    fn p256_key_pair(&mut self) -> Result<([u8; 32], [u8; 32]), Error> {
        for _ in 0..KEY_DRAWS {
            let mut private = [0; 32];
            self.software.random(&mut private);
            if in_range(&private) {
                let (x, mut y) = self
                    .accelerator
                    .multiply(&private, &GENERATOR_X, &GENERATOR_Y)
                    .inspect_err(|_| private.zeroize())?;
                y.zeroize();
                return Ok((private, x));
            }
            private.zeroize();
        }
        Err(Error::Protection)
    }
    fn p256_public_key(&mut self, private: &[u8; 32]) -> Result<[u8; 65], Error> {
        if !in_range(private) {
            return Err(Error::Protection);
        }
        let (x, y) = self
            .accelerator
            .multiply(private, &GENERATOR_X, &GENERATOR_Y)?;
        let mut point = [0; 65];
        point[0] = 4;
        point[1..33].copy_from_slice(&x);
        point[33..].copy_from_slice(&y);
        Ok(point)
    }
    fn p256_decompress(&mut self, x: &[u8; 32]) -> Result<[u8; 65], Error> {
        self.software.p256_decompress(x)
    }
    fn p256_validate(&mut self, point: &[u8]) -> Result<(), Error> {
        self.software.p256_validate(point)
    }
    fn p256_ecdh(&mut self, private: &[u8; 32], public_x: &[u8; 32]) -> Result<[u8; 32], Error> {
        if !in_range(private) {
            return Err(Error::Protection);
        }
        // Either y gives the same shared x. Decompressing also rejects an x not on the curve.
        let point = self.software.p256_decompress(public_x)?;
        let y = point[33..].try_into().map_err(|_| Error::Protection)?;
        let (mut shared, mut shared_y) = self.accelerator.multiply(private, public_x, &y)?;
        shared_y.zeroize();
        let result = shared;
        shared.zeroize();
        Ok(result)
    }
    fn random(&mut self, output: &mut [u8]) {
        self.software.random(output);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use core::{cell::Cell, convert::Infallible};
    use std::rc::Rc;

    use mai_coap::crypto::SoftwareSha256;
    use rand_core::{TryCryptoRng, TryRng};

    use super::*;

    /// Not for real use. Tests only need the two sides of a session to differ.
    pub(crate) struct TestRng(pub(crate) u64);
    impl TryRng for TestRng {
        type Error = Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Infallible> {
            Ok(self.try_next_u64()? as u32)
        }
        fn try_next_u64(&mut self) -> Result<u64, Infallible> {
            self.0 = self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
            Ok(self.0 ^ (self.0 >> 29))
        }
        fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Infallible> {
            for chunk in output.chunks_mut(8) {
                let bytes = self.try_next_u64()?.to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
            Ok(())
        }
    }
    impl TryCryptoRng for TestRng {}

    #[derive(Default)]
    pub(crate) struct Calls {
        pub(crate) hashes: Cell<u32>,
        pub(crate) multiplications: Cell<u32>,
    }

    /// Stands in for the hardware, using software that's tested elsewhere as the reference.
    pub(crate) struct FakeAccelerator {
        pub(crate) calls: Rc<Calls>,
        reference: SoftwareCrypto<TestRng>,
    }

    impl FakeAccelerator {
        pub(crate) fn new() -> Self {
            Self {
                calls: Rc::default(),
                reference: SoftwareCrypto::new(TestRng(99)),
            }
        }
    }

    impl Accelerator for FakeAccelerator {
        type Sha256 = SoftwareSha256;

        fn sha256_start(&mut self) -> Result<SoftwareSha256, Error> {
            self.calls.hashes.set(self.calls.hashes.get() + 1);
            self.reference.sha256_start()
        }

        fn multiply(
            &mut self,
            k: &[u8; 32],
            x: &[u8; 32],
            y: &[u8; 32],
        ) -> Result<([u8; 32], [u8; 32]), Error> {
            assert!(in_range(k), "the provider must check the scalar first");
            self.calls
                .multiplications
                .set(self.calls.multiplications.get() + 1);
            if *x == GENERATOR_X && *y == GENERATOR_Y {
                let point = self.reference.p256_public_key(k)?;
                Ok((
                    point[1..33].try_into().unwrap(),
                    point[33..].try_into().unwrap(),
                ))
            } else {
                // The reference only exposes the shared x, which is all ECDH reads.
                Ok((self.reference.p256_ecdh(k, x)?, [0; 32]))
            }
        }
    }

    fn accelerated() -> (AcceleratedCrypto<FakeAccelerator, TestRng>, Rc<Calls>) {
        let accelerator = FakeAccelerator::new();
        let calls = accelerator.calls.clone();
        (AcceleratedCrypto::new(accelerator, TestRng(1)), calls)
    }

    fn software() -> SoftwareCrypto<TestRng> {
        SoftwareCrypto::new(TestRng(2))
    }

    #[test]
    fn generator_is_the_p256_base_point() {
        // 1 × G is G: if a constant were mistyped this would disagree with the reference.
        let mut one = [0; 32];
        one[31] = 1;
        let point = software().p256_public_key(&one).unwrap();
        assert_eq!(point[1..33], GENERATOR_X);
        assert_eq!(point[33..], GENERATOR_Y);
    }

    #[test]
    fn public_keys_and_shared_secrets_match_software_and_use_the_accelerator() {
        let (mut crypto, calls) = accelerated();
        let (mine, my_x) = crypto.p256_key_pair().unwrap();
        assert_eq!(calls.multiplications.get(), 1);
        let public = crypto.p256_public_key(&mine).unwrap();
        assert_eq!(public, software().p256_public_key(&mine).unwrap());
        assert_eq!(public[1..33], my_x);

        let (theirs, their_x) = software().p256_key_pair().unwrap();
        let before = calls.multiplications.get();
        let shared = crypto.p256_ecdh(&mine, &their_x).unwrap();
        assert_eq!(calls.multiplications.get(), before + 1);
        assert_eq!(shared, software().p256_ecdh(&theirs, &my_x).unwrap());
        assert_ne!(shared, [0; 32]);
    }

    #[test]
    fn scalars_out_of_range_never_reach_the_accelerator() {
        let (mut crypto, calls) = accelerated();
        let mut too_big = ORDER;
        for bad in [[0; 32], ORDER, [0xff; 32]] {
            assert!(crypto.p256_public_key(&bad).is_err());
            assert!(crypto.p256_ecdh(&bad, &GENERATOR_X).is_err());
        }
        too_big[31] -= 1;
        assert!(crypto.p256_public_key(&too_big).is_ok(), "n - 1 is valid");
        assert_eq!(calls.multiplications.get(), 1);
    }

    #[test]
    fn an_x_not_on_the_curve_never_reaches_the_accelerator() {
        let (mut crypto, calls) = accelerated();
        let mut one = [0; 32];
        one[31] = 1;
        // About half of all values of x have no point on the curve.
        let off_curve = (1..=u8::MAX)
            .map(|low| {
                let mut x = [0; 32];
                x[31] = low;
                x
            })
            .find(|x| software().p256_decompress(x).is_err())
            .unwrap();
        assert!(crypto.p256_ecdh(&one, &off_curve).is_err());
        assert_eq!(calls.multiplications.get(), 0);
    }

    #[test]
    fn hashing_uses_the_accelerator() {
        let (mut crypto, calls) = accelerated();
        let digest = crypto.sha256(b"abc").unwrap();
        assert_eq!(calls.hashes.get(), 1);
        assert_eq!(digest, software().sha256(b"abc").unwrap());
    }

    #[test]
    fn authenticated_encryption_and_key_derivation_match_software() {
        let (mut crypto, calls) = accelerated();
        let (key, nonce) = ([7; 16], [9; 13]);
        let mut mine = *b"sixteen byte msg";
        let mut reference = mine;
        let tag = crypto.encrypt(&key, &nonce, b"aad", &mut mine).unwrap();
        assert_eq!(
            tag,
            software()
                .encrypt(&key, &nonce, b"aad", &mut reference)
                .unwrap()
        );
        assert_eq!(mine, reference);
        crypto
            .decrypt(&key, &nonce, b"aad", &mut mine, &tag)
            .unwrap();
        assert_eq!(&mine, b"sixteen byte msg");

        let prk = crypto.hkdf_extract(Some(b"salt"), b"input").unwrap();
        assert_eq!(
            prk,
            software().hkdf_extract(Some(b"salt"), b"input").unwrap()
        );
        assert_eq!(calls.multiplications.get(), 0);
        assert_eq!(calls.hashes.get(), 0, "HKDF stays in software");
    }
}

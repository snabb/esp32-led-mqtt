//! The device's own CoAP key: P-256 for EDHOC, stored in hex with the other settings.

use core::convert::Infallible;

use heapless::String;
use mai_coap::{
    Error,
    crypto::SoftwareCrypto,
    edhoc::{CcsEncoding, Identity},
    security::CryptoProvider,
};
use rand_core::{CryptoRng, TryCryptoRng, TryRng};

use crate::device_config::CoapCredential;

/// Bytes in a credential that fits a `CoapCredential`.
const MAX_CREDENTIAL_BYTES: usize = 128;

fn write_hex<const N: usize>(bytes: &[u8]) -> Result<String<N>, Error> {
    let mut output = String::new();
    for byte in bytes {
        heapless::format!(3; "{byte:02x}")
            .ok()
            .and_then(|pair: String<3>| output.push_str(&pair).ok())
            .ok_or(Error::Capacity)?;
    }
    Ok(output)
}

/// Decodes hex into `output`, returning the decoded part.
pub fn read_hex<'a>(hex: &str, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
    let digit = |b: u8| (b as char).to_digit(16).ok_or(Error::Malformed);
    let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
    let decoded = output.get_mut(..pairs.len()).ok_or(Error::Capacity)?;
    if !rest.is_empty() {
        return Err(Error::Malformed);
    }
    for (byte, pair) in decoded.iter_mut().zip(pairs) {
        *byte = (digit(pair[0])? << 4 | digit(pair[1])?) as u8;
    }
    Ok(decoded)
}

/// A new private key in hex.
pub fn generate_device_key(rng: impl CryptoRng) -> Result<String<64>, Error> {
    let (private, _) = SoftwareCrypto::new(rng).p256_key_pair()?;
    write_hex(&private)
}

/// The EDHOC identity for a private key in hex: an empty key ID and the x coordinate alone,
/// the shortest form. Clients must trust exactly these credential bytes.
pub fn identity(private_hex: &str, crypto: &mut impl CryptoProvider) -> Result<Identity, Error> {
    let mut private = [0; 32];
    if read_hex(private_hex, &mut private)?.len() != private.len() {
        return Err(Error::Malformed);
    }
    Identity::from_private_key(private, &[], CcsEncoding::CompactX, crypto)
}

/// Stands in for the generator `SoftwareCrypto` requires when only deriving a public key,
/// which draws nothing from it. Never use it where randomness is consumed.
struct NoRandomness;
impl TryRng for NoRandomness {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        Ok(0)
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok(0)
    }
    fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Infallible> {
        output.fill(0);
        Ok(())
    }
}
impl TryCryptoRng for NoRandomness {}

/// The public credential in hex for a private key in hex.
pub fn public_credential(private_hex: &str) -> Result<CoapCredential, Error> {
    let identity = identity(private_hex, &mut SoftwareCrypto::new(NoRandomness))?;
    let bytes = identity.credential_bytes();
    if bytes.len() > MAX_CREDENTIAL_BYTES {
        return Err(Error::Capacity);
    }
    write_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter(u8);
    impl TryRng for Counter {
        type Error = Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Infallible> {
            let mut bytes = [0; 4];
            self.try_fill_bytes(&mut bytes)?;
            Ok(u32::from_le_bytes(bytes))
        }
        fn try_next_u64(&mut self) -> Result<u64, Infallible> {
            let mut bytes = [0; 8];
            self.try_fill_bytes(&mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        }
        fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Infallible> {
            for byte in output {
                self.0 = self.0.wrapping_mul(31).wrapping_add(7);
                *byte = self.0;
            }
            Ok(())
        }
    }
    impl TryCryptoRng for Counter {}

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        let mut output = [0; 4];
        assert_eq!(read_hex("a1FF", &mut output).unwrap(), [0xa1, 0xff]);
        assert_eq!(write_hex::<8>(&[0xa1, 0xff]).unwrap(), "a1ff");
        assert!(read_hex("abc", &mut output).is_err());
        assert!(read_hex("zz", &mut output).is_err());
        assert!(read_hex("0011223344", &mut output).is_err());
        assert!(write_hex::<3>(&[1, 2]).is_err());
    }

    #[test]
    fn generated_key_yields_a_usable_credential() {
        let key = generate_device_key(Counter(1)).unwrap();
        assert_eq!(key.len(), 64);
        assert_ne!(key, generate_device_key(Counter(2)).unwrap());

        let credential = public_credential(&key).unwrap();
        // {8: {1: {1: 2, 2: h'', -1: 1, -2: h'…32 bytes…'}}}
        assert!(credential.starts_with("a108a101a4010202402001215820"));
        assert_eq!(credential.len(), 2 * (14 + 32));
        assert_ne!(
            credential,
            public_credential(&generate_device_key(Counter(2)).unwrap()).unwrap()
        );
    }

    #[test]
    fn keys_of_the_wrong_length_or_range_are_rejected() {
        for bad in ["", "ab", &"ab".repeat(33), &"00".repeat(32)] {
            assert!(public_credential(bad).is_err(), "{bad}");
        }
    }
}

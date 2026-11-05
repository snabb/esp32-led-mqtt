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

pub fn write_hex<const N: usize>(bytes: &[u8]) -> Result<String<N>, Error> {
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
    write_hex(identity(private_hex, &mut SoftwareCrypto::new(NoRandomness))?.credential_bytes())
}

/// First eight hex digits of the credential's SHA-256, to name a client key in the API.
pub fn fingerprint(ccs: &[u8]) -> Result<String<8>, Error> {
    let digest = SoftwareCrypto::new(NoRandomness).sha256(ccs)?;
    write_hex(&digest[..4])
}

/// One CBOR item's head: major type and argument, plus what follows it.
fn cbor_head(bytes: &[u8]) -> Option<(u8, usize, &[u8])> {
    let (&first, rest) = bytes.split_first()?;
    let (argument, rest) = match first & 0x1f {
        small @ 0..=23 => (usize::from(small), rest),
        24 => {
            let (&byte, rest) = rest.split_first()?;
            (usize::from(byte), rest)
        }
        25 => {
            let (pair, rest) = rest.split_first_chunk::<2>()?;
            (usize::from(u16::from_be_bytes(*pair)), rest)
        }
        _ => return None,
    };
    Some((first >> 5, argument, rest))
}

/// Skips one item of the kinds a CCS uses: integers, strings and maps.
fn cbor_skip(bytes: &[u8], depth: u8) -> Option<&[u8]> {
    let (major, argument, mut rest) = cbor_head(bytes)?;
    match major {
        0 | 1 => Some(rest),
        2 | 3 => rest.get(argument..),
        5 if depth > 0 => {
            for _ in 0..argument * 2 {
                rest = cbor_skip(rest, depth - 1)?;
            }
            Some(rest)
        }
        _ => None,
    }
}

/// In a CBOR map, the value of the first entry with this small unsigned key.
fn cbor_map_value(map: &[u8], wanted: usize) -> Option<&[u8]> {
    let (5, entries, mut rest) = cbor_head(map)? else {
        return None;
    };
    for _ in 0..entries {
        let (major, key, after_key) = cbor_head(rest)?;
        if major == 0 && key == wanted {
            return Some(after_key);
        }
        rest = cbor_skip(cbor_skip(rest, 0)?, 4)?;
    }
    None
}

/// The key ID in a CCS: `{8: {1: {2: kid, …}, …}, …}`. Two clients with the same key ID
/// can't be told apart in a handshake, and neither of them could connect.
pub fn credential_key_id(ccs: &[u8]) -> Option<&[u8]> {
    let cnf = cbor_map_value(ccs, 8)?;
    let cose_key = cbor_map_value(cnf, 1)?;
    let value = cbor_map_value(cose_key, 2)?;
    let (2, length, rest) = cbor_head(value)? else {
        return None;
    };
    rest.get(..length)
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
    fn key_id_is_found_in_either_credential_form() {
        let mut crypto = SoftwareCrypto::new(Counter(3));
        for (kid, encoding) in [
            (&[][..], CcsEncoding::CompactX),
            (&[0x01][..], CcsEncoding::FullXY),
            (&[1, 2, 3, 4][..], CcsEncoding::FullXY),
        ] {
            let identity = Identity::from_private_key([7; 32], kid, encoding, &mut crypto).unwrap();
            assert_eq!(credential_key_id(identity.credential_bytes()), Some(kid));
        }
        // aiocoap's form, with a subject ahead of `cnf`: {2: "x", 8: {1: {1: 2, 2: h'05'}}}
        let with_subject = [
            0xa2, 0x02, 0x61, b'x', 0x08, 0xa1, 0x01, 0xa2, 0x01, 0x02, 0x02, 0x41, 0x05,
        ];
        assert_eq!(credential_key_id(&with_subject), Some(&[5][..]));
    }

    #[test]
    fn key_id_of_malformed_credentials_is_none() {
        for bad in [
            &[][..],
            &[0xa1],
            &[0xa1, 0x08],
            &[0xa1, 0x08, 0x01],
            &[0xa1, 0x08, 0xa1, 0x01, 0xa1, 0x02, 0x45, 0x01],
            &[0x81, 0x00],
        ] {
            assert_eq!(credential_key_id(bad), None, "{bad:02x?}");
        }
    }

    #[test]
    fn fingerprints_differ_by_credential() {
        assert_eq!(fingerprint(b"a").unwrap().len(), 8);
        assert_ne!(fingerprint(b"a").unwrap(), fingerprint(b"b").unwrap());
    }

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

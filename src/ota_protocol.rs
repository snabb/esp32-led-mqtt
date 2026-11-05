//! Bounded OTA command, response, and image validation shared with host tests.

use digest::Digest;
use heapless::String;
use serde::Deserialize;

pub const SLOT_SIZE: u32 = 0x1f0000;
pub const IMAGE_PREFIX_SIZE: usize = 288;
pub const PROJECT: &str = env!("FIRMWARE_APP_PROJECT");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Command,
    Url,
    Host,
    Size,
    Digest,
    Target,
    Build,
    Http,
    Image,
    Incomplete,
}

/// What an image must turn out to be, however it arrives.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub size: u32,
    pub crc32: u32,
    pub sha256: [u8; 32],
    pub build: String<32>,
}

/// A download request owns its strings so the receive buffer can be released.
#[derive(Clone, Debug)]
pub struct Request {
    pub host: String<128>,
    pub path: String<384>,
    pub port: u16,
    pub tls: bool,
    pub manifest: Manifest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command<'a> {
    url: &'a str,
    size: u32,
    crc32: u32,
    sha256: &'a str,
    target: &'a str,
    build: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFields<'a> {
    size: u32,
    crc32: u32,
    sha256: &'a str,
    target: &'a str,
    build: &'a str,
}

impl Manifest {
    /// Parses a manifest for an image the client will upload itself: a download command
    /// without its `url`.
    pub fn parse(payload: &[u8]) -> Result<Self, Error> {
        let (fields, consumed): (ManifestFields<'_>, _) =
            serde_json_core::from_slice(payload).map_err(|_| Error::Command)?;
        if !payload[consumed..].iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Command);
        }
        Self::validated(
            fields.size,
            fields.crc32,
            fields.sha256,
            fields.target,
            fields.build,
        )
    }

    fn validated(
        size: u32,
        crc32: u32,
        sha256: &str,
        target: &str,
        build: &str,
    ) -> Result<Self, Error> {
        if target != "esp32c6" {
            return Err(Error::Target);
        }
        if size < IMAGE_PREFIX_SIZE as u32 || size > SLOT_SIZE {
            return Err(Error::Size);
        }
        if build.is_empty()
            || build.len() > 31
            || !build
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::Build);
        }
        Ok(Self {
            size,
            crc32,
            sha256: decode_sha256(sha256)?,
            build: String::try_from(build).map_err(|_| Error::Build)?,
        })
    }
}

impl Request {
    /// Parse a complete command and enforce the configured host allowlist.
    ///
    /// `hosts` is a comma-separated list of exact DNS names or IPv4 addresses.
    /// Plain HTTP is rejected unless explicitly enabled for local testing.
    pub fn parse(payload: &[u8], hosts: &str, allow_http: bool) -> Result<Self, Error> {
        let (command, consumed): (Command<'_>, _) =
            serde_json_core::from_slice(payload).map_err(|_| Error::Command)?;
        if !payload[consumed..].iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Command);
        }
        let manifest = Manifest::validated(
            command.size,
            command.crc32,
            command.sha256,
            command.target,
            command.build,
        )?;
        let (tls, rest, default_port) = if let Some(rest) = command.url.strip_prefix("https://") {
            (true, rest, 443)
        } else if allow_http {
            (
                false,
                command.url.strip_prefix("http://").ok_or(Error::Url)?,
                80,
            )
        } else {
            return Err(Error::Url);
        };
        let (authority, path) = rest.split_once('/').ok_or(Error::Url)?;
        if authority.is_empty()
            || command.url.bytes().any(|b| b <= 0x20 || b >= 0x7f)
            || command.url.contains(['@', '#', '\\'])
        {
            return Err(Error::Url);
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| Error::Url)?),
            None => (authority, default_port),
        };
        if host.is_empty()
            || port == 0
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err(Error::Url);
        }
        if !hosts
            .split(',')
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(host))
        {
            return Err(Error::Host);
        }
        let mut request = Self {
            host: String::try_from(host).map_err(|_| Error::Url)?,
            path: String::new(),
            port,
            tls,
            manifest,
        };
        request.path.push('/').map_err(|_| Error::Url)?;
        request.path.push_str(path).map_err(|_| Error::Url)?;
        Ok(request)
    }
}

fn decode_sha256(text: &str) -> Result<[u8; 32], Error> {
    if text.len() != 64 {
        return Err(Error::Digest);
    }
    let mut output = [0; 32];
    for (slot, pair) in output.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        let hex = |b: u8| {
            (b as char)
                .to_digit(16)
                .map(|v| v as u8)
                .ok_or(Error::Digest)
        };
        *slot = (hex(pair[0])? << 4) | hex(pair[1])?;
    }
    Ok(output)
}

/// Validate a complete HTTP header block. Bodies with ambiguous framing fail closed.
pub fn validate_headers(bytes: &[u8], expected_size: u32) -> Result<(), Error> {
    let text = core::str::from_utf8(bytes).map_err(|_| Error::Http)?;
    let text = text.strip_suffix("\r\n\r\n").ok_or(Error::Http)?;
    let mut lines = text.split("\r\n");
    let status = lines.next().ok_or(Error::Http)?;
    if !(status.starts_with("HTTP/1.1 200 ") || status.starts_with("HTTP/1.0 200 ")) {
        return Err(Error::Http);
    }
    let mut length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(Error::Http)?;
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 0x20 && b != b'\t')
        {
            return Err(Error::Http);
        }
        if name.eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            if length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::Http);
            }
            length = Some(value.parse::<u32>().map_err(|_| Error::Http)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-encoding")
                && !value.trim().eq_ignore_ascii_case("identity"))
        {
            return Err(Error::Http);
        }
    }
    if length != Some(expected_size) {
        return Err(Error::Size);
    }
    Ok(())
}

/// Validate the ESP image header and first-segment app descriptor before flash writes.
pub fn validate_image_prefix(prefix: &[u8], build: &str) -> Result<(), Error> {
    if prefix.len() < IMAGE_PREFIX_SIZE
        || prefix[0] != 0xe9
        || !(1..=16).contains(&prefix[1])
        || u16::from_le_bytes([prefix[12], prefix[13]]) != 13
        || u32::from_le_bytes(prefix[28..32].try_into().unwrap()) < 256
        || prefix[32..36] != 0xabcd5432_u32.to_le_bytes()
    {
        return Err(Error::Image);
    }
    let cstr = |field: &[u8], expected: &str| {
        field
            .iter()
            .position(|&b| b == 0)
            .is_some_and(|end| &field[..end] == expected.as_bytes())
    };
    if !cstr(&prefix[48..80], build) || !cstr(&prefix[80..112], PROJECT) {
        return Err(Error::Build);
    }
    Ok(())
}

/// Streaming integrity check. Activation requires both exact length and matching SHA-256.
/// `H` is the SHA-256 implementation, so host tests can run without the accelerator.
pub struct Verifier<H> {
    hash: H,
    expected: [u8; 32],
    remaining: u32,
}

impl<H: Digest> Verifier<H> {
    pub fn new(manifest: &Manifest) -> Self {
        Self {
            hash: H::new(),
            expected: manifest.sha256,
            remaining: manifest.size,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let len = u32::try_from(bytes.len()).map_err(|_| Error::Size)?;
        self.remaining = self.remaining.checked_sub(len).ok_or(Error::Size)?;
        self.hash.update(bytes);
        Ok(())
    }

    pub fn finish(self) -> Result<(), Error> {
        if self.remaining != 0 {
            return Err(Error::Incomplete);
        }
        if self.hash.finalize().as_slice() != self.expected {
            return Err(Error::Digest);
        }
        Ok(())
    }
}

/// A slot's flash, which only takes an image once its start has been checked.
pub trait ImageTarget {
    fn begin(&mut self, size: u32, crc32: u32) -> Result<(), TargetError>;
    fn write(&mut self, bytes: &[u8]) -> Result<(), TargetError>;
    /// Reads the image back and makes it the next one to boot, on trial.
    fn activate(&mut self) -> Result<(), TargetError>;
}

/// The flash driver's reason isn't useful beyond which step failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetError;

/// What went wrong with an image, as reported in OTA status messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    Identity,
    Begin,
    FlashWrite,
    Size,
    Sha256,
    Activate,
}

impl ImageError {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Identity => "image_identity",
            Self::Begin => "begin",
            Self::FlashWrite => "flash_write",
            Self::Size => "size",
            Self::Sha256 => "sha256",
            Self::Activate => "verify_or_activate",
        }
    }
}

/// Flash sectors. Writing less at a time would erase a sector once for each write.
const IMAGE_CHUNK: usize = 4096;

/// Checks an image as it arrives, in any size of pieces, and writes it to `T`.
pub struct ImageWriter<H> {
    size: u32,
    crc32: u32,
    build: String<32>,
    verifier: Verifier<H>,
    chunk: [u8; IMAGE_CHUNK],
    chunk_len: usize,
    begun: bool,
    received: u32,
}

impl<H: Digest> ImageWriter<H> {
    pub fn new(manifest: &Manifest) -> Self {
        Self {
            size: manifest.size,
            crc32: manifest.crc32,
            build: manifest.build.clone(),
            verifier: Verifier::new(manifest),
            chunk: [0; IMAGE_CHUNK],
            chunk_len: 0,
            begun: false,
            received: 0,
        }
    }

    /// Accepts the next bytes, writing whole chunks. An error ends the transfer.
    pub fn push<T: ImageTarget>(
        &mut self,
        target: &mut T,
        mut bytes: &[u8],
    ) -> Result<(), ImageError> {
        self.verifier.update(bytes).map_err(|_| ImageError::Size)?;
        self.received += bytes.len() as u32;
        while !bytes.is_empty() {
            let count = bytes.len().min(IMAGE_CHUNK - self.chunk_len);
            self.chunk[self.chunk_len..self.chunk_len + count].copy_from_slice(&bytes[..count]);
            self.chunk_len += count;
            bytes = &bytes[count..];
            if self.chunk_len == IMAGE_CHUNK {
                self.flush(target)?;
            }
        }
        Ok(())
    }

    fn flush<T: ImageTarget>(&mut self, target: &mut T) -> Result<(), ImageError> {
        let chunk = &self.chunk[..self.chunk_len];
        if !self.begun {
            validate_image_prefix(chunk, &self.build).map_err(|_| ImageError::Identity)?;
            target
                .begin(self.size, self.crc32)
                .map_err(|_| ImageError::Begin)?;
            self.begun = true;
        }
        target.write(chunk).map_err(|_| ImageError::FlashWrite)?;
        self.chunk_len = 0;
        Ok(())
    }

    /// Checks the whole image against the manifest, then activates it.
    pub fn finish<T: ImageTarget>(mut self, target: &mut T) -> Result<(), ImageError> {
        if self.chunk_len != 0 {
            self.flush(target)?;
        }
        self.verifier.finish().map_err(|_| ImageError::Sha256)?;
        target.activate().map_err(|_| ImageError::Activate)
    }

    /// Share of the image received, in percent.
    pub fn percent(&self) -> u8 {
        (u64::from(self.received) * 100 / u64::from(self.size)) as u8
    }
}

/// First-boot confirmation policy; the deadline takes priority over a late health result.
#[derive(Debug, Eq, PartialEq)]
pub enum TrialDecision {
    Wait,
    Confirm,
    Rollback,
}

/// `control` is true once a configured control plane proves it works, or from the start
/// if none is configured.
pub fn trial_decision(elapsed_ms: u64, led: bool, control: bool, network: bool) -> TrialDecision {
    if elapsed_ms >= 120_000 {
        TrialDecision::Rollback
    } else if led && control && network {
        TrialDecision::Confirm
    } else {
        TrialDecision::Wait
    }
}

/// Accept only a valid TLS chain/name, allowing a certificate issued after build time.
/// `future_flag` is the TLS backend's not-yet-valid certificate flag; expiry remains fatal.
pub fn tls_flags_acceptable(flags: u32, future_flag: u32) -> bool {
    flags & !future_flag == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(url: &str, size: u32, digest: &str, build: &str) -> std::string::String {
        format!(
            r#"{{"url":"{url}","size":{size},"crc32":1,"sha256":"{digest}","target":"esp32c6","build":"{build}"}}"#
        )
    }
    const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    fn parse(url: &str) -> Result<Request, Error> {
        Request::parse(
            command(url, 4096, DIGEST, "test-1").as_bytes(),
            "fw.example",
            false,
        )
    }

    #[test]
    fn command_restricts_host_and_transport() {
        let request = parse("https://fw.example:8443/fw.bin").unwrap();
        assert!(request.tls);
        assert_eq!(request.port, 8443);
        assert_eq!(request.path, "/fw.bin");
        assert_eq!(
            parse("https://evil.example/fw.bin").unwrap_err(),
            Error::Host
        );
        assert_eq!(parse("http://fw.example/fw.bin").unwrap_err(), Error::Url);
        for url in [
            "https://fw.example@evil.example/fw.bin",
            "https://fw.example/fw.bin#x",
            "https://fw.example:0/fw.bin",
            "https://fw.example/fw.bin\r\nX: x",
        ] {
            assert!(parse(url).is_err());
        }
    }

    #[test]
    fn command_rejects_ambiguous_or_invalid_fields() {
        let good = command("https://fw.example/fw.bin", 4096, DIGEST, "test-1");
        for bad in [
            format!("{good}x"),
            good.replace("4096", "0"),
            good.replace("4096", "4294967295"),
            good.replace("esp32c6", "esp32s3"),
            good.replace(DIGEST, "ff"),
            good.replace("test-1", ""),
            good.replace("\"size\":4096", "\"size\":4096,\"size\":4096"),
        ] {
            assert!(Request::parse(bad.as_bytes(), "fw.example", false).is_err());
        }
    }

    #[test]
    fn manifest_is_a_command_without_a_url() {
        let good = format!(
            r#"{{"size":4096,"crc32":7,"sha256":"{DIGEST}","target":"esp32c6","build":"test-1"}}"#
        );
        let manifest = Manifest::parse(good.as_bytes()).unwrap();
        assert_eq!((manifest.size, manifest.crc32), (4096, 7));
        assert_eq!(manifest.build, "test-1");
        for bad in [
            format!("{good}x"),
            good.replace("4096", "0"),
            good.replace("4096", "4294967295"),
            good.replace("esp32c6", "esp32s3"),
            good.replace(DIGEST, "ff"),
            good.replace("test-1", ""),
            good.replacen('{', r#"{"url":"https://fw.example/fw.bin","#, 1),
        ] {
            assert!(Manifest::parse(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    /// Records how a writer used its flash.
    #[derive(Default)]
    struct FakeTarget {
        begun: std::vec::Vec<(u32, u32)>,
        chunks: std::vec::Vec<std::vec::Vec<u8>>,
        activated: bool,
        fail_write: bool,
        fail_activate: bool,
    }
    impl ImageTarget for FakeTarget {
        fn begin(&mut self, size: u32, crc32: u32) -> Result<(), TargetError> {
            self.begun.push((size, crc32));
            Ok(())
        }
        fn write(&mut self, bytes: &[u8]) -> Result<(), TargetError> {
            if self.fail_write {
                return Err(TargetError);
            }
            self.chunks.push(bytes.to_vec());
            Ok(())
        }
        fn activate(&mut self) -> Result<(), TargetError> {
            if self.fail_activate {
                return Err(TargetError);
            }
            self.activated = true;
            Ok(())
        }
    }

    fn test_image(size: usize) -> std::vec::Vec<u8> {
        let mut image: std::vec::Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        image[..IMAGE_PREFIX_SIZE].fill(0);
        image[0] = 0xe9;
        image[1] = 3;
        image[12] = 13;
        image[28..32].copy_from_slice(&256_u32.to_le_bytes());
        image[32..36].copy_from_slice(&0xabcd5432_u32.to_le_bytes());
        image[48..54].copy_from_slice(b"test-1");
        image[80..80 + PROJECT.len()].copy_from_slice(PROJECT.as_bytes());
        image
    }

    fn manifest_for(image: &[u8]) -> Manifest {
        Manifest {
            size: image.len() as u32,
            crc32: 7,
            sha256: sha2::Sha256::digest(image).into(),
            build: "test-1".try_into().unwrap(),
        }
    }

    #[test]
    fn writer_stores_whole_chunks_whatever_size_they_arrive_in() {
        let image = test_image(10_000);
        for piece in [1, 64, 1024, 4096, 10_000] {
            let mut target = FakeTarget::default();
            let mut writer = ImageWriter::<sha2::Sha256>::new(&manifest_for(&image));
            for bytes in image.chunks(piece) {
                writer.push(&mut target, bytes).unwrap();
            }
            assert_eq!(writer.percent(), 100);
            writer.finish(&mut target).unwrap();
            assert!(target.activated);
            assert_eq!(target.begun, [(10_000, 7)]);
            let sizes: std::vec::Vec<_> = target.chunks.iter().map(|c| c.len()).collect();
            assert_eq!(sizes, [4096, 4096, 1808], "pieces of {piece}");
            assert_eq!(target.chunks.concat(), image);
        }
    }

    #[test]
    fn writer_refuses_the_wrong_image_before_touching_flash() {
        let image = test_image(5000);
        let mut other_build = manifest_for(&image);
        other_build.build = "test-2".try_into().unwrap();
        let mut target = FakeTarget::default();
        let mut writer = ImageWriter::<sha2::Sha256>::new(&other_build);
        assert_eq!(writer.push(&mut target, &image), Err(ImageError::Identity));
        assert!(target.begun.is_empty() && target.chunks.is_empty());
    }

    #[test]
    fn writer_never_activates_an_image_that_fails_a_check() {
        let image = test_image(5000);

        let mut wrong_digest = manifest_for(&image);
        wrong_digest.sha256[0] ^= 1;
        let mut target = FakeTarget::default();
        let mut writer = ImageWriter::<sha2::Sha256>::new(&wrong_digest);
        writer.push(&mut target, &image).unwrap();
        assert_eq!(writer.finish(&mut target), Err(ImageError::Sha256));
        assert!(!target.activated);

        let mut target = FakeTarget::default();
        let mut writer = ImageWriter::<sha2::Sha256>::new(&manifest_for(&image));
        writer.push(&mut target, &image[..4999]).unwrap();
        assert_eq!(writer.finish(&mut target), Err(ImageError::Sha256));
        assert!(!target.activated);

        let mut target = FakeTarget::default();
        let mut writer = ImageWriter::<sha2::Sha256>::new(&manifest_for(&image[..4999]));
        assert_eq!(writer.push(&mut target, &image), Err(ImageError::Size));
        assert!(target.begun.is_empty());

        let mut target = FakeTarget {
            fail_write: true,
            ..FakeTarget::default()
        };
        let mut writer = ImageWriter::<sha2::Sha256>::new(&manifest_for(&image));
        assert_eq!(
            writer.push(&mut target, &image),
            Err(ImageError::FlashWrite)
        );

        let mut target = FakeTarget {
            fail_activate: true,
            ..FakeTarget::default()
        };
        let mut writer = ImageWriter::<sha2::Sha256>::new(&manifest_for(&image));
        writer.push(&mut target, &image).unwrap();
        assert_eq!(writer.finish(&mut target), Err(ImageError::Activate));
    }

    #[test]
    fn http_framing_is_strict() {
        assert_eq!(
            validate_headers(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n", 4096),
            Ok(())
        );
        for header in [
            "HTTP/1.1 302 Redirect\r\nContent-Length: 4096\r\n\r\n",
            "HTTP/1.1 200 OK\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nContent-Length: 4096\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nTransfer-Encoding: chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nContent-Encoding: gzip\r\n\r\n",
        ] {
            assert!(validate_headers(header.as_bytes(), 4096).is_err());
        }
    }

    #[test]
    fn image_target_and_identity_must_match() {
        let mut image = [0; IMAGE_PREFIX_SIZE];
        image[0] = 0xe9;
        image[1] = 3;
        image[12] = 13;
        image[28..32].copy_from_slice(&256_u32.to_le_bytes());
        image[32..36].copy_from_slice(&0xabcd5432_u32.to_le_bytes());
        image[48..54].copy_from_slice(b"test-1");
        image[80..80 + PROJECT.len()].copy_from_slice(PROJECT.as_bytes());
        assert_eq!(validate_image_prefix(&image, "test-1"), Ok(()));
        assert_eq!(validate_image_prefix(&image, "test-2"), Err(Error::Build));
        image[12] = 5;
        assert_eq!(validate_image_prefix(&image, "test-1"), Err(Error::Image));
        assert!(validate_image_prefix(&image[..24], "test-1").is_err());
    }

    #[test]
    fn trial_requires_all_health_checks_before_deadline() {
        assert_eq!(
            trial_decision(119_999, true, true, true),
            TrialDecision::Confirm
        );
        for health in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert_eq!(
                trial_decision(119_999, health.0, health.1, health.2),
                TrialDecision::Wait
            );
        }
        assert_eq!(
            trial_decision(120_000, true, true, true),
            TrialDecision::Rollback
        );
        assert_eq!(
            trial_decision(u64::MAX, false, false, false),
            TrialDecision::Rollback
        );
    }

    #[test]
    fn tls_lower_bound_does_not_hide_other_verification_failures() {
        let future = 0x0200;
        assert!(tls_flags_acceptable(0, future));
        assert!(tls_flags_acceptable(future, future));
        // MbedTLS expiry, hostname mismatch, untrusted chain, and unknown flags.
        for invalid in [0x01, 0x04, 0x08, 0x8000_0000, u32::MAX] {
            assert!(!tls_flags_acceptable(invalid, future));
            assert!(!tls_flags_acceptable(invalid | future, future));
        }
    }

    #[test]
    fn digest_and_exact_length_gate_completion() {
        let mut request = parse("https://fw.example/fw.bin").unwrap();
        request.manifest.size = 3;
        let mut verifier = Verifier::<sha2::Sha256>::new(&request.manifest);
        verifier.update(b"a").unwrap();
        verifier.update(b"bc").unwrap();
        assert_eq!(verifier.finish(), Ok(()));
        let mut verifier = Verifier::<sha2::Sha256>::new(&request.manifest);
        verifier.update(b"ab").unwrap();
        assert_eq!(verifier.finish(), Err(Error::Incomplete));
        let mut verifier = Verifier::<sha2::Sha256>::new(&request.manifest);
        assert_eq!(verifier.update(b"abcd"), Err(Error::Size));
        let mut verifier = Verifier::<sha2::Sha256>::new(&request.manifest);
        verifier.update(b"bad").unwrap();
        assert_eq!(verifier.finish(), Err(Error::Digest));
    }
}

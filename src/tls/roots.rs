//! Flash-resident trust roots, adapted from upstream's CA-bundle verify callback.
//! MIT license notice: licenses/mbedtls-rs-example-MIT.txt.
//! https://github.com/esp-rs/mbedtls-rs/blob/07aeabb493ac5c71b06199414807cf49d899dec6/examples/common/ca_bundle.rs

use core::ffi::{c_int, c_void};
use mbedtls_rs::{VerifyCallback, sys::*};

struct Root {
    subject: &'static [u8],
    key: &'static [u8],
}

static ROOTS: &[Root] = include!(concat!(env!("OUT_DIR"), "/roots.rs"));

pub fn callback() -> VerifyCallback {
    VerifyCallback {
        f: verify,
        p_ctx: core::ptr::null_mut(),
    }
}

unsafe extern "C" fn verify(
    _context: *mut c_void,
    certificate: *mut mbedtls_x509_crt,
    _depth: c_int,
    flags: *mut u32,
) -> c_int {
    // SAFETY: MbedTLS supplies a live certificate and writable verification flags
    // for the duration of this callback; the root table has static lifetime.
    let (child, flags) = unsafe { (&*certificate, &mut *flags) };
    // Preserve every other verification failure. FUTURE is retained for the
    // caller's build-time date policy, including certificates issued after build.
    if *flags & !MBEDTLS_X509_BADCERT_FUTURE != MBEDTLS_X509_BADCERT_NOT_TRUSTED {
        return 0;
    }
    // SAFETY: issuer_raw is a parsed DER Name owned by the live certificate.
    let issuer = unsafe { core::slice::from_raw_parts(child.issuer_raw.p, child.issuer_raw.len) };
    // A peer may send a cross-signed version of a root we already trust. Anchor
    // at the known subject AND key, rather than requiring its external signer.
    // SAFETY: these DER views belong to the live parsed certificate.
    let subject =
        unsafe { core::slice::from_raw_parts(child.subject_raw.p, child.subject_raw.len) };
    let key = unsafe { core::slice::from_raw_parts(child.pk_raw.p, child.pk_raw.len) };
    if ROOTS
        .iter()
        .any(|root| root.subject == subject && root.key == key)
    {
        *flags &= !MBEDTLS_X509_BADCERT_NOT_TRUSTED;
        return 0;
    }
    for root in ROOTS.iter().filter(|root| root.subject == issuer) {
        // SAFETY: child remains live and root.key is immutable static DER.
        if unsafe { verifies_signature(child, root.key) } {
            *flags &= !MBEDTLS_X509_BADCERT_NOT_TRUSTED;
            return 0;
        }
    }
    MBEDTLS_ERR_X509_FATAL_ERROR as c_int
}

unsafe fn verifies_signature(child: &mbedtls_x509_crt, key_der: &[u8]) -> bool {
    // SAFETY: MbedTLS initializes the zeroed key context, borrows the DER during
    // parsing, and frees only its own allocations before this function returns.
    unsafe {
        let mut key = core::mem::zeroed();
        mbedtls_pk_init(&mut key);
        let parsed = mbedtls_pk_parse_public_key(&mut key, key_der.as_ptr(), key_der.len());
        let md = mbedtls_md_info_from_type(child.private_sig_md);
        let mut hash = [0; MBEDTLS_MD_MAX_SIZE as usize];
        let valid = parsed == 0
            && mbedtls_pk_can_do(&key, child.private_sig_pk) != 0
            && !md.is_null()
            && mbedtls_md(md, child.tbs.p, child.tbs.len, hash.as_mut_ptr()) == 0
            && mbedtls_pk_verify_ext(
                child.private_sig_pk,
                child.private_sig_opts as *const c_void,
                &mut key,
                child.private_sig_md,
                hash.as_ptr(),
                mbedtls_md_get_size(md) as usize,
                child.private_sig.p,
                child.private_sig.len,
            ) == 0;
        mbedtls_pk_free(&mut key);
        valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CERTIFICATES: &[&[u8]] = &[
        include_bytes!(concat!(env!("OUT_DIR"), "/root-0.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-1.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-2.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-3.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-4.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-5.der")),
        include_bytes!(concat!(env!("OUT_DIR"), "/root-6.der")),
    ];

    struct Certificate(mbedtls_x509_crt);
    impl Certificate {
        fn parse(der: &[u8]) -> Self {
            // SAFETY: initialize a unique C context; parse_der copies its input.
            unsafe {
                let mut cert = Self(core::mem::zeroed());
                mbedtls_x509_crt_init(&mut cert.0);
                assert_eq!(
                    mbedtls_x509_crt_parse_der(&mut cert.0, der.as_ptr(), der.len()),
                    0
                );
                cert
            }
        }
        fn verify(&mut self, flags: &mut u32) -> c_int {
            // SAFETY: the context and flags remain live throughout the callback.
            let callback = callback();
            unsafe { (callback.f)(callback.p_ctx, &mut self.0, 0, flags) }
        }
    }
    impl Drop for Certificate {
        fn drop(&mut self) {
            // SAFETY: this context was initialized once and has unique ownership.
            unsafe { mbedtls_x509_crt_free(&mut self.0) }
        }
    }

    #[test]
    fn every_bundled_root_verifies_with_its_flash_public_key() {
        assert_eq!(ROOTS.len(), CERTIFICATES.len());
        for der in CERTIFICATES {
            let mut certificate = Certificate::parse(der);
            let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED;
            assert_eq!(certificate.verify(&mut flags), 0);
            assert_eq!(flags, 0);
        }
    }

    const INTERMEDIATE: &[u8] = include_bytes!("../../tests/fixtures/google-intermediate.der");
    const CROSS_SIGNED: &[u8] = include_bytes!("../../tests/fixtures/gts-r4-cross-signed.der");

    #[test]
    fn signed_intermediate_and_cross_signed_anchor_are_trusted() {
        for der in [INTERMEDIATE, CROSS_SIGNED] {
            let mut certificate = Certificate::parse(der);
            let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED;
            assert_eq!(certificate.verify(&mut flags), 0);
            assert_eq!(flags, 0);
        }
    }

    #[test]
    fn bad_signature_and_unknown_issuer_are_rejected() {
        let mut bad_signature = INTERMEDIATE.to_vec();
        *bad_signature.last_mut().unwrap() ^= 1;
        let mut certificate = Certificate::parse(&bad_signature);
        let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED;
        assert_ne!(certificate.verify(&mut flags), 0);
        assert_ne!(flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED, 0);

        let mut certificate = Certificate::parse(INTERMEDIATE);
        let original_len = certificate.0.issuer_raw.len;
        certificate.0.issuer_raw.len = 0;
        let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED;
        let result = certificate.verify(&mut flags);
        certificate.0.issuer_raw.len = original_len;
        assert_ne!(result, 0);
        assert_ne!(flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED, 0);
    }

    #[test]
    fn matching_subject_without_matching_key_is_not_an_anchor() {
        let mut certificate = Certificate::parse(INTERMEDIATE);
        let subject = (certificate.0.subject_raw.p, certificate.0.subject_raw.len);
        let issuer_len = certificate.0.issuer_raw.len;
        certificate.0.subject_raw.p = ROOTS[0].subject.as_ptr() as *mut u8;
        certificate.0.subject_raw.len = ROOTS[0].subject.len();
        certificate.0.issuer_raw.len = 0;
        let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED;
        let result = certificate.verify(&mut flags);
        certificate.0.subject_raw.p = subject.0;
        certificate.0.subject_raw.len = subject.1;
        certificate.0.issuer_raw.len = issuer_len;
        assert_ne!(result, 0);
        assert_ne!(flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED, 0);
    }

    #[test]
    fn other_certificate_failures_are_preserved() {
        for failure in [
            MBEDTLS_X509_BADCERT_CN_MISMATCH,
            MBEDTLS_X509_BADCERT_EXPIRED,
            MBEDTLS_X509_BADCERT_BAD_MD,
            MBEDTLS_X509_BADCERT_BAD_KEY,
        ] {
            let mut certificate = Certificate::parse(CERTIFICATES[0]);
            let original = MBEDTLS_X509_BADCERT_NOT_TRUSTED | failure;
            let mut flags = original;
            assert_eq!(certificate.verify(&mut flags), 0);
            assert_eq!(flags, original);
        }
    }

    #[test]
    fn future_flag_is_retained_for_the_build_time_policy() {
        let mut certificate = Certificate::parse(CERTIFICATES[0]);
        let mut flags = MBEDTLS_X509_BADCERT_NOT_TRUSTED | MBEDTLS_X509_BADCERT_FUTURE;
        assert_eq!(certificate.verify(&mut flags), 0);
        assert_eq!(flags, MBEDTLS_X509_BADCERT_FUTURE);
    }
}

Public certificates observed in the TLS chain from storage.googleapis.com on
2026-11-03, retrieved with openssl s_client -servername storage.googleapis.com
-connect storage.googleapis.com:443 -showcerts.

- google-intermediate.der: Google Trust Services WE2, signed by GTS Root R4.
- gts-r4-cross-signed.der: GTS Root R4, cross-signed by GlobalSign Root CA.

These fixtures exercise signature verification and anchoring a cross-signed
certificate at an existing trusted subject/public-key pair.

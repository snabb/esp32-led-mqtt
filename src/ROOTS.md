# TLS trust roots

[`roots.pem`](roots.pem) is the CA set used for HTTPS OTA downloads and MQTT over TLS. It is a
deliberately small subset of public roots, copied on 2026-11-02 from the
development host's Mozilla CA store (`/usr/share/ca-certificates/mozilla`). See
the [Mozilla CA certificate program](https://www.mozilla.org/en-US/about/governance/policies/security-group/certs/).

The build extracts DER-encoded subject names and public keys from these roots.
Only that binary trust data is embedded in firmware. The verification callback
selects a root by issuer name and parses only its key during a handshake;
MbedTLS retains chain, hostname and date verification. The callback follows the
[official upstream CA-bundle example](https://github.com/esp-rs/mbedtls-rs/blob/07aeabb493ac5c71b06199414807cf49d899dec6/examples/common/ca_bundle.rs).
The upstream dependency is pinned to that revision for its callback API.

SHA-256 fingerprints:

- ISRG_Root_X1: `96:BC:EC:06:26:49:76:F3:74:60:77:9A:CF:28:C5:A7:CF:E8:A3:C0:AA:E1:1A:8F:FC:EE:05:C0:BD:DF:08:C6`
- ISRG_Root_X2: `69:72:9B:8E:15:A8:6E:FC:17:7A:57:AF:B7:17:1D:FC:64:AD:D2:8C:2F:CA:8C:F1:50:7E:34:45:3C:CB:14:70`
- GTS_Root_R1: `D9:47:43:2A:BD:E7:B7:FA:90:FC:2E:6B:59:10:1B:12:80:E0:E1:C7:E4:E4:0F:A3:C6:88:7F:FF:57:A7:F4:CF`
- GTS_Root_R3: `34:D8:A7:3E:E2:08:D9:BC:DB:0D:95:65:20:93:4B:4E:40:E6:94:82:59:6E:8B:6F:73:C8:42:6B:01:0A:6F:48`
- GTS_Root_R4: `34:9D:FA:40:58:C5:E2:63:12:3B:39:8A:E7:95:57:3C:4E:13:13:C8:3F:E6:8F:93:55:6C:D5:E8:03:1B:3C:7D`
- Amazon_Root_CA_1: `8E:CD:E6:88:4F:3D:87:B1:12:5B:A3:1A:C3:FC:B1:3D:70:16:DE:7F:57:CC:90:4F:E1:CB:97:C6:AE:98:19:6E`
- DigiCert_Global_Root_G2: `CB:3C:CB:B7:60:31:E5:E0:13:8F:8D:D3:9A:23:F9:DE:47:FF:C3:5E:43:C1:14:4C:EA:27:D4:6A:5A:B1:CB:5F`

## Adding a root

1. Append the root certificate in PEM format to `roots.pem`.
2. Compare its fingerprint with a trusted source:
   `openssl x509 -in root.pem -noout -fingerprint -sha256`
3. Add it to the list above, then rebuild.

Devices pick up the new root only after they run an image that includes it.
To get it there, update from a host the old roots already cover, or reinstall
over USB.

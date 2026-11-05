# OTA updates

An update starts with a JSON command over MQTT that gives the image's URL,
size and digests. The device downloads the image over HTTPS and writes it to
the slot it isn't running from. It checks the image, reboots into it on trial,
and keeps it only if the LEDs and Wi-Fi work within two minutes and a control
plane proves it works too: an MQTT connection, or one authenticated
[CoAP](COAP.md) request. If neither is configured, LEDs and Wi-Fi are enough.
Otherwise the bootloader goes back to the previous image.

With the `coap` feature, an image can also be
[uploaded directly](COAP.md#firmware-upload), and the command below can be sent to
`/firmware/update`.

Stored Wi-Fi and MQTT settings are never touched by an update.
See also [MQTT over TLS configuration](CONFIGURATION.md#mqtt-over-tls).

## Enabling OTA

OTA needs all of these:

1. The rollback bootloader from `tools/build-ota-bootloader.sh`, flashed as in
   the [README quick start](../README.md#3-flash-over-usb). espflash's default
   bootloader doesn't do rollback.
2. A build with `OTA_ROLLBACK_BOOTLOADER=1` and a non-empty `OTA_ALLOWED_HOSTS`.
3. Device settings, so that the device has Wi-Fi and MQTT.

If anything is missing, the device runs normally and reports OTA as `disabled`.
Use the same OTA variables for every image you build. An image built without
them can only be replaced over USB.

### Build variables

| Variable | Meaning |
|---|---|
| `OTA_ALLOWED_HOSTS` | Comma-separated list of exact host names or IPv4 addresses (case-insensitive) that the device may download from. It is compiled in, so a new host only works once an image that lists it is running. |
| `OTA_ROLLBACK_BOOTLOADER=1` | You confirm that the rollback bootloader is installed. The firmware can't detect this itself. |
| `OTA_ALLOW_HTTP=1` | Also accept `http://` URLs. For local testing only. |
| `OTA_BUILD_ID` | Image version, 1–31 characters from letters, digits, `.`, `_` and `-`. The default is the UTC build time plus the Git commit, such as `20261102T194500Z-ad6073d4`, with `-dirty` added if there are uncommitted changes. |
| `SOURCE_DATE_EPOCH` | Build time in Unix seconds, for reproducible builds. Defaults to now and must fall in 2026–2100. It is also the earliest date HTTPS certificate checks assume, so it must not be later than the real build time. |

The build ID is printed at boot (`Starting LED firmware BUILD hardware …`) and
appears in every OTA status message.

## Send an update

1. Build with the same `hardware.json` and OTA variables as the running image,
   and with a new build ID:

   ```sh
   OTA_BUILD_ID=v2 OTA_ALLOWED_HOSTS=firmware.example.com \
     OTA_ROLLBACK_BOOTLOADER=1 cargo build --locked --release
   ```

2. Convert the ELF to an application binary. Don't use `--merge`:

   ```sh
   espflash save-image --chip esp32c6 --partition-table partitions.csv \
     target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt firmware.bin
   ```

3. Upload `firmware.bin` to an allowed host. See
   [hosting requirements](#hosting-requirements).

4. Write the command. `build` must equal the image's build ID. `crc32` is the
   standard zlib CRC-32 of the whole file:

   ```sh
   python3 - firmware.bin https://firmware.example.com/firmware.bin v2 \
     > command.json <<'EOF'
   import hashlib, json, sys, zlib
   image = open(sys.argv[1], "rb").read()
   print(json.dumps({"url": sys.argv[2], "size": len(image), "crc32": zlib.crc32(image),
                     "sha256": hashlib.sha256(image).hexdigest(), "target": "esp32c6",
                     "build": sys.argv[3]}, separators=(",", ":")))
   EOF
   ```

   That produces:

   ```json
   {"url":"https://firmware.example.com/firmware.bin","size":1148544,"crc32":200828364,"sha256":"<64 hex digits>","target":"esp32c6","build":"v2"}
   ```

   All six fields are required, and the device rejects any others.

5. Watch the status topic and publish the command **without** retain. Add
   `-u USER -P PASSWORD` if your broker needs them:

   ```sh
   mosquitto_sub -h BROKER -t esp32-led-XXXXXX/ota/status -v &
   mosquitto_pub -h BROKER -t esp32-led-XXXXXX/ota/set -f command.json
   ```

A successful update reports `downloading` (in 10 % steps), `verifying`,
`rebooting`, `trial` and finally `ready` with the new `running_build`. A failed
download leaves the running image as it was. Downloads can't resume, so send the command again to retry.

## Status messages

`PREFIX/ota/status` is retained and holds the latest state only, so quick
transitions can be skipped:

```json
{"state":"downloading","detail":"","progress":40,"running_build":"v1"}
```

| `state` | `detail` |
|---|---|
| `ready` | Ready to accept a command |
| `disabled` | `configure_network_and_ota`: OTA isn't built in, or there are no device settings. `tls_entropy` or `tls_init`: TLS initialization failed. Other details identify a failed boot check. |
| `trial` | `health_check`: the new image is waiting for LEDs, Wi-Fi and a control plane |
| `downloading` | `progress` is 0–100 |
| `verifying` | `flash_readback` |
| `rebooting` | `verified`: the device is rebooting into the new image |
| `rejected` | The command wasn't started (table below) |
| `failed` | The step that failed (table below) |

### Troubleshooting

| Detail | Cause and fix |
|---|---|
| `invalid_command` | Malformed JSON, missing or extra fields, a host not in `OTA_ALLOWED_HOSTS`, an `http://` URL without `OTA_ALLOW_HTTP`, `target` other than `esp32c6`, a `size` outside 288 bytes–1984 KiB, or a malformed digest |
| `already_running` | `build` is the same as the running build ID |
| `unavailable` | OTA is disabled in the running image, or a trial boot hasn't been confirmed yet |
| `busy` | An update or USB settings write is already running |
| `retained_command` | The command was retained. Clear it with `mosquitto_pub -h BROKER -t PREFIX/ota/set -r -n`. |
| `dns`, `dns_timeout`, `connect`, `connect_timeout` | The device can't reach the host. Only IPv4 (DNS A records) is supported. |
| `tls_handshake`, `tls_certificate`, other `tls_*` | The certificate chain isn't covered by the bundled roots, the host name doesn't match, the certificate expired before the build date, or the server doesn't accept P-256. Retry once: a one-off handshake failure has been seen. |
| `http_framing` | Not a `200` response, missing or wrong `Content-Length`, chunked or compressed body, or a redirect |
| `headers_too_large` | The response headers are over 2 KiB |
| `image_identity` | Not an ESP32-C6 app image, a `build` that doesn't match the image, or a different `hardware.json` |
| `sha256`, `size`, `truncated`, `read`, `read_timeout` | The download didn't match the command or was cut off |
| `timeout` | The whole update took more than 180 s |
| `begin`, `flash_write`, `verify_or_activate`, `partition_table` | Flash or partition error. Check the serial log. |
| `bootloader_state` (state `disabled`) | The bootloader doesn't support rollback. Reflash it as in the [README](../README.md#3-flash-over-usb). |
| Serial log `OTA unavailable: partition_layout` (or `missing_partition`, `partition_table`) | The flash doesn't match `partitions.csv`. Settings aren't loaded either, so nothing reaches MQTT. Reflash as in the README. |

## Trial boot and rollback

After rebooting into a new image, the firmware has 120 s to write a frame to
the LEDs, get an IPv4 address and connect to MQTT. If all three succeed, it
marks the image valid. If not, it resets, and the bootloader switches back to
the previous image. A 125 s hardware watchdog catches a firmware that hangs.

- Wi-Fi or broker outages during the trial also cause a rollback.
- An image without device settings can't pass the trial.
- The old image doesn't retry the rejected update by itself.
- OTA commands sent during the trial are rejected with `unavailable`.

## Reinstalling over USB

Once an update has run, the device may be booting from `ota_1`, and a USB flash
to `ota_0` alone would be ignored. Use the full command from the
[README](../README.md#3-flash-over-usb). It erases `otadata` (OTA metadata)
so that the device boots `ota_0`, and leaves the device settings alone.

## Hosting requirements

- The URL must be `https://host[:port]/path`, without user info or a fragment,
  with a host name of up to 128 bytes and a path of up to 384 bytes. Use a
  direct object URL.
- The response must be `200` with exactly one `Content-Length` equal to `size`.
  It must not be chunked (`Transfer-Encoding`) or compressed
  (`Content-Encoding` other than `identity`). Redirects aren't followed.
- The response headers must fit in 2 KiB.
- The server certificate has to chain to one of the
  [bundled roots](../src/ROOTS.md): ISRG Root X1/X2 (Let's Encrypt), GTS
  Root R1/R3/R4 (Google Trust Services), Amazon Root CA 1 and DigiCert Global
  Root G2. Private CAs and other providers need their root added to
  `src/roots.pem`, followed by a rebuild.
- The server must accept P-256 (secp256r1) for key exchange, the only group the
  device offers for updates. TLS 1.3 requires this support, but a server can
  still be configured without it.

### Certificate dates

The device has no real-time clock or SNTP, so it uses the firmware's build time
as the earliest possible date:

- Certificates that expired before the build are rejected.
- Certificates that aren't valid yet are accepted. This allows certificates
  issued after the build.
- A certificate that expires after the build isn't detected as expired.

The device rejects every other verification error, including an untrusted
chain or a host name mismatch, before it sends the HTTP request.

## Security model

OTA is designed for a trusted network:

- The MQTT command is the authorization. Anyone who can publish to
  `PREFIX/ota/set` can install any image from an allowed host that matches the
  hardware profile. Restrict that topic with broker ACLs.
- Images are checked against the SHA-256, CRC32 and size in the command and
  against the app descriptor. They aren't signed.
- There is no anti-rollback, so an older build can be installed.
- Flash encryption and secure boot aren't used. Anyone with physical access can
  read the settings, credentials included.

With the `coap` feature, a trusted CoAP client can upload any image, without
`OTA_ALLOWED_HOSTS` applying. See [CoAP security](COAP.md#security).

## Flash layout

4 MiB flash, from [`partitions.csv`](../partitions.csv). The firmware checks
this layout at boot and turns off OTA and settings storage if it doesn't match.

| Partition | Offset | Size | Purpose |
|---|---|---|---|
| `nvs` | `0x9000` | 16 KiB | Not used by the application code |
| `otadata` | `0xd000` | 8 KiB | OTA boot selection |
| `phy_init` | `0xf000` | 4 KiB | Not used by the application code |
| `ota_0` | `0x10000` | 1984 KiB | App slot. Gets the first install. |
| `ota_1` | `0x200000` | 1984 KiB | App slot |
| `device_config` | `0x3f0000` | 64 KiB | Wi-Fi and MQTT settings |

The bootloader sits at `0x0` and the partition table at `0x8000`. There is no
factory slot.

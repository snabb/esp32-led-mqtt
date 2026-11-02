# OTA prototype

This prototype uses MQTT commands and a separate HTTP(S) download, writing only
the inactive application slot. It verifies the expected byte count, ESP32-C6
image header, application name, build identity, SHA-256, and CRC32. `finish(true)`
reads the image back before selecting it for a trial boot.

This is a trusted-network prototype. The MQTT command authorizes the update and
supplies its digest; firmware signatures and security anti-rollback are not
implemented. Do not retain update commands. The receiver rejects retained
commands, concurrent updates, and commands for the currently running build.

## Build and first provisioning

The 4 MiB layout has two 1,984 KiB application slots and 8 KiB of OTA metadata.
There is no factory slot. The initial application goes in `ota_0`.

1. Build the rollback-enabled ESP-IDF 6.1 bootloader:

   ```sh
   tools/build-ota-bootloader.sh
   ```

   This requires Docker. The script uses a pinned container digest, mounts only
   a fresh `target/ota-bootloader.*` directory, checks the rollback option and
   bootloader size, and writes the artifact and provenance to `bootloader/`. Generated binaries and
   provenance are ignored by Git. See [bootloader details](../bootloader/README.md).

2. Choose one or more exact download hostnames and build:

   ```sh
   OTA_ALLOWED_HOSTS=firmware.example.com,storage.googleapis.com \
   OTA_ROLLBACK_BOOTLOADER=1 cargo build --locked --release
   ```

   `OTA_ROLLBACK_BOOTLOADER=1` is an operator assertion that the matching rollback
   bootloader will be provisioned. It cannot detect the flashed bootloader's
   configuration. Without it, or without a host allowlist, OTA is disabled and
   ordinary LED/MQTT operation continues. Keep these settings on every OTA build.
   For an explicit local HTTP trial only, also set `OTA_ALLOW_HTTP=1`.

3. The new layout reuses part of the former NVS area for `otadata`.
   Fresh-provision the connected board with an explicit port:

   ```sh
   espflash flash --chip esp32c6 --port PORT --partition-table partitions.csv \
     --bootloader bootloader/esp32c6-rollback.bin --target-app-partition ota_0 \
     --erase-parts otadata target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt
   ```

   Erasing `otadata` is for this initial provisioning only. Normal OTA updates
   preserve it. Do not use the fresh-provisioning command for routine USB updates
   without deciding which image should boot afterward. `espflash.toml` selects
   the custom table and bootloader for local tools; a normal USB flash into
   `ota_0` does not override metadata selecting `ota_1`.

4. Provision a private JSON or YAML configuration file over USB:

   ```sh
   uv run tools/provision.py status --port PORT
   uv run tools/provision.py provision --port PORT --config local/device.json
   ```

   The file contains `wifi_ssid`, `wifi_password`, and `mqtt_broker`; optional
   fields are `schema` (1), `mqtt_port` (1883), `mqtt_username`, `mqtt_password`,
   and `wifi_auth` (`wpa2_personal` or explicit `open`). Keep credentials in an
   ignored directory. Successful provisioning saves settings and reboots.
   `erase-config --port PORT` explicitly clears settings and reboots.

5. Inspect serial output for the running build/slot, LED, Wi-Fi, MQTT and OTA
   ready state. A blank metadata partition is initialized as valid `ota_0`.

The build ID appears in the app descriptor and logs. Its default is UTC build
time plus eight Git SHA characters, such as `20261102T194500Z-ad6073d4`, with
`-dirty` appended for an uncommitted working tree. `OTA_BUILD_ID=trial-2` can select a
short explicit identity. `SOURCE_DATE_EPOCH` supplies the timestamp for
reproducible builds; it also sets the TLS time lower bound, so choose it honestly.

## HTTPS policy

HTTPS uses MbedTLS with radio-backed entropy and a bundled CA set. It checks the
certificate chain and URL hostname before sending the HTTP request. Certificate
expiry is checked against the firmware build time. Only the not-yet-valid flag
is tolerated, allowing hosting certificates issued after the firmware build.
Certificates expired before the build, untrusted chains, hostname mismatches,
and all other verification failures are rejected.

No SNTP or current wall clock is required. This lower-bound policy cannot detect
expiry occurring after the firmware was built. It also cannot determine whether
a later-issued certificate has started its validity period in actual UTC.

The initial CA bundle covers ISRG X1/X2, GTS R1/R3/R4, Amazon Root CA 1 and
DigiCert Global Root G2. It is not a complete public root store. Add the appropriate
reviewed root for another provider before deploying that URL; private CAs also
need explicit provisioning. See [root provenance](../src/ota/ROOTS.md).

## Create an update

Build a different image with the same OTA settings, then generate an application
binary without `--merge`:

```sh
espflash save-image --chip esp32c6 --partition-table partitions.csv \
  target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt firmware.bin
```

Host that binary at an allowed URL. Publish a JSON command, without retain,
to `esp32_led_mqtt_SUFFIX/ota/set`. A command has this shape:

```json
{"url":"https://firmware.example.com/firmware.bin","size":700000,"crc32":123456789,"sha256":"64 hexadecimal digits","target":"esp32c6","build":"trial-2"}
```

Subscribe to `esp32_led_mqtt_SUFFIX/ota/status` for retained state/progress. Logs
provide the running slot/build and errors. Status updates are snapshots, not a
lossless event stream.

The HTTP response must be status 200 with exactly one matching Content-Length.
Redirects, chunked transfer encoding and compressed responses are rejected.
Use a direct object URL; follow neither login pages nor hosting redirects.

Downloads have 15-second connect/read timeouts and a 180-second overall async
budget. Flash writes and final read-back/activation are synchronous; a timeout
cannot interrupt one of these operations. Data is accumulated into 4 KiB blocks
so short TCP reads do not repeatedly erase a flash sector. Resume is not yet
implemented; restart a failed update from the beginning.

## Confirmation and rollback

A trial image has two minutes to complete a successful LED write and establish
usable Wi-Fi and MQTT. It then marks itself valid. Failure causes a software
reset; a 125-second hardware watchdog also resets a stalled executor. The
rollback-enabled bootloader selects the previous valid slot on the next boot.
External Wi-Fi or broker outages also cause trial rollback under this policy.
The old image is not automatically asked to reinstall the rejected build.

Do not start another OTA while a trial image is unconfirmed. If the bootloader
leaves an image in NEW rather than PENDING_VERIFY, the updater disables itself
rather than pretending rollback is enabled.

## Validation boundary

Host tests cover malformed commands, framing, image identity, exact length,
digest failures and the selective TLS verification-flag policy. Build checks do
not establish ESP32-C6 MMU detection, actual certificate verification, flash
power-loss safety, stack margin, LED timing during flash, or working rollback.

Hardware acceptance requires both slot directions, an interrupted transfer,
wrong image/digests, TLS hostname/chain/expiry failures, trial failure/watchdog
rollback, successful confirmation, and power interruption during metadata writes.
Record serial logs and flash/stack/heap observations for those trials.

Software verification on 2026-11-02 used example credentials and
`OTA_ALLOWED_HOSTS=updates.example.invalid OTA_ROLLBACK_BOOTLOADER=1` on the peer
Arm build host: all 17 host tests, target Clippy with warnings denied, and the
linked release build passed. Rustfmt, Ruff, shell syntax, partition conversion,
app-image generation, command generation and bootloader checksum checks passed.

The resulting application image is 1,116,096 bytes of a 2,031,616-byte slot.
Compared with the pre-OTA release from the same host, RAM sections excluding the
reserved stack grew by 20,988 bytes (including a 324-byte increase in RAM code).
This includes static task storage, but does not measure dynamic TLS heap peaks
or runtime stack peaks. The bootloader is 22,224 bytes and passed the 0x8000
boundary check in two independent build directories. No new layout or firmware
has been flashed as part of this implementation.

## Public firmware publication

The public prefix is `gs://BUCKET/firmware/esp32-led-mqtt/`. Firmware uploads are
deferred until credentials move out of the app image. See the
[persistent configuration plan](DEVICE_CONFIG_PLAN.md) for provisioning, OTA
preservation, versioning and publication layout.

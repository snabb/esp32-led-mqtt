# OTA updates

The firmware takes update commands over MQTT and downloads the image over
HTTP(S), writing only the inactive application slot. It verifies the expected byte count, ESP32-C6
image header, application name, build identity, SHA-256, and CRC32. `finish(true)`
reads the image back before selecting it for a trial boot.

This is meant for a trusted network. The MQTT command authorizes the update and
supplies its digest; firmware signatures and security anti-rollback are not
implemented. Do not retain update commands. The receiver rejects retained
commands, concurrent updates, and commands for the currently running build.

## First installation over USB

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

3. Flash the bootloader, partition table and application into `ota_0`, erasing
   the OTA metadata:

   ```sh
   espflash flash --chip esp32c6 --port PORT --partition-table partitions.csv \
     --bootloader bootloader/esp32c6-rollback.bin --target-app-partition ota_0 \
     --erase-parts otadata target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt
   ```

   OTA updates preserve `otadata`; see [reinstalling over USB](#reinstalling-over-usb)
   before erasing it again.

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
   The log line `Device identity esp32_led_mqtt_SUFFIX` gives the MQTT topic
   prefix used below; `SUFFIX` is the last six hex digits of the MAC address.

The build ID appears in the app descriptor and logs. Its default is UTC build
time plus eight Git SHA characters, such as `20261102T194500Z-ad6073d4`, with
`-dirty` appended for an uncommitted working tree. `OTA_BUILD_ID=trial-2` can select a
short explicit identity of at most 31 letters, digits, dots, underscores or
hyphens. `SOURCE_DATE_EPOCH` supplies the timestamp for reproducible builds; it
also sets the TLS time lower bound, so choose it honestly.

## Reinstalling over USB

After an OTA update, `otadata` may select `ota_1`, and a USB flash into `ota_0`
then has no effect. To install an image over USB, repeat the step 3 command:
erasing `otadata` makes the bootloader start `ota_0` again. Stored Wi-Fi and
MQTT settings are kept because `device_config` is not erased.

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

## Create and send an update

1. Build with the same `hardware.json`, `OTA_ALLOWED_HOSTS` and
   `OTA_ROLLBACK_BOOTLOADER` as the running image, and an explicit build ID that
   differs from the running one. Images for another hardware profile are
   rejected with `image_identity`.

   ```sh
   OTA_BUILD_ID=trial-2 OTA_ALLOWED_HOSTS=firmware.example.com \
   OTA_ROLLBACK_BOOTLOADER=1 cargo build --locked --release
   ```

2. Generate the application binary, without `--merge`:

   ```sh
   espflash save-image --chip esp32c6 --partition-table partitions.csv \
     target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt firmware.bin
   ```

3. Upload `firmware.bin` to a URL on an allowed host. The allowlist is compiled
   into the running image, so a new host takes effect only after an update to an
   image that allows it.

4. Generate the command. `crc32` is the standard zlib CRC-32 of the whole file:

   ```sh
   python3 - firmware.bin https://firmware.example.com/firmware.bin trial-2 \
     > command.json <<'EOF'
   import hashlib, json, sys, zlib
   image = open(sys.argv[1], "rb").read()
   print(json.dumps({"url": sys.argv[2], "size": len(image), "crc32": zlib.crc32(image),
                     "sha256": hashlib.sha256(image).hexdigest(), "target": "esp32c6",
                     "build": sys.argv[3]}, separators=(",", ":")))
   EOF
   ```

   A command has this shape:

   ```json
   {"url":"https://firmware.example.com/firmware.bin","size":700000,"crc32":123456789,"sha256":"64 hexadecimal digits","target":"esp32c6","build":"trial-2"}
   ```

5. Watch the status topic, then publish the command without retain. Add
   `-u USER -P PASSWORD` if the broker requires them:

   ```sh
   mosquitto_sub -h BROKER -t esp32_led_mqtt_SUFFIX/ota/status -v &
   mosquitto_pub -h BROKER -t esp32_led_mqtt_SUFFIX/ota/set -f command.json
   ```

A successful update reports `downloading` in 10% steps, `rebooting`
(`verified`), then `trial` and `ready` with the new `running_build`. `rejected`
and `failed` give the reason in `detail`. The status topic is retained and
reports snapshots, not a lossless event stream; serial logs show the running
slot, build and errors.

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

## Public firmware publication

The public prefix is `gs://BUCKET/firmware/esp32-led-mqtt/`. See
[persistent configuration](DEVICE_CONFIG.md) for provisioning, OTA preservation,
versioning and publication layout.

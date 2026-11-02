# OTA Firmware Update Plan

Reviewed against upstream sources on 2026-11-02. This is an implementation plan;
OTA and rollback have not been validated on this device. The prototype is
implemented; see [operating instructions](docs/OTA.md) and the recorded software
validation boundary there.

## Decision

Prototype with `esp-hal-ota` 0.7.2 and `esp-storage` 0.10 on the current
esp-hal 1.2.2 stack. Use MQTT to trigger updates and report status, and a separate
HTTP(S) connection to download the application image.

Keep the transport separate from the flash writer. Use the existing
`esp-bootloader-esp-idf` 0.6 `ota_updater::OtaUpdater` as the alternative if
ESP32-C6 hardware testing exposes issues with `esp-hal-ota`. This alternative
provides partition and metadata management, but requires our own streaming
writer and image verification.

## Library Assessment

- `esp-hal-ota` 0.7.2 was published on October 17, 2026. It provides streaming
  writes, CRC32 verification, mandatory flash read-back verification, progress,
  resume information, and image-state management without allocation.
- Its `esp32c6` feature and MMU support are present, but the author still lists
  ESP32-C6 as untested. Successful compilation is not hardware validation.
- `esp-bootloader-esp-idf` 0.6 is already a dependency. Its OTA support is a
  lower-level alternative, not evidence of a fully validated updater.
- `esp-ota-nostd` 0.1 was last published in October 2024 and uses
  `embedded-io-async` 0.6. Retain it as reference material rather than a preferred
  dependency.
- Rustyfarian now provides an Embassy HTTP OTA implementation. Its published
  `rustyfarian-esp-hal-network` 0.6 pins embassy-net 0.8, while this firmware uses
  0.9.1, and its repository documents flash erase amplification. Consult its
  implementation without adopting the networking framework.
- ESP-IDF/`std` OTA libraries do not fit this bare-metal firmware.

## Provisioning and Image Format

- Add `partitions.csv` with `otadata` (two sectors, 0x2000 bytes) and two app slots
  on the verified 4 MiB M5Stack NanoC6. Use 64 KiB-aligned app offsets and leave
  room for firmware growth. The current application image is 669,968 bytes;
  enforce the actual slot capacity for every generated image.
- Use a repo `espflash.toml` to select the partition table. Pass it explicitly
  when tooling runs outside the repo directory, and use the same layout for
  USB flashing, image generation, and runtime partition discovery.
- Build or obtain a bootloader with `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`
  enabled. Record its source revision, configuration, and artifact checksum.
  Espressif's OTA example states that espflash's default precompiled bootloader
  does not enable automatic rollback.
- Provision the bootloader, partition table, and initial application over USB.
  Check existing flash contents before changing the layout. Do not erase
  `otadata` as part of normal updates or routine reflashing.
- Keep `esp_bootloader_esp_idf::esp_app_desc!()` in firmware. Include a distinct
  build identity/version so both the device and operator can identify an update.
- Generate the OTA payload with `espflash save-image --chip esp32c6` without
  `--merge`. Transfer the application `.bin`, not the ELF or a combined flash
  image containing the bootloader and partition table.

## Update Flow

- Add `esp-hal-ota` with `esp32c6` and `esp-storage` 0.10 with `esp32c6` and
  `embedded-storage`; the storage trait implementations are opt-in in 0.10.
- Give one OTA task ownership of `esp_storage::FlashStorage::new(peripherals.FLASH)`.
  Reject concurrent updates and updates while the running image is unconfirmed.
- Read the partition table and identify the running and inactive slots. Fail
  without modifying boot selection if the layout or running slot is unknown.
- Validate declared image size, target chip, and application identity before
  activation. Reject zero-length, oversized, truncated, and unexpected images.
- Use the current `esp_hal_ota::Ota` API:
  1. `begin(size, expected_crc32)`.
  2. `write(chunk)` into the inactive slot.
  3. Check the exact received length and expected digest.
  4. `finish(true)` to verify CRC32, read the image back, and select it with
     rollback requested.
  5. Reboot with `esp_hal::system::software_reset()`.
- Change boot selection only after all checks succeed. On failure, keep the
  current image bootable and report the failure without rebooting into the
  incomplete image.
- Require a fresh transfer after interruption for the first prototype. Persisted
  resume support is deferred even though the library exposes resume information.

## Transport

- Define a bounded MQTT update command containing the image URL, size, CRC32,
  SHA-256 digest, target, and build identity. Publish progress and failure status
  on separate OTA topics; keep firmware bytes out of MQTT messages.
- Use a dedicated Embassy TCP socket and a bounded HTTP downloader. Account for
  its socket and buffer requirements in `StackResources` and task storage.
- For the prototype, accept only configured download hosts and simple
  HTTP(S) responses: status 200, one valid Content-Length matching the command,
  no redirects, and no chunked transfer encoding.
- Bound connection time, each read, and total update duration. Handle short reads
  and premature EOF. Reject a response that does not match the declared size.
- Keep network reads and progress publication outside flash critical sections.
  MQTT command reception must remain available while the OTA task downloads.

## Flash Scheduling and RAM

`esp-storage` 0.10's ordinary `write` performs sector read-modify-erase-write and
uses a 4 KiB stack buffer. Both OTA library choices can reach this path. Passing
small socket reads directly to it repeatedly erases the same sector.

- Accumulate incoming bytes into sector-aligned 4 KiB blocks before writing.
  Bound and handle the final partial sector explicitly.
- Measure erase count, throughput, task storage, and runtime stack high-water
  use. Include internal storage scratch space in the RAM budget.
- If using explicit erase plus `write_nor`, use the required alignment and erase
  discipline; do not add erases in front of the existing read-modify-write path.
- Flash operations are synchronous and can disable interrupts/cache access.
  Yield between sector operations and measure their effect on Wi-Fi, MQTT,
  button responsiveness, and RMT output. Decide from hardware evidence whether
  to pause LED animation during updates.

## First-Boot Health and Rollback

- Inspect the running image's OTA state at boot. Start a bounded confirmation
  window for a trial image and prohibit another update until it is confirmed.
- Check application initialization, LED output, Wi-Fi, and MQTT. Mark the image
  valid with `mark_app_valid()` only after the agreed health checks pass.
- Use a two-minute confirmation timeout, then reset into rollback even if the
  failure is an external Wi-Fi/MQTT outage. Do not automatically retry a rejected
  build, avoiding an update/rollback cycle.
- Provide a watchdog or explicit reset on failed confirmation. The bootloader
  rolls back an unconfirmed image on the next reset; setting OTA state alone
  cannot recover an application that hangs indefinitely.
- Verify that the booted slot, selected slot, and state agree after successful
  updates and after rollback. Do not assume rollback works from API success.

## Validation

- Host-test command parsing, bounded HTTP framing, image length/digest checks,
  and update/confirmation state transitions with failure cases.
- Run formatting, host tests, firmware checks, Clippy, and a linked release build.
- Fresh-provision the actual bootloader/table/app combination over USB and
  verify the partition layout and artifact identities.
- Test initial-slot to alternate-slot updates and repeated alternation, checking
  build identity, image read-back, and boot selection each time.
- Test interrupted download, stalled server, short response, oversized image,
  incorrect target, invalid image header, wrong CRC32, and wrong SHA-256.
- Interrupt power during image writing and OTA metadata changes. Confirm that
  the last valid application remains bootable and can retry an update.
- Test trial-image panic, hang/watchdog reset, failed health confirmation,
  temporary Wi-Fi/MQTT outage, and successful confirmation. Verify actual
  rollback to the previous known-good build.
- Observe serial logs throughout hardware tests and measure LED/network behavior,
  stack margin, and erase amplification during download and verification.

## Security

CRC32 and SHA-256 detect corruption; neither authenticates firmware when an
attacker can replace both the image and its advertised checksum. MQTT commands authorize updates and are acceptable only on an explicitly trusted
local prototype network. HTTPS authenticates the download server; it does not
authenticate the MQTT command or provide firmware signatures.

Before wider use, authenticate update authorization and firmware metadata or
verify signed firmware against a trusted key. HTTPS verifies the chain and
hostname. Per the selected prototype policy, expiry is checked against firmware
build time, not current UTC; not-yet-valid flags are allowed for later-issued
certificates. SNTP is not required. Bootloader rollback and security anti-rollback
are separate mechanisms; eFuse-based anti-rollback is outside this prototype.

## Sources

- [esp-hal-ota README](https://github.com/filipton/esp-hal-ota)
- [esp-hal-ota migration guide](https://github.com/filipton/esp-hal-ota/blob/master/MIGRATING.md)
- [esp-hal-ota releases and dates](https://crates.io/api/v1/crates/esp-hal-ota)
- [Espressif OTA updater](https://github.com/esp-rs/esp-hal/blob/esp-bootloader-esp-idf-v0.6.0/esp-bootloader-esp-idf/src/ota_updater.rs)
- [Espressif OTA example and bootloader caveat](https://github.com/esp-rs/esp-hal/blob/main/examples/ota/update/src/main.rs)
- [ESP-IDF rollback lifecycle](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-reference/system/ota.html)
- [esp-storage 0.10 write implementation](https://github.com/esp-rs/esp-hal/blob/esp-storage-v0.10.0/esp-storage/src/storage.rs)
- [Rustyfarian OTA implementation](https://github.com/datenkollektiv/rustyfarian-network/tree/main/crates/rustyfarian-esp-hal-network/src/ota)
- [Rustyfarian erase amplification report](https://github.com/datenkollektiv/rustyfarian-network/blob/main/docs/bugs/004-hal-ota-erase-amplification-2026-10-05.md)

## Persistent configuration before public publication

Follow [DEVICE_CONFIG_PLAN.md](docs/DEVICE_CONFIG_PLAN.md) to move Wi-Fi/MQTT
credentials into a separate persistent flash partition before publishing generic
firmware under `gs://BUCKET/firmware/esp32-led-mqtt/`. Public builds use UTC timestamp versions and hardware-specific filenames. Git
revision provenance and publishing configuration stay in private local records.

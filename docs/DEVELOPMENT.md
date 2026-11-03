# Development

## Checks

These are the commands CI runs (`.github/workflows/ci.yml`). The default target
is `riscv32imac-unknown-none-elf` (set in `.cargo/config.toml`), so host tests
need an explicit target:

```sh
cargo fmt --check
cargo test --lib --target x86_64-unknown-linux-gnu
cargo check --target riscv32imac-unknown-none-elf
OTA_ALLOWED_HOSTS=updates.example.invalid OTA_ROLLBACK_BOOTLOADER=1 \
  cargo clippy --target riscv32imac-unknown-none-elf -- -D warnings
```

Run Clippy with the OTA variables set, or the OTA code paths go unchecked.

Host tests cover the library crate (`src/lib.rs` and its modules): effects, OTA
command, HTTP and image validation, the TLS flag policy, settings validation,
USB command framing, and the settings store, including simulated power loss
at every step of a settings write. The binary-only modules (networking, flash
ownership, USB I/O) are tested only on hardware.

## Heap measurements

Build with `--features heap-stats` to enable the allocator's allocation-time
high-water counter. Serial `HEAP` records show current usage, peak usage and
minimum free heap at TLS, MQTT and OTA milestones. The counter resets on reboot;
it is an observed heap peak, not a bound on stack or total RAM use.

MQTTS and HTTPS OTA need two concurrent TLS sessions. The heap is 164 KiB,
including 64 KiB of reclaimed startup RAM. Trust-root subject names and public keys are converted to DER at build time.
The verification callback selects a key directly from flash, without parsing
a complete root store into heap memory.

On 2026-11-03, a NanoC6 completed HTTPS OTA in both slot directions while
connected to a Let’s Encrypt-authenticated MQTTS broker. The callback build
recorded peaks of 133,024 and 140,432 bytes, leaving at least 27,504 bytes free
in those runs. These are observations for those certificate chains and traffic,
not worst-case bounds. The earlier 100 KiB heap failed concurrent OTA with
`tls_roots`.

## Hardware crypto

`tls::initialize` hooks MbedTLS to the SHA, RSA, AES and ECC peripherals, and the
OTA digest uses the SHA peripheral. SHA-384/512, AES-192, X25519 and P-384 stay
in software.

Measured on a NanoC6 against `storage.googleapis.com`, 4 boots per arm, 40
handshakes and 12 full image downloads per arm, 0 failures. Medians, with exact
Mann-Whitney p over boot medians (0.029 is the minimum for 4 against 4):

| Metric | Software | SHA, RSA, ECC | + AES |
|---|---|---|---|
| TLS handshake | 965 ms | 700 ms (p = 0.029) | not measured with default groups |
| Download | 121 kB/s | 128 kB/s (p = 0.69) | no significant change |
| Longest LED stall, handshake | 119 ms | 120 ms (p = 0.34) | no significant change |
| Longest LED stall, download | 832 ms | 570 ms (p = 0.029) | no significant change |

The AES comparison used arms offering P-256 first: 733 against 732 ms handshake
(p = 0.34) and 122 against 129 kB/s (p = 0.20). Those throughputs may be limited
by the network, so they don't show the CPU cost of either AES path. The cause of
the download LED stall wasn't identified. No RSA operation ran: this chain ends
at an ECDSA root.

The key-exchange groups stay at the MbedTLS default even though only P-256 is
accelerated. That server answers a P-256 key share with a HelloRetryRequest for
X25519, which raised the handshake from 700 to 733 ms (p = 0.029).

## Code layout

| Path | Role |
|---|---|
| `build.rs` | Turns `hardware.json` into `OUT_DIR/hardware.rs` (LED type, pins, timing). Sets the build ID, build time and app name. |
| `src/main.rs` | Peripheral setup, Wi-Fi connection loop, LED render loop |
| `src/light_state.rs` | Shared light state and change signal |
| `src/mqtt.rs` | MQTT session, Home Assistant discovery, command handling |
| `src/button.rs` | Short, double and long press handling |
| `src/effects/` | One file per effect |
| `src/flash.rs` | The only owner of `FlashStorage`: partition-layout check, watchdog, one flash job at a time (OTA or settings) |
| `src/config.rs` | USB command loop, settings load and save |
| `src/ota.rs`, `src/tls.rs` | Download, trial confirmation, shared MbedTLS client |
| `src/ota_protocol.rs` | Host-tested OTA command, HTTP and image checks |
| `src/device_config.rs`, `src/provisioning.rs`, `src/config_store.rs` | Host-tested settings schema, USB framing and flash store |
| `tools/provision.py` | Host side of USB provisioning |
| `tools/build-ota-bootloader.sh` | Rollback bootloader build. See [bootloader](../bootloader/README.md). |

## Adding an effect

1. Add `src/effects/my_effect.rs` with a `render` function.
2. In `src/effects/mod.rs`, add `mod my_effect;` and a dispatch arm.
3. In `src/lib.rs`, add the `EffectId` variant and an `EFFECTS` entry at the
   same position, and increase the length of `EFFECTS`.

Home Assistant's effect list, name parsing and the button cycle all come from
`EFFECTS`. The tests check that every effect animates and works with one LED.

## Design notes

### Device settings storage

- `device_config` holds one [`sequential-storage`](https://crates.io/crates/sequential-storage)
  map entry with the whole JSON snapshot. The library takes care of CRCs, page
  management and recovery after power loss. A major version upgrade of
  `sequential-storage` needs an on-flash format migration.
- The partition uses the data subtype `0x06` (Undefined) because the partition
  parser only accepts known subtypes. The contents aren't ESP-IDF NVS.
- Each save gets a generation number one higher than the last and is read back
  to check it.
- The host never writes the store format. It sends JSON over USB, and the
  firmware validates and stores it.
- Flash access goes through `YieldingAsync<BlockingAsync<…>>`. Single flash
  operations are still synchronous. `flash.rs` runs settings writes and OTA one
  at a time. Watchdogs: 125 s from boot until the boot and trial checks finish,
  190 s during an update, 30 s during a settings write.

### Secrets

- `DeviceConfig` and `provisioning::Command` deliberately don't implement
  `Debug`.
- Replies, logs and MQTT status report only whether settings exist, plus their
  schema and generation. Don't add credentials, raw records or digests of
  settings to logs, status, manifests or build IDs.
- The USB receive buffer is zeroed after each command.
- Never publish a flash dump. `device_config` holds the credentials as plain
  text. Images made from the ELF (`save-image`, with or without `--merge`)
  don't include it.

### Schema compatibility

Settings outlive the image that wrote them, and a rollback can bring back an
older image. So:

- Images that use schema 2 must read the same format and must not rewrite it
  during a trial boot.
- Unknown or unsupported data is reported, never silently replaced with
  defaults.
- An incompatible format change must keep a record that the previous image can
  read until the new image is confirmed. Never erase the only old-format copy
  first.

### Publishing images

Keep published images immutable (one URL per build ID). A public manifest can
include the image URL, build ID, hardware profile, size, CRC32 and SHA-256, and
an `ota_command` object with exactly the
[command fields](OTA.md#send-an-update). Upload the image first and the
manifest last. Don't put an image that hasn't been tested on hardware behind a
"latest" pointer. Build-ID timestamp ordering doesn't prevent downgrades.

## Hardware test status

Tested on an M5Stack NanoC6 (4 MiB):

- USB install and provisioning, with settings kept across reboots and OTA
- HTTPS OTA in both slot directions, trial confirmation
- Retained discovery, state and OTA status, Home Assistant control
- Button actions

Not yet tested on hardware:

- A forced trial failure that triggers rollback
- Power loss during an OTA or settings write
- `busy` replies during an update
- TLS failures (bad chain, host name or date)

Before a release, test both slot directions, an interrupted download, wrong
digests or a wrong image, TLS failures, a trial failure and a watchdog
rollback, and power loss during writes to `otadata` and `device_config`. Keep
the serial logs.

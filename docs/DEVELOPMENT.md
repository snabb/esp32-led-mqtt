# Development

## Checks

These are the commands CI runs (`.github/workflows/ci.yml`). The default target
is `riscv32imac-unknown-none-elf` (set in `.cargo/config.toml`), so host tests
need an explicit target:

```sh
cargo fmt --check
cargo test --lib --target x86_64-unknown-linux-gnu
cargo test --lib --target x86_64-unknown-linux-gnu --no-default-features --features mdns,mqtt
cargo check --target riscv32imac-unknown-none-elf
OTA_ALLOWED_HOSTS=updates.example.invalid OTA_ROLLBACK_BOOTLOADER=1 \
  cargo clippy --target riscv32imac-unknown-none-elf -- -D warnings
```

CI repeats the firmware checks with `--no-default-features`, and with that plus each of
`--features mqtt`, `--features mdns,mqtt` and `--features coap`.

Run Clippy with the OTA variables set, or the OTA code paths go unchecked.

## Cargo features

| Feature | Default | Enables |
|---|---|---|
| `mqtt` | Yes | MQTT, Home Assistant discovery and OTA started over MQTT |
| `mdns` | Yes | mDNS responder for `esp32-led-XXXXXX.local` |
| `coap` | Yes | [CoAP API](COAP.md) |
| `heap-stats` | No | Allocator high-water counters, see [heap measurements](#heap-measurements) |

Without a control plane the firmware still drives the LEDs, the button and USB
provisioning. An image built that way can only be replaced over USB. `_control` is
internal: it is implied by a control plane and brings in OTA downloads, TLS and the
temperature sampler.

Sizes on 2026-11-05, in the same OTA configuration, from `size -A`. Image is what occupies
the 2.03 MB slot, BSS includes the 164 KiB heap, and stack is what remains for it:

| Features | Image | BSS | Stack |
|---|---|---|---|
| none | 679,034 | 166,032 | 211,936 |
| `mqtt` | 1,233,488 | 184,040 | 191,368 |
| `mqtt`, `mdns` | 1,264,260 | 190,920 | 184,280 |
| `coap` | 1,393,292 | 226,184 | 148,696 |
| `coap`, `mdns` | 1,422,634 | 233,064 | 141,584 |
| `mqtt`, `mdns`, `coap` | 1,494,716 | 248,752 | 125,656 |

Of the CoAP cost, the server task is about 48 KB of BSS and the log ring 8 KB.

### CoAP measurements

On a NanoC6 with 60 LEDs on 2026-11-05, over Wi-Fi on one router hop. Single runs, not a
controlled comparison. These are from instrumented builds that log long gaps between LED frames.

| | Software P-256 | Hardware P-256 |
|---|---|---|
| Longest LED gaps for a new session | about 270 and 105 ms | about 45 and 35 ms |
| Whole request from `coap.py`, including its handshake | about 0.59 s | about 0.24 s |

- Uploading 1.49 MB took 45–56 s, about 30 kB/s, with one run of 107 s. CoAP Block1 sends one
  block per round trip. For comparison, an HTTPS download from Oracle ran at about 39 kB/s, and a
  whole update of the same image took 46 s including its reboot and trial. LED gaps were about 64 ms per flash chunk and 541 ms for the final read-back.
- Peak main stack was 31.7 KB of 127 KB, reached during boot.
- Lowest free heap was 111.7 KB running idle and 67.8 KB during an HTTPS download.
- One boot of the `mqtt`-only build, out of five, didn't reach Home Assistant within 60 s. There
  is no serial log of it, and four more boots reported within 5 s, so the cause is unknown.

Host tests cover the library crate (`src/lib.rs` and its modules): effects, OTA
command, HTTP and image validation, the TLS flag policy, settings validation,
USB command framing, and the settings store, including simulated power loss
at every step of a settings write. They also cover the CoAP
resources, through real EDHOC and OSCORE sessions over loopback UDP. The binary-only
modules (networking, flash ownership, USB I/O) are tested only on hardware.

To test `tools/coap.py` without a device, run the resources on the host. The commands are at
the top of [`examples/coap_host_server.rs`](../examples/coap_host_server.rs).

## Heap measurements

Build with `--features heap-stats` to enable the allocator's allocation-time
high-water counter. Serial `HEAP` records show current usage, peak usage and
minimum free heap at TLS, MQTT and OTA milestones. The counter resets on reboot;
it is an observed heap peak, not a bound on stack or total RAM use.

MQTTS and HTTPS OTA need two concurrent TLS sessions. The heap is 164 KiB,
including 64 KiB of reclaimed startup RAM. Trust-root subject names and public keys are converted to DER at build time.
The verification callback selects a key directly from flash, without parsing
a complete root store into heap memory.

On 2026-11-04, a NanoC6 with hardware crypto completed HTTPS OTA in both slot
directions while connected to a Let’s Encrypt-authenticated MQTTS broker. It
recorded peaks of 142,292 and 140,440 bytes, leaving at least 25,644 bytes free
in those runs. These are observations for those certificate chains and traffic,
not worst-case bounds. The earlier 100 KiB heap failed concurrent OTA with
`tls_roots`.

## Hardware crypto

`tls::initialize` hooks MbedTLS to the SHA, RSA, AES and ECC peripherals, and the
OTA digest uses the SHA peripheral. SHA-384/512, AES-192, X25519 and P-384 stay
in software.

CoAP uses `src/coap_crypto.rs`: P-256 point multiplication and streaming SHA-256 run on the
peripherals, as does firmware upload's image hash. AES-CCM stays in software because the
hardware has no such mode, as do HKDF and point decompression, which gain little.

OTA offers only P-256 for key exchange. MQTTS offers P-256 first and keeps the
other MbedTLS default groups as fallback, because the broker is user-chosen.
Against a Mosquitto broker with OpenSSL, P-256 first took the handshake from a
median of 1450 to 1233 ms (5 boots each, run in sequence, ranges not overlapping).
A broker that prefers X25519 can answer with a HelloRetryRequest instead, as the
OTA host does.

Measured on a NanoC6 against `storage.googleapis.com` in two runs with balanced
arm order, 4 boots per arm, 40 handshakes and 12 full image downloads per arm,
0 failures. Medians, with exact Mann-Whitney p over boot medians (0.029 is the
minimum for 4 against 4). Compare values only within a run.

| Metric | Run 1: software | Run 1: SHA, RSA, ECC | Run 2: all hooks, default groups | Run 2: all hooks, P-256 only |
|---|---|---|---|---|
| TLS handshake | 965 ms | 700 ms (p = 0.029) | 714 ms | 500 ms (p = 0.029) |
| Download | 121 kB/s | 128 kB/s (p = 0.69) | 118 kB/s | 120 kB/s (p = 0.69) |
| Longest LED stall, handshake | 119 ms | 120 ms (p = 0.34) | 121 ms | 21 ms (p = 0.029) |
| Longest LED stall, download | 832 ms | 570 ms (p = 0.029) | 580 ms | 476 ms (p = 0.029) |

Offering P-256 ahead of X25519 is slower than the default order against this
server (733 against 700 ms, p = 0.029): it answers with a HelloRetryRequest and
still selects X25519.

AES is enabled without a measured benefit: adding it changed handshakes from 733
to 732 ms (p = 0.34) and downloads from 122 to 129 kB/s (p = 0.20). Those
throughputs may be limited by the network, so they don't show the CPU cost of
either AES path. The cause of the download LED stall wasn't identified.

The RSA path is unconfirmed on hardware. Counters showed no RSA operation against
the Google chain, and the Let's Encrypt chain tested uses ECDSA certificates.

## Code layout

| Path | Role |
|---|---|
| `build.rs` | Turns `hardware.json` into `OUT_DIR/hardware.rs` (LED type, pins, timing). Sets the build ID, build time and app name. |
| `src/main.rs` | Peripheral setup, Wi-Fi connection loop, LED render loop |
| `src/light_state.rs` | Shared light state and change signal |
| `src/light_command.rs` | Host-tested light and effect-speed command parsing |
| `src/coap_api.rs`, `src/coap_crypto.rs`, `src/coap_keys.rs`, `src/log_buffer.rs` | Host-tested CoAP resources, crypto provider, keys and log ring |
| `src/coap.rs`, `src/log_sink.rs` | CoAP server task, logger with RAM copy |
| `src/mqtt.rs` | MQTT session, Home Assistant discovery, command handling |
| `src/mdns.rs` | mDNS responder |
| `src/button.rs` | Short, double and long press handling |
| `src/effects/` | One file per effect |
| `src/flash.rs` | The only owner of `FlashStorage`: partition-layout check, watchdog, one flash job at a time (OTA or settings) |
| `src/config.rs` | USB command loop, settings load and save |
| `src/ota.rs`, `src/ota/download.rs`, `src/tls.rs` | Trial confirmation, download, shared MbedTLS client |
| `src/tsens.rs` | Chip temperature sampling |
| `src/ota_protocol.rs` | Host-tested OTA command, HTTP and image checks |
| `src/device_config.rs`, `src/provisioning.rs`, `src/config_store.rs` | Host-tested settings schema, USB framing and flash store |
| `tools/provision.py` | Host side of USB provisioning |
| `tools/coap.py` | CoAP client |
| `tools/migrate-device-name.py` | Broker-side upgrade from the old device name |
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
- `coap_device_key` is generated on the device and never accepted from outside or returned.
  Logs can be read over CoAP, which is one more reason to keep secrets out of them.
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
- `coap_device_key`, `coap_clients` and an empty `mqtt_broker` are only written when
  set, so other snapshots stay byte-compatible with older images. Settings can't be
  written during a trial boot, so a rollback from a trial is safe. An image older
  than these fields can't read a snapshot that uses them: a deliberate downgrade
  past that point needs USB provisioning.

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
- The device rename on one unit, with `tools/migrate-device-name.py`
- mDNS on its own subnet. From another VLAN it depends on the router's reflection
- CoAP over Wi-Fi: every resource, Observe, client keys, settings, logs and tailing
- Firmware upload in both slot directions, interrupted, and with a wrong digest
- A trial confirmed by CoAP during a broker outage
- A trial failure that rolls back, followed by more updates and another rollback
- HTTPS OTA started over MQTT, from the build workflow through to trial confirmation
- `coap` alone, `mqtt` alone, and the defaults. Without any features: boot, Wi-Fi and USB
  provisioning

Not yet tested on hardware:

- Power loss during an OTA or settings write
- `busy` replies during an update
- TLS failures (bad chain, host name or date)

Before a release, test both slot directions, an interrupted download, wrong
digests or a wrong image, TLS failures, a trial failure and a watchdog
rollback, and power loss during writes to `otadata` and `device_config`. Keep
the serial logs.

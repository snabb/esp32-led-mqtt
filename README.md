# esp32-led-mqtt

Bare-metal Rust Embassy firmware for ESP32-C6 boards driving WS281x- and
SK68xx-compatible addressable RGB LED strips with ESP-HAL RMT output, Wi-Fi, and
Home Assistant MQTT discovery/control. The default configuration works out of
the box with the [M5Stack NanoC6](https://docs.m5stack.com/en/core/M5NanoC6)
and 60 LED [M5Stack RGB LED Strip](https://docs.m5stack.com/en/unit/rgb_led_strip).

## Hardware

- ESP32-C6 board [M5Stack NanoC6](https://docs.m5stack.com/en/core/M5NanoC6)
- 60 LED SK6812 LED strip [M5Stack RGB LED Strip](https://docs.m5stack.com/en/unit/rgb_led_strip)
- LED data on GPIO2
- Common ground between the board and strip power supply

## Configuration

Hardware settings are compiled from [`hardware.json`](hardware.json): LED count,
controller timing, RGB color order, data GPIO, optional power GPIO/polarity,
button GPIO and frame interval. The default is 60 SK6812 LEDs, GRB, data GPIO2,
button GPIO9 and no power-enable pin. For the NanoC6 internal LED, select one
WS2812 LED, data GPIO20 and power GPIO19.

Wi-Fi and MQTT settings are stored separately in the `device_config` flash
partition. Firmware builds never read credentials. Provision them over USB as
shown under [Install](#install); see
[persistent configuration](docs/DEVICE_CONFIG.md) for details.
An unconfigured device runs its LEDs/button and accepts USB configuration.

MQTT topics and Home Assistant identifiers are generated from the MAC address,
for example `esp32_led_mqtt_123456`. A short button press cycles effects, a double
press changes color, and a long press toggles power.

## Install

The firmware boots through a rollback-enabled bootloader and a two-slot
partition table. See [OTA updates](docs/OTA.md) for details on each step.

1. Build the bootloader once. This needs Docker:

   ```sh
   tools/build-ota-bootloader.sh
   ```

2. Build with OTA enabled, listing the exact hosts that will serve updates.
   Without these variables the image works but cannot be updated over OTA:

   ```sh
   OTA_ALLOWED_HOSTS=firmware.example.com OTA_ROLLBACK_BOOTLOADER=1 \
     cargo build --locked --release
   ```

3. Connect the board over USB and flash the bootloader, partition table and
   application. Erasing `otadata` is for the first installation; see
   [reinstalling over USB](docs/OTA.md#reinstalling-over-usb) later:

   ```sh
   espflash flash --chip esp32c6 --port PORT --partition-table partitions.csv \
     --bootloader bootloader/esp32c6-rollback.bin --target-app-partition ota_0 \
     --erase-parts otadata target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt
   ```

4. Provision Wi-Fi and MQTT settings. `local/` is ignored by Git:

   ```sh
   mkdir -p local
   cp tools/device.example.json local/device.json
   # Edit local/device.json with your settings.
   uv run tools/provision.py provision --port PORT --config local/device.json
   uv run tools/provision.py status --port PORT
   ```

   Provisioning saves the settings in a separate flash partition and reboots.
   Firmware updates preserve them. `espflash monitor --chip esp32c6` shows the
   boot log, including the device's MQTT prefix (`Device identity ...`).

## Updates over OTA

Build later images with the same `hardware.json` and OTA variables, upload the
binary to an allowed host and publish an update command over MQTT; see
[create and send an update](docs/OTA.md#create-and-send-an-update). An updated
image runs on trial and rolls back unless the LEDs, Wi-Fi and MQTT work within
two minutes. HTTPS checks certificates against a bundled set of roots.

## Other ESP32 Chips

The firmware currently targets ESP32-C6. Other Wi-Fi ESP32 chips with RMT, such
as ESP32-C3, ESP32-S3, and the original ESP32, should be possible by changing
the ESP crate chip features, Rust target, and `espflash --chip` setting, but
they are not tested by this repository.

## Home Assistant

The firmware publishes MQTT discovery for one JSON-schema light and one sibling
number entity on the same device. Home Assistant can control:

- Power
- RGB color
- Brightness
- Effect
- Effect Speed

The `None` effect disables animations and uses the Home Assistant RGB color
directly. `Effect Speed` is a 1-128 slider with finer adjustment at slow speeds;
the default is `64`. The default boot effect is `Rainbow` at the firmware
default brightness.

## Development

### Build and Check

The repository pins the Rust target and ESP flash runner in `.cargo/config.toml`.
These commands are useful before submitting changes; they are not required
before flashing a configured device.

```sh
cargo fmt --check
cargo test --lib --target x86_64-unknown-linux-gnu
cargo check
cargo clippy --target riscv32imac-unknown-none-elf --lib --bin esp32-led-mqtt
```

### Adding Effects

Effects live in `src/effects`, with one file per effect. To add one:

1. Add `src/effects/my_effect.rs` with a `render` function.
2. Add `mod my_effect;` and one dispatch arm in `src/effects/mod.rs`.
3. Add the `EffectId` variant and `EFFECTS` entry in `src/lib.rs`, in the same order.

Home Assistant discovery, name parsing and button cycling are derived from
`EFFECTS`.

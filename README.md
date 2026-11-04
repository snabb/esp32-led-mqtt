# esp32-led-mqtt

Bare-metal Rust firmware ([Embassy](https://embassy.dev/) + esp-hal) that
runs an addressable RGB LED strip from an ESP32-C6 and connects it to
Home Assistant over MQTT.

- 24 animated effects plus a solid color, with brightness and effect-speed control
- Home Assistant MQTT discovery: a light entity plus an effect-speed slider
- Optional push button: next effect, random color, power on/off
- Wi-Fi and MQTT settings are written over USB and kept in flash, so one
  firmware image (with no credentials in it) works for every device
- Optional over-the-air (OTA) updates over HTTPS, started by an MQTT message, with
  a trial boot and automatic rollback

## Supported hardware

| | |
|---|---|
| Board | ESP32-C6 with 4 MiB flash and the built-in USB Serial/JTAG port. Tested on the [M5Stack NanoC6](https://docs.m5stack.com/en/core/M5NanoC6). Other ESP32 chips are not supported. |
| LED strip | WS2812, WS2812B, WS2811 or SK6812 (RGB), 1–512 LEDs. Tested with the 60-LED [M5Stack RGB LED Strip](https://docs.m5stack.com/en/unit/rgb_led_strip). |
| Wiring | Strip data on one GPIO (default GPIO2), with the board and the strip power supply sharing a ground. |
| Network | 2.4 GHz Wi-Fi, WPA2-Personal or open. MQTT 5 over plain TCP, IPv4. The broker must support MQTT 5. |

The defaults in [`hardware.json`](hardware.json) match the NanoC6 driving the
M5Stack strip from its Grove port. For any other setup, see
[hardware profile](docs/CONFIGURATION.md#hardware-profile).

## Quick start

### Prerequisites

- [Rust](https://rustup.rs/) through rustup. `rust-toolchain.toml` installs the
  RISC-V target and components on first build.
- [espflash](https://github.com/esp-rs/espflash) 4.x (tested with 4.6.0):
  `cargo install espflash --locked`
- Docker, to build the bootloader
- [uv](https://docs.astral.sh/uv/), to run the provisioning tool
- An MQTT broker. Home Assistant needs its MQTT integration set up.

The commands below are for Linux. The board usually shows up as
`/dev/ttyACM0`:

```sh
git clone https://github.com/snabb/esp32-led-mqtt.git
cd esp32-led-mqtt
PORT=/dev/ttyACM0
```

### 1. Build the bootloader (once)

The firmware needs an ESP-IDF bootloader with app rollback enabled. This script
builds one in a pinned ESP-IDF container and writes it to
`bootloader/esp32c6-rollback.bin`:

```sh
tools/build-ota-bootloader.sh
```

### 2. Build the firmware

Edit `hardware.json` first if your hardware differs from the defaults. To make
OTA updates possible, list the hosts that will serve firmware images:

```sh
OTA_ALLOWED_HOSTS=firmware.example.com OTA_ROLLBACK_BOOTLOADER=1 \
  cargo build --locked --release
```

Leaving out both variables (`cargo build --locked --release`) builds an image
that can't be updated over OTA. You can turn OTA on later by reinstalling over
USB. See [OTA updates](docs/OTA.md#enabling-ota) for details.

### 3. Flash over USB

```sh
espflash flash --chip esp32c6 --port $PORT --partition-table partitions.csv \
  --bootloader bootloader/esp32c6-rollback.bin --target-app-partition ota_0 \
  --erase-parts otadata target/riscv32imac-unknown-none-elf/release/esp32-led-mqtt
```

Run the same command to reinstall later. Erasing `otadata` makes the device boot
the new image in `ota_0` and leaves the stored Wi-Fi and MQTT settings alone.

The LEDs should now show the `Rainbow` effect at low brightness. The device
needs settings before it can connect.

### 4. Provision Wi-Fi and MQTT

`local/` is ignored by Git, so it's a safe place for credentials:

```sh
mkdir -p local
cp tools/device.example.json local/device.json
$EDITOR local/device.json
uv run tools/provision.py provision --port $PORT --config local/device.json
```

The device saves the settings and reboots. For every field, see
[device settings](docs/CONFIGURATION.md#device-settings).

### 5. Check that it's running

```sh
espflash monitor --chip esp32c6 --port $PORT
```

Look for `Device identity esp32_led_mqtt_XXXXXX` (its MQTT topic prefix, made
from the last six hex digits of the MAC address), then `Network ready with IP`
and `MQTT connected`. Home Assistant should now list a new **ESP32 LED MQTT**
device. Close the monitor before using `provision.py` again, because both use
the same port.

## Usage

### Home Assistant

The device has four entities:

- **LED Strip** (light): on/off, RGB color, brightness and effect. The `None`
  effect shows the solid color.
- **Effect Speed** (number, 1–128, default 64 = normal): exponential, from 1/16× to 16×.
- **Chip Temperature** (diagnostic sensor, °C): die temperature, updated every minute.
- **Firmware Version** (diagnostic sensor): the running build ID.

Effects: Rainbow, Color Wipe, Scan, Twinkle, Random Twinkle, Fireworks, Flicker,
Breathe, Theater Chase, Confetti, Sinelon, Juggle, BPM, Fire 2012, Pacifica,
Aurora, Plasma Flow, Circus, Static Noise, Police Car.

After a reboot the strip stays dark until the device reaches the MQTT broker,
then restores the light state and effect speed it last published there. If the
broker has none, or is never reached, the strip stays dark until it is turned on
from Home Assistant or with the button. The button turns it on as `Rainbow` at
brightness 8/255 and speed 64, and a button press before the broker is reached
cancels the restore.

### Button

When `button_gpio` is set (GPIO9, the NanoC6 button, by default). The button is
active-low and uses the internal pull-up.

| Press | Action |
|---|---|
| Short | Next effect. After the last effect comes `None` (solid color), then `Rainbow` again. |
| Double | Pick a random color |
| Long (≥ 0.7 s) | Turn on or off |

### MQTT without Home Assistant

Send JSON commands to `esp32_led_mqtt_XXXXXX/light/set`:

```sh
mosquitto_pub -h BROKER -t esp32_led_mqtt_XXXXXX/light/set \
  -m '{"state":"ON","effect":"Fire 2012","brightness":64}'
```

All topics and payloads are listed in the [MQTT reference](docs/MQTT.md).

### Firmware updates

Build a new image with the same `hardware.json` and OTA settings, upload it to
an allowed HTTPS host, then publish an update command over MQTT. The device
checks the image, boots it on trial, and rolls back if LEDs, Wi-Fi and MQTT
aren't all working within two minutes. See [OTA updates](docs/OTA.md).

## Documentation

| Document | Contents |
|---|---|
| [Configuration](docs/CONFIGURATION.md) | `hardware.json`, device settings and provisioning, build options |
| [MQTT reference](docs/MQTT.md) | Topics, payloads, Home Assistant discovery |
| [OTA updates](docs/OTA.md) | Enabling OTA, sending updates, rollback, HTTPS hosting, troubleshooting |
| [Development](docs/DEVELOPMENT.md) | Checks, code layout, adding effects, design notes, test status |
| [Bootloader](bootloader/README.md) | How the rollback bootloader is built, and its provenance |

## Troubleshooting

| Symptom | Check |
|---|---|
| LEDs stay dark or flicker | `data_gpio`, `timing` and `led_count` in `hardware.json`; a shared ground; strip power. The default brightness is low. |
| Colors are swapped | `color_order` in `hardware.json` |
| Log says `Device configuration required` | The device has no settings yet. Run step 4. |
| `provision.py` reports `invalid` | A setting failed validation, such as a WPA2 password that isn't 8–63 characters. See [device settings](docs/CONFIGURATION.md#device-settings). |
| `provision.py` reports `busy` | An OTA update or trial boot is running. Wait two minutes. If it keeps happening, look for `OTA unavailable: partition_layout` in the log and reflash using step 3. |
| `provision.py` times out | Another program, such as `espflash monitor`, has the port open. |
| A USB flash changes nothing | After an OTA update the device boots `ota_1`. Flash with `--erase-parts otadata` as in step 3. |
| Wi-Fi keeps reconnecting | Transmit power is capped at 10 dBm (`set_max_tx_power(40)` in `src/main.rs`). Move closer to the access point. |
| No device in Home Assistant | The log should show `MQTT connected`. Discovery uses the default `homeassistant` prefix. |

## Limitations

- MQTT runs without TLS, and Wi-Fi and MQTT credentials sit unencrypted in
  flash. Use this on a trusted network.
- Anyone who can publish to the broker can start an OTA update from an allowed
  host. Images are checked against the digests in the command, but they aren't
  signed. See [OTA security model](docs/OTA.md#security-model).
- The Home Assistant discovery prefix (`homeassistant`) and the device name are
  fixed in the firmware.
- No web or Bluetooth setup. Settings can only be changed over USB.

## License

[MIT](LICENSE)

### Finland effects

Finland Snow, Finland Blue White Twinkle, Finland Snowfall and Finland Ribbon use fixed blue and white palettes. Brightness and effect speed remain adjustable.

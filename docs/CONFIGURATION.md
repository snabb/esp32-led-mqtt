# Configuration

Configuration comes in two layers:

- **Hardware profile** (`hardware.json`) is compiled into the firmware. One
  image fits every device with the same hardware.
- **Device settings** (Wi-Fi, MQTT) are stored in each device's flash and
  written over USB. Builds never read them.

## Hardware profile

`build.rs` reads [`hardware.json`](../hardware.json). Every field is required.

| Field | Values | Default |
|---|---|---|
| `board` | Name made of letters, digits, `_` and `-`. It becomes part of the hardware ID in the boot log. | `m5stack-nanoc6` |
| `led_count` | 1–512 | `60` |
| `timing` | `sk6812`, `ws2812`, `ws2812b`, `ws2811`, `ws2811_low_speed` | `sk6812` |
| `color_order` | `rgb`, `rbg`, `grb`, `gbr`, `brg`, `bgr` | `grb` |
| `data_gpio` | GPIO number 0–30 | `2` |
| `button_gpio` | GPIO number, or `null` for no button. Active-low, with the internal pull-up. | `9` |
| `power_gpio` | GPIO that switches the LED power on and off, or `null` | `null` |
| `power_active_high` | `true` or `false`. Required even when `power_gpio` is `null`. | `true` |
| `frame_interval_us` | 1000–1000000 | `16667` (about 60 fps) |

Example for the NanoC6's built-in RGB LED:

```json
{
  "board": "m5stack-nanoc6-internal",
  "led_count": 1,
  "timing": "ws2812",
  "color_order": "grb",
  "data_gpio": 20,
  "button_gpio": 9,
  "power_gpio": 19,
  "power_active_high": true,
  "frame_interval_us": 16667
}
```

> [!IMPORTANT]
> The image's application name includes a hash of the exact bytes of
> `hardware.json`. An OTA update is only accepted when that file is
> byte-for-byte identical to the one in the running image. Otherwise the device
> rejects the update with `image_identity`. To change the hardware profile of a
> device, reinstall it over USB.

## Device settings

Settings live in the `device_config` flash partition. They survive OTA updates
and USB reinstalls that use the documented `espflash flash` command. They are
erased by `provision.py erase-config` or by a full flash erase
(`espflash erase-flash`).

Without settings, the device still drives the LEDs and button and waits for USB
provisioning. It doesn't start a setup access point.

### Settings file

The tool accepts JSON or YAML. Start from
[`tools/device.example.json`](../tools/device.example.json) and keep the file
under `local/`, which Git ignores.

| Field | Required | Rules |
|---|---|---|
| `wifi_ssid` | yes | 1–32 bytes |
| `wifi_password` | yes | WPA2: 8–63 characters, or 64 hex digits. Open networks: `""`. |
| `wifi_auth` | no | `wpa2_personal` (default) or `open` |
| `mqtt_broker` | yes | Host name or IPv4 address made of letters, digits, `.` and `-`, up to 128 bytes. No scheme or port. |
| `mqtt_port` | no | Default `1883`; normally `8883` with TLS |
| `mqtt_tls` | no | Default `false`. Enable MQTT over TLS. |
| `mqtt_username` | no | Up to 128 bytes. The device logs in only if the username or password is non-empty. |
| `mqtt_password` | no | Up to 128 bytes |
| `schema` | no | Must be `2` |

Unknown fields are rejected.

YAML example:

```yaml
wifi_ssid: MyNetwork
wifi_password: correct-horse-battery
mqtt_broker: homeassistant.local
mqtt_username: led
mqtt_password: secret
```

The broker name is resolved through DNS (A records only). Multicast names such
as `.local` work only if the DHCP-provided DNS server resolves them. If it
doesn't, use an IPv4 address.

### MQTT over TLS

Set `mqtt_tls` to `true` and `mqtt_port` to the broker's TLS port (normally
8883). The certificate must match `mqtt_broker` and chain to one of the seven
[bundled public roots](../src/ROOTS.md). Private CA and self-signed LAN broker
certificates require adding the trusted root and rebuilding. The example uses
plaintext port 1883, as used by a default Home Assistant Mosquitto broker.
TLS never falls back to plaintext. Certificate dates use the
[build-time policy](OTA.md#certificate-dates).

MQTTS works independently of the OTA build options. MQTT and HTTPS OTA share
TLS initialization but use separate connections.

Existing schema 1 settings require `erase-config` followed by USB provisioning
with schema 2. Downgrading to older firmware also requires reprovisioning.

### Provisioning tool

```sh
uv run tools/provision.py status       --port $PORT
uv run tools/provision.py provision    --port $PORT --config local/device.json
uv run tools/provision.py erase-config --port $PORT
```

The tool opens the USB serial port (which may reset the board), sends one
command and waits up to 30 s (`--timeout`) for the answer. Credentials are read
from the file, never from the command line. Replies carry only metadata, for
example `{"id":"…","status":"saved","schema":2,"generation":2}`. The
`generation` count goes up by one with every save.

| Status | Meaning |
|---|---|
| `configured` / `unconfigured` | Reply to `status` |
| `saved` / `erased` | Done. The device reboots. |
| `invalid` | The command or a setting failed validation. Nothing was written. |
| `too_large` | The command was over 4224 bytes |
| `busy` | Flash is in use by an OTA update or boot check. Try again later. |
| `storage_error` | The flash write failed. The device reboots and loads whatever was stored. |

The device reboots after every save or erase attempt, including failed ones.
This keeps the running settings the same as the stored ones.

## Build options

| Variable | Effect |
|---|---|
| `ESP_LOG` | Log filter built into the image, read by esp-println: `error`, `warn`, `info`, `debug` or `trace`, optionally followed by per-module filters. Default `info`, set in `.cargo/config.toml`. A value in the environment takes precedence. |
| `OTA_ALLOWED_HOSTS`, `OTA_ROLLBACK_BOOTLOADER`, `OTA_ALLOW_HTTP`, `OTA_BUILD_ID`, `SOURCE_DATE_EPOCH` | OTA and build identity. See [OTA build variables](OTA.md#build-variables). |

Changing any of these variables triggers a rebuild.

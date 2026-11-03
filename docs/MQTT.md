# MQTT reference

Each device has a topic prefix `esp32_led_mqtt_XXXXXX`. `XXXXXX` is the last six
hex digits of its MAC address, and the boot log prints the full prefix
(`Device identity …`). The same string is used as the MQTT client ID and as the
Home Assistant unique ID.

The device connects with MQTT 5, a 30-second keepalive and a last will that
sets `status` to `offline`. Everything the device publishes is retained. After
a lost connection it retries every 5 seconds.

## Topics

| Topic | Direction | Payload |
|---|---|---|
| `PREFIX/status` | device → | `online` or `offline` |
| `PREFIX/light/set` | → device | Light command (JSON, see below) |
| `PREFIX/light/state` | device → | Light state (JSON) |
| `PREFIX/effect_speed/set` | → device | Integer `1`–`128` as text |
| `PREFIX/effect_speed/state` | device → | Integer `1`–`128` as text |
| `PREFIX/temperature/state` | device → | Chip temperature in °C as text, one decimal. Published on connect and every 60 s. |
| `PREFIX/firmware/state` | device → | Running build ID (as text), published on connect |
| `PREFIX/ota/set` | → device | OTA command. Must not be retained. See [OTA updates](OTA.md#send-an-update). |
| `PREFIX/ota/status` | device → | OTA status (JSON). See [status messages](OTA.md#status-messages). |
| `homeassistant/light/PREFIX/config` | device → | Discovery for the light |
| `homeassistant/number/PREFIX_effect_speed/config` | device → | Discovery for the speed slider |
| `homeassistant/sensor/PREFIX_temperature/config` | device → | Discovery for the chip temperature sensor |
| `homeassistant/sensor/PREFIX_firmware/config` | device → | Discovery for the firmware version sensor |

## Light command

Every field is optional, and fields you leave out keep their current values:

```json
{"state":"ON","brightness":128,"color":{"r":255,"g":0,"b":64},"effect":"Pacifica"}
```

- `state`: `ON` or `OFF`, case-insensitive
- `brightness`: 0–255. Gamma correction is applied on output.
- `color`: RGB, each 0–255. Shown when the effect is `None`, and used as the
  base color by some effects.
- `effect`: an effect name, case-insensitive, or `None` for the solid color.
  The full list is in the light discovery payload's `effect_list` and in the
  [README](../README.md#home-assistant).

The device ignores payloads that aren't valid JSON. An unknown effect name is
ignored, but the other fields in the same command are still applied.

The device answers on `PREFIX/light/state`:

```json
{"state":"ON","brightness":8,"color_mode":"rgb","color":{"r":255,"g":96,"b":24},"effect":"Rainbow"}
```

## Effect speed

`64` (the default) is normal speed. The scale is exponential: every 16 steps
double or halve the speed, so `1` is about 1/16× and `128` is 16×. Changing the
speed never makes an effect jump.

```sh
mosquitto_pub -h BROKER -t PREFIX/effect_speed/set -m 20
```

## Home Assistant discovery

On each connection the device publishes a JSON-schema light (`LED Strip`) and a
number entity (`Effect Speed`, slider mode) and a diagnostic temperature sensor
(`Chip Temperature`, die temperature) and a diagnostic `Firmware Version` sensor. All belong to a device named
**ESP32 LED MQTT**. All entities use `PREFIX/status` for availability.

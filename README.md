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

The default LED hardware configuration is grouped near the top of
`src/bin/main.rs`:

- `LED_COUNT` sets the number of LEDs.
- The `LedStrip` type selects the controller timing and color order. The default
  uses `Sk68xxTiming` with `color_order::Grb`.
- The LED data pin is the direct `peripherals.GPIO2` argument passed to
  `LedStrip::new_with_memsize`; replace that field with another ESP HAL GPIO
  field for your board.
- Optional strip power control is configured by `led_power_pin` in `main`.
  The default is `None`. To drive an active-high power-enable GPIO, change it
  to `Some(peripherals.GPIOx.into())`; change `LED_POWER_ACTIVE_LEVEL` to
  `esp_hal::gpio::Level::Low` if the enable pin is active-low.
- Optional active-low button input is configured by `button_pin` in `main`.
  The default uses the M5Stack NanoC6 button on GPIO9. Set it to `None` if your
  board has no button. A short press cycles effects, including `None`; a double
  press changes the current RGB color; a long press toggles the light on or off.

If you do not have the separate LED strip available, the M5Stack NanoC6
internal LED can be used instead:

- Set `LED_COUNT` to `1`.
- In the `LedStrip` type, use `esp_hal_smartled::Ws2812Timing`.
- Set `led_power_pin` to `Some(peripherals.GPIO19.into())`.
- Pass `peripherals.GPIO20` to `LedStrip::new_with_memsize`.

`esp-hal-smartled2` also provides these controller timing types:

- `Sk68xxTiming`
- `Ws2812bTiming`
- `Ws2812Timing`
- `Ws2811Timing`
- `Ws2811LowSpeedTiming`

Other RGB color-order options are:

- `color_order::Rgb`
- `color_order::Rbg`
- `color_order::Grb`
- `color_order::Gbr`
- `color_order::Brg`
- `color_order::Bgr`

Set your Wi-Fi and MQTT details in `src/secrets.rs`, or copy the example
secrets file and adjust it for local ignored credentials:

```sh
cp secrets.example.yaml secrets.yaml
```

When `secrets.yaml` exists, it overrides `src/secrets.rs`. It is ignored by git
and must not be committed. `wifi_ssid`, `wifi_password`, and `mqtt_broker` are
required. `mqtt_broker` may be a bare IPv4 address or a DNS hostname.
`mqtt_port` defaults to `1883` when omitted, and MQTT username/password may be
omitted when the broker does not use authentication. Editing `src/secrets.rs`
modifies a tracked file; use `secrets.yaml` for private local credentials.

## Flash

Connect the ESP32-C6 over USB and run:

```sh
cargo run --release
```

Cargo uses the configured runner:

```sh
espflash flash --monitor --chip esp32c6
```

In non-interactive shells the flash can succeed even if the monitor cannot
attach afterward.

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
3. Add the `EffectId` variant and `EffectDefinition` entry in `src/lib.rs`.
4. Add the effect to the animation tests when it should change over time.

Home Assistant discovery, name parsing, button cycling, and numeric effect
codes are derived from `EFFECT_DEFINITIONS`.

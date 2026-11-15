//! Two or three waves along the strip shown as colour changes at full
//! brightness, never as dark nodes. The variation sets the rate of the last
//! wave relative to the first: 0 stops it, 128 matches it, 255 nearly doubles
//! it. LED 0 is the bottom, so "up" is rising index.

use super::sin_q16;
use crate::{EffectRuntime, hsv_rainbow16, phase_at};
use smart_leds::RGB8;

#[derive(Clone, Copy)]
pub(super) enum Variant {
    /// Counter-propagating waves bend the hue: nodes keep the base hue,
    /// antinodes swing a third of the wheel either way.
    ColorInterference,
    /// One wave per colour channel, each with its own length and direction,
    /// scaled so the brightest channel is always full.
    ChromaWaves,
    /// Two rainbows in opposite directions mixed: vivid where their hues
    /// agree, pale where they oppose.
    RainbowBeats,
}

const HUE_MS: u32 = 120_000;
/// A third of the colour wheel at full field, in Q16 hue.
const HUE_SWING: i32 = 0x5555;

/// Wavelengths in 1/256 LED.
const fn lambda(leds_x10: u32) -> u32 {
    leds_x10 * 256 / 10
}

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, variant: Variant) {
    let first_us = runtime.clock_us;
    let second_us = runtime.variation_clock_us();
    let hue = runtime.phase(HUE_MS);
    let pixel = |index: usize| -> RGB8 {
        match variant {
            Variant::ColorInterference => {
                let up = wave(index, lambda(150), first_us, 3_000, Direction::Up);
                let down = wave(index, lambda(150), second_us, 3_000, Direction::Down);
                bent_hue(hue, (up + down) / 2)
            }
            Variant::ChromaWaves => {
                let r = wave(index, lambda(230), first_us, 5_000, Direction::Up);
                let g = wave(index, lambda(170), first_us, 4_000, Direction::Down);
                let b = wave(index, lambda(130), second_us, 3_000, Direction::Up);
                // The floor bounds the gain where all three are low, which
                // would otherwise amplify rounding into flicker.
                normalized([r, g, b].map(|wave| ((wave + 65_536) >> 9) as u16 + 64))
            }
            Variant::RainbowBeats => {
                let up = space_phase(index, lambda(300)).wrapping_sub(phase_at(first_us, 6_000));
                let down = space_phase(index, lambda(200)).wrapping_add(phase_at(second_us, 6_000));
                let a = hsv_rainbow16(up.wrapping_add(hue), 255, 255);
                let b = hsv_rainbow16(down.wrapping_add(hue), 255, 255);
                normalized([
                    u16::from(a.r) + u16::from(b.r),
                    u16::from(a.g) + u16::from(b.g),
                    u16::from(a.b) + u16::from(b.b),
                ])
            }
        }
    };
    for (index, out) in runtime.frame.iter_mut().enumerate() {
        *out = pixel(index);
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Down,
}

/// Phase of `index` along a wave of `lambda_q8` (1/256 LED).
fn space_phase(index: usize, lambda_q8: u32) -> u16 {
    ((index as u64 * (65_536 << 8)) / u64::from(lambda_q8)) as u16
}

/// Sine in Q16 of a wave travelling one wavelength per `period_ms`.
fn wave(index: usize, lambda_q8: u32, time_us: u64, period_ms: u32, direction: Direction) -> i32 {
    let space = space_phase(index, lambda_q8);
    let time = phase_at(time_us, period_ms);
    sin_q16(match direction {
        Direction::Up => space.wrapping_sub(time),
        Direction::Down => space.wrapping_add(time),
    })
}

/// Full-brightness colour whose hue is `hue` bent by a field in Q16.
fn bent_hue(hue: u16, field: i32) -> RGB8 {
    let offset = (i64::from(field) * i64::from(HUE_SWING)) >> 16;
    hsv_rainbow16(hue.wrapping_add(offset as u16), 255, 255)
}

/// Scales the channels so the brightest is full: a dim or grey mix becomes
/// a pale tint, never a dark pixel.
fn normalized(channels: [u16; 3]) -> RGB8 {
    let peak = channels.into_iter().max().unwrap_or(0).max(1);
    let [r, g, b] = channels.map(|value| (u32::from(value) * 255 / u32::from(peak)) as u8);
    RGB8 { r, g, b }
}

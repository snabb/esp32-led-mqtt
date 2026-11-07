//! A sine rising and a sawtooth falling, each in its own colour and mixed
//! additively. The variation sets the sawtooth's rate relative to the sine:
//! 0 stops it, 128 matches it and 255 nearly doubles it. LED 0 is the bottom.

use super::sin_q16;
use crate::{EffectRuntime, hsv_rainbow16, phase_at};
use smart_leds::RGB8;

const HUE_MS: u32 = 120_000;
/// The sawtooth is a third of the colour wheel away from the sine.
const SAWTOOTH_HUE: u16 = 0x5555;
/// Wavelengths in 1/256 LED.
const SINE_LAMBDA: u32 = 20 << 8;
const SAWTOOTH_LAMBDA: u32 = 12 << 8;
const SINE_MS: u32 = 4_000;
const SAWTOOTH_MS: u32 = 3_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let sine_phase = phase_at(runtime.clock_us, SINE_MS);
    let sawtooth_phase = phase_at(runtime.variation_clock_us(), SAWTOOTH_MS);
    let hue = runtime.phase(HUE_MS);
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let rising = sin_q16(space_phase(index, SINE_LAMBDA).wrapping_sub(sine_phase));
        let rising = ((rising + 65_536) >> 9).min(255) as u8;
        let falling = sawtooth(space_phase(index, SAWTOOTH_LAMBDA).wrapping_add(sawtooth_phase));
        let a = hsv_rainbow16(hue, 255, rising);
        let b = hsv_rainbow16(hue.wrapping_add(SAWTOOTH_HUE), 255, falling);
        *pixel = RGB8 {
            r: a.r.saturating_add(b.r),
            g: a.g.saturating_add(b.g),
            b: a.b.saturating_add(b.b),
        };
    }
}

/// Phase of `index` along a wave of `lambda_q8` (1/256 LED).
fn space_phase(index: usize, lambda_q8: u32) -> u16 {
    ((index as u64 * (65_536 << 8)) / u64::from(lambda_q8)) as u16
}

/// A tooth with a bright leading edge at its bottom, fading towards the top.
/// The edge rises over ~1.5 LEDs so it never pops.
fn sawtooth(phase: u16) -> u8 {
    // Distance behind the leading edge as a fraction of the tooth.
    let tail = u64::from(phase);
    let rise_len = 65_536 * (3 << 7) / u64::from(SAWTOOTH_LAMBDA);
    let rise = (tail * 65_536 / rise_len).min(65_536);
    let fade = 65_536 - tail;
    let fade = (fade * fade) >> 16;
    ((rise * fade) >> 24).min(255) as u8
}

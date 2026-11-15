//! Grains of coloured sand fall from the top and pile up in layers from LED 0
//! (the bottom). When the strip is full, the pile slides out of the bottom and
//! a new one starts in new colours.

use super::hash;
use crate::{EffectRuntime, blend_rgb, hsv_rainbow16, phase_at};
use smart_leds::RGB8;

const GRAIN_US: u64 = 350_000;
/// Time to fall the whole strip from rest.
const FALL_US: u64 = 1_500_000;
const HOLD_US: u64 = 2_500_000;
const GAP_US: u64 = 600_000;
const LAYER: usize = 6;
const HUE_MS: u32 = 200_000;
/// Trail length above a falling grain, in 1/256 LED.
const TRAIL_Q8: i64 = 3 * 256;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let drain_us = N as u64 * GRAIN_US + HOLD_US;
    let cycle_us = drain_us + FALL_US + GAP_US;
    let (cycle, t) = (runtime.clock_us / cycle_us, runtime.clock_us % cycle_us);
    // The pile takes its colours from when it started, opposite the background.
    let palette = Palette::new(cycle, phase_at(cycle * cycle_us, HUE_MS));
    let background = hsv_rainbow16(runtime.phase(HUE_MS), 230, 120);

    let fallen = |grain: usize| fall_q8::<N>(t.saturating_sub(grain as u64 * GRAIN_US));
    let landed = |grain: usize| fallen(grain) >= ((N - grain) * 256) as i64;
    let cell = |grain: usize| {
        if grain < N && landed(grain) {
            palette.grain(grain)
        } else {
            background
        }
    };
    let drop_q8 = if t > drain_us {
        fall_q8::<N>(t - drain_us)
    } else {
        0
    };
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let position = index as i64 * 256 + drop_q8;
        let grain = (position >> 8) as usize;
        *pixel = blend_rgb(cell(grain), cell(grain + 1), position as u8);
    }

    let spawned = (t / GRAIN_US + 1).min(N as u64) as usize;
    for grain in (0..spawned).filter(|&grain| !landed(grain)) {
        let y_q8 = (N * 256) as i64 - fallen(grain);
        // The trail shrinks over the last two LEDs so it doesn't vanish on landing.
        let trail_scale = ((y_q8 - (grain * 256) as i64) / 2).min(256);
        let color = palette.grain(grain);
        for (index, pixel) in runtime.frame.iter_mut().enumerate() {
            let offset = index as i64 * 256 - y_q8;
            let head = 256 - offset.abs();
            let trail = if (0..TRAIL_Q8).contains(&offset) {
                96 * (TRAIL_Q8 - offset) / TRAIL_Q8 * trail_scale / 256
            } else {
                0
            };
            let weight = head.max(trail);
            if weight > 0 {
                *pixel = blend_rgb(*pixel, color, weight.min(255) as u8);
            }
        }
    }
}

/// Distance fallen from rest after `elapsed_us`, in 1/256 LED.
fn fall_q8<const N: usize>(elapsed_us: u64) -> i64 {
    let elapsed = u128::from(elapsed_us);
    (N as u128 * 256 * elapsed * elapsed / u128::from(FALL_US * FALL_US)) as i64
}

struct Palette {
    seed: u32,
    base: u16,
    /// Hue between the three alternating layer colours.
    spread: u16,
    /// Slow hue drift from layer to layer.
    drift: u16,
}

impl Palette {
    fn new(cycle: u64, background_hue: u16) -> Self {
        let seed = hash(cycle as u32);
        let base = background_hue
            .wrapping_add(0x8000)
            .wrapping_add((seed % 0x4000) as u16)
            .wrapping_sub(0x2000);
        let spread = 0x1800 + ((seed >> 14) % 0x1c00) as u16;
        let mut drift = 0x0200 + ((seed >> 24) % 0x40) as u16 * 0x10;
        if seed >> 31 == 1 {
            drift = drift.wrapping_neg();
        }
        Self {
            seed,
            base,
            spread,
            drift,
        }
    }

    /// Layers cycle through three colours that drift as the pile grows; each
    /// grain varies a little, like sand.
    fn grain(&self, grain: usize) -> RGB8 {
        let layer = (grain / LAYER) as u16;
        let jitter = hash(self.seed ^ (grain as u32).wrapping_mul(0x9e37_79b1));
        let hue = self
            .base
            .wrapping_add(self.spread.wrapping_mul(layer % 3))
            .wrapping_add(self.drift.wrapping_mul(layer))
            .wrapping_add((jitter % 0x0800) as u16);
        hsv_rainbow16(hue, 255, 210 + ((jitter >> 16) % 46) as u8)
    }
}

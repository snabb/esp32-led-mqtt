use super::{BLUE, WHITE, band, hash, noise_time, value_noise};
use crate::effects::{add_smooth_dot, strip_position16};
use crate::{EffectRuntime, blend_rgb, ease_wave16, scale_rgb, scale8};

const SPAWN_MS: u64 = 720;
const MIN_FALL_MS: u64 = 5_000;
const MAX_FALL_MS: u64 = 11_000;
/// Enough spawn slots to cover the slowest flake's whole fall.
const FLAKE_SLOTS: u64 = MAX_FALL_MS / SPAWN_MS + 1;

/// Flake fade-in and fade-out distance, as `u16` strip position: two LEDs of 60.
const FLAKE_FADE: u64 = 2_200;

const DRIFT_CYCLE_MS: u64 = 100_000;
const MELT_MS: u64 = 18_000;
/// Deepest the drift gets, as `u16` strip position: a third of the strip.
const DRIFT_DEPTH: u64 = 21_845;

/// Height of the drift over its cycle: slow build-up, then a quicker melt.
fn drift_height(clock_ms: u64) -> u64 {
    let build_ms = DRIFT_CYCLE_MS - MELT_MS;
    let at = clock_ms % DRIFT_CYCLE_MS;
    if at < build_ms {
        at * DRIFT_DEPTH / build_ms
    } else {
        let melted = (at - build_ms) * 32767 / MELT_MS;
        DRIFT_DEPTH * (65535 - u64::from(ease_wave16(melted as u16))) / 65535
    }
}

/// Flakes fall at their own speeds and pile up in a drift on the ground, which
/// grows until it melts away. Faster flakes are nearer, so brighter.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_us = runtime.clock_us;
    let clock_ms = clock_us / 1000;
    let surface = drift_height(clock_ms) as i32;
    let soft = 65536 / N as i32;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let y = i32::from(strip_position16::<N>(index)) + soft / 2;
        let snow = band(y, -soft, surface, soft);
        // Bluish at the fresh surface, packed white further down, with a little glitter.
        let packing = ((surface - y).max(0) / 16).min(255) as u8;
        let glitter = value_noise(index as u32, noise_time(clock_us, 240)) / 4;
        let drift = scale_rgb(
            blend_rgb(scale_rgb(BLUE, 180), WHITE, packing.saturating_add(90)),
            192 + glitter,
        );
        *pixel = blend_rgb(scale_rgb(BLUE, 32), drift, snow);
    }

    let newest = clock_ms / SPAWN_MS;
    let fall_depth = 65535 - surface as u64;
    for flake in newest.saturating_sub(FLAKE_SLOTS)..=newest {
        let span = u64::from(hash(flake as u32) % 256);
        let fall_ms = MAX_FALL_MS - (MAX_FALL_MS - MIN_FALL_MS) * span / 255;
        let age_ms = clock_ms.saturating_sub(flake * SPAWN_MS);
        let fallen = age_ms * 65535 / fall_ms;
        let room = fall_depth.saturating_sub(fallen).min(fallen);
        if room > 0 {
            // Fades at both ends: flakes must not pop in at the sky or out at the drift.
            let fade = (room * 255 / FLAKE_FADE).min(255) as u8;
            let level = scale8(90 + (span * 165 / 255) as u8, fade);
            add_smooth_dot(
                &mut runtime.frame,
                (65535 - fallen) as u16,
                scale_rgb(WHITE, level),
            );
        }
    }
}

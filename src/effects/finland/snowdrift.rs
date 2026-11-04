use super::{BLUE, WHITE, hash};
use crate::effects::add_smooth_dot;
use crate::{EffectRuntime, scale_rgb, scale8};

const SPAWN_MS: u64 = 720;
const MIN_FALL_MS: u64 = 5_000;
const MAX_FALL_MS: u64 = 11_000;
/// Enough spawn slots to cover the slowest flake's whole fall.
const FLAKE_SLOTS: u64 = MAX_FALL_MS / SPAWN_MS + 1;
/// Flake fade-in and fade-out distance, as `u16` strip position: two LEDs of 60.
const FLAKE_FADE: u64 = 2_200;

/// Flakes fall at their own speeds and vanish at the bottom. Faster flakes are
/// nearer, so brighter.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_ms = runtime.clock_us / 1000;
    runtime.frame.fill(scale_rgb(BLUE, 32));

    let newest = clock_ms / SPAWN_MS;
    for flake in newest.saturating_sub(FLAKE_SLOTS)..=newest {
        let span = u64::from(hash(flake as u32) % 256);
        let fall_ms = MAX_FALL_MS - (MAX_FALL_MS - MIN_FALL_MS) * span / 255;
        let age_ms = clock_ms.saturating_sub(flake * SPAWN_MS);
        let fallen = age_ms * 65535 / fall_ms;
        let room = 65535_u64.saturating_sub(fallen).min(fallen);
        if room > 0 {
            // Fades at both ends: flakes must not pop in at the top or out at the bottom.
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

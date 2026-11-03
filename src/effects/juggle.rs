use super::{MAX_TICKS_PER_FRAME, add_smooth_dot};
use crate::{EffectRuntime, ease_wave16, fade_to_black, hsv_rainbow16, phase_at};

const TICK_MS: u32 = 10;
const HUE_CYCLE_MS: u32 = 10_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, time_us| {
        for pixel in &mut runtime.tick_frame {
            *pixel = fade_to_black(*pixel, 20);
        }

        let hue_drift = phase_at(time_us, HUE_CYCLE_MS);
        for dot in 0..N.min(6) as u16 {
            // Dot `i` runs at 7 + i BPM, so the dots drift in and out of sync.
            let period_ms = 60_000 / (7 + u32::from(dot));
            let phase = phase_at(time_us, period_ms).wrapping_add(dot.wrapping_mul(0x2aaa));
            let hue = hue_drift.wrapping_add(dot << 13);
            add_smooth_dot(
                &mut runtime.tick_frame,
                ease_wave16(phase),
                hsv_rainbow16(hue, 220, 255),
            );
        }
    });
}

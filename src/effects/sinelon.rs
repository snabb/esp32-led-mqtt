use super::{MAX_TICKS_PER_FRAME, add_smooth_dot};
use crate::{EffectRuntime, ease_wave16, fade_to_black, phase_at};

/// 13 BPM, one full back-and-forth.
const PERIOD_MS: u32 = 4_600;
const TICK_MS: u32 = 10;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let primary = runtime.params.primary;
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, time_us| {
        for pixel in &mut runtime.tick_frame {
            *pixel = fade_to_black(*pixel, 20);
        }
        let wave = ease_wave16(phase_at(time_us, PERIOD_MS));
        add_smooth_dot(&mut runtime.tick_frame, wave, primary);
    });
}

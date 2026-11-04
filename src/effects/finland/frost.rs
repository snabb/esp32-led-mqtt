use super::{BLUE, WHITE};
use crate::effects::{MAX_TICKS_PER_FRAME, strip_position16};
use crate::{EffectRuntime, blend_rgb, ease_wave16, phase_at, scale_rgb};

const TICK_MS: u32 = 20;
const SWELL_MS: u32 = 13_000;
const GUST_MS: u32 = 9_000;
/// Half width of the glitter band a gust carries along the strip, as `u16` phase.
const GUST_REACH: u32 = 7_000;

/// Sharp ice glints on a slowly swelling blue, with gusts of glitter passing by.
/// Glints strike at once and die slowly, unlike the symmetric twinkles.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |r, tick_us| {
        let swell = phase_at(tick_us, SWELL_MS);
        let gust = u32::from(phase_at(tick_us, GUST_MS));
        for i in 0..N {
            let pos = strip_position16::<N>(i);
            let glint = r.effect_data[i];
            let faded = glint.saturating_sub(glint / 20 + 1);

            let distance = u32::from(pos)
                .abs_diff(gust)
                .min(65536 - u32::from(pos).abs_diff(gust));
            let boost = GUST_REACH.saturating_sub(distance) * 24 / GUST_REACH;
            r.effect_data[i] = if faded == 0 && u32::from(r.rng.next_u8()) <= boost {
                255
            } else {
                faded
            };

            let level = 60 + (ease_wave16(pos.wrapping_add(swell)) >> 8) as u8 / 3;
            r.tick_frame[i] = blend_rgb(scale_rgb(BLUE, level), WHITE, r.effect_data[i]);
        }
    });
}

use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, half_sin8, hsv_rainbow, scale_rgb};

const TICK_MS: u32 = 20;
const RANDOM_TICK_MS: u32 = 40;
/// 255 / 3 = 85 ticks per twinkle.
const DECAY: u8 = 3;

/// Twinkle in the primary color, or in random hues for Random Twinkle.
pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, random_color: bool) {
    let tick_ms = if random_color {
        RANDOM_TICK_MS
    } else {
        TICK_MS
    };
    runtime.simulate(tick_ms, MAX_TICKS_PER_FRAME, |runtime, _| {
        for index in 0..N {
            runtime.effect_data[index] = runtime.effect_data[index].saturating_sub(DECAY);
            if runtime.effect_data[index] == 0 && runtime.chance(runtime.params.intensity, 20) {
                runtime.effect_data[index] = 255;
                runtime.twinkle_color[index] = if random_color {
                    hsv_rainbow(runtime.rng.next_u8(), 220, 255)
                } else {
                    runtime.params.primary
                };
            }
            let level = half_sin8(runtime.effect_data[index]);
            runtime.tick_frame[index] = scale_rgb(runtime.twinkle_color[index], level);
        }
    });
}

use crate::{EffectRuntime, half_sin8, scale_rgb, speed_interval};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    render_with_color_mode(runtime, now_ms, false);
}

pub(super) fn render_with_color_mode<const N: usize>(
    runtime: &mut EffectRuntime<N>,
    now_ms: u32,
    random_color: bool,
) {
    let interval = if random_color { 32 } else { 4 };
    if runtime.elapsed(now_ms, speed_interval(interval, runtime.params.speed)) {
        for index in 0..N {
            runtime.effect_data[index] = runtime.effect_data[index].saturating_sub(8);
            if runtime.effect_data[index] == 0 && runtime.chance(runtime.params.intensity, 20) {
                runtime.effect_data[index] = 255;
                runtime.twinkle_color[index] = if random_color {
                    crate::hsv_rainbow(runtime.rng.next_u8(), 220, 255)
                } else {
                    runtime.params.primary
                };
            }
        }
    }

    for index in 0..N {
        let level = half_sin8(runtime.effect_data[index]);
        runtime.frame.as_mut_slice()[index] = scale_rgb(runtime.twinkle_color[index], level);
    }
}

use crate::{EffectRuntime, scale_rgb, speed_interval};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(16, runtime.params.speed)) {
        for index in 0..N {
            let flicker = 224_u8.saturating_add(runtime.rng.next_u8() >> 3);
            runtime.frame.as_mut_slice()[index] = scale_rgb(runtime.params.primary, flicker);
        }
    }
}

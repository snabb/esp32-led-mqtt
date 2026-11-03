use super::wave_phase;
use crate::{EffectRuntime, half_sin8, scale_rgb};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let wave = wave_phase(now_ms, runtime.params.speed, 96);
    let level = half_sin8(wave).saturating_add(8);
    runtime.frame.fill(scale_rgb(runtime.params.primary, level));
}

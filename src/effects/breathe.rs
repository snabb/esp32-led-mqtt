use crate::{EffectRuntime, half_sin8, scale_rgb};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let speed = u32::from(runtime.params.speed.max(1));
    let wave = ((now_ms.saturating_mul(speed) / 96) & 0xff) as u8;
    let level = half_sin8(wave).saturating_add(8);
    runtime
        .frame
        .set_all(scale_rgb(runtime.params.primary, level));
}

use crate::{EffectRuntime, ease_wave16, scale_rgb};

const BREATH_MS: u32 = 5_000;
const FLOOR: u8 = 8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let wave = ease_wave16(runtime.phase(BREATH_MS));
    let level = FLOOR + (u32::from(wave) * u32::from(255 - FLOOR) / u32::from(u16::MAX)) as u8;
    runtime.frame.fill(scale_rgb(runtime.params.primary, level));
}

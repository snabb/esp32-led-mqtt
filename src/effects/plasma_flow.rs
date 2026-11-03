use super::strip_position16;
use crate::{EffectRuntime, half_sin16, hsv_rainbow16, scale8};

const WAVE_A_MS: u32 = 6_500;
const WAVE_B_MS: u32 = 10_100;
const WAVE_C_MS: u32 = 14_100;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let phase_a = runtime.phase(WAVE_A_MS);
    let phase_b = runtime.phase(WAVE_B_MS);
    let phase_c = runtime.phase(WAVE_C_MS);
    let brightness_floor = 64_u8.saturating_add(runtime.params.intensity / 8);
    let hue_offset = phase_b.wrapping_add(u16::from(runtime.params.primary.r / 2) << 8);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position16::<N>(index);
        let wave_a = half_sin16(pos.wrapping_add(phase_a));
        let wave_b = half_sin16(pos.wrapping_mul(2).wrapping_sub(phase_b));
        let wave_c = half_sin16(pos.wrapping_mul(5).wrapping_add(phase_c));
        let mixed = ((u16::from(wave_a) + u16::from(wave_b) + u16::from(wave_c)) / 3) as u8;
        let hue = (u16::from(mixed) << 8).wrapping_add(hue_offset);
        let value = brightness_floor.saturating_add(scale8(mixed, 255 - brightness_floor));

        *pixel = hsv_rainbow16(hue, 240, value);
    }
}

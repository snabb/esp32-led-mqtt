use super::{strip_position, wave_phase};
use crate::{EffectRuntime, half_sin8, hsv_rainbow, scale8};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let phase_a = wave_phase(now_ms, runtime.params.speed, 97);
    let phase_b = wave_phase(now_ms, runtime.params.speed, 151);
    let phase_c = wave_phase(now_ms, runtime.params.speed, 211);
    let brightness_floor = 64_u8.saturating_add(runtime.params.intensity / 8);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position::<N>(index);
        let wave_a = half_sin8(pos.wrapping_add(phase_a));
        let wave_b = half_sin8(pos.wrapping_mul(2).wrapping_sub(phase_b));
        let wave_c = half_sin8(pos.wrapping_mul(5).wrapping_add(phase_c));
        let mixed = ((u16::from(wave_a) + u16::from(wave_b) + u16::from(wave_c)) / 3) as u8;
        let hue = mixed
            .wrapping_add(phase_b)
            .wrapping_add(runtime.params.primary.r / 2);
        let value = brightness_floor.saturating_add(scale8(mixed, 255 - brightness_floor));

        *pixel = hsv_rainbow(hue, 240, value);
    }
}

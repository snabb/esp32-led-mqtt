use super::strip_position16;
use crate::{EffectRuntime, half_sin16, scale8};
use smart_leds::RGB8;

const WAVE_A_MS: u32 = 10_000;
const WAVE_B_MS: u32 = 15_400;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let phase_a = runtime.phase(WAVE_A_MS);
    let phase_b = runtime.phase(WAVE_B_MS);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position16::<N>(index);
        let wave_a = half_sin16(pos.wrapping_add(phase_a));
        let wave_b = half_sin16(pos.wrapping_mul(2).wrapping_sub(phase_b));
        let green = scale8(wave_a, 140).saturating_add(16);
        let blue = scale8(wave_b.saturating_add(wave_a / 2), 210).saturating_add(32);
        let white = scale8(wave_a.saturating_sub(190), 80);
        *pixel = RGB8 {
            r: white,
            g: green.saturating_add(white),
            b: blue.saturating_add(white),
        };
    }
}

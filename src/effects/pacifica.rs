use super::{strip_position, wave_phase};
use crate::{EffectRuntime, half_sin8, scale8};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let phase_a = wave_phase(now_ms, runtime.params.speed, 128);
    let phase_b = wave_phase(now_ms, runtime.params.speed, 197);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position::<N>(index);
        let wave_a = half_sin8(pos.wrapping_add(phase_a));
        let wave_b = half_sin8(pos.wrapping_mul(2).wrapping_sub(phase_b));
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

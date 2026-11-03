use super::{strip_position, wave_phase};
use crate::{EffectRuntime, half_sin8, scale8};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let phase_a = wave_phase(now_ms, runtime.params.speed, 181);
    let phase_b = wave_phase(now_ms, runtime.params.speed, 313);
    let phase_c = wave_phase(now_ms, runtime.params.speed, 89);
    let shimmer_gain = runtime.params.intensity.max(32);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position::<N>(index);
        let curtain = half_sin8(pos.wrapping_add(phase_a));
        let fold = half_sin8(pos.wrapping_mul(3).wrapping_sub(phase_b));
        let shimmer_wave = half_sin8(pos.wrapping_mul(7).wrapping_add(phase_c));
        let shimmer = scale8(shimmer_wave.saturating_sub(212), shimmer_gain);

        let green = scale8(curtain, 170).saturating_add(scale8(fold, 80));
        let blue = scale8(curtain, 95).saturating_add(scale8(fold, 185));
        let violet = scale8(fold.saturating_sub(curtain / 2), 130);

        *pixel = RGB8 {
            r: violet.saturating_add(shimmer),
            g: green.saturating_add(shimmer),
            b: blue.saturating_add(shimmer),
        };
    }
}

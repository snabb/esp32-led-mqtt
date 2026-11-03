use super::strip_position16;
use crate::{EffectRuntime, half_sin16, scale8};
use smart_leds::RGB8;

const CURTAIN_MS: u32 = 12_000;
const FOLD_MS: u32 = 20_750;
const SHIMMER_MS: u32 = 5_900;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let phase_a = runtime.phase(CURTAIN_MS);
    let phase_b = runtime.phase(FOLD_MS);
    let phase_c = runtime.phase(SHIMMER_MS);
    let shimmer_gain = runtime.params.intensity.max(32);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position16::<N>(index);
        let curtain = half_sin16(pos.wrapping_add(phase_a));
        let fold = half_sin16(pos.wrapping_mul(3).wrapping_sub(phase_b));
        let shimmer_wave = half_sin16(pos.wrapping_mul(7).wrapping_add(phase_c));
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

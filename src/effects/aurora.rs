use crate::{EffectRuntime, half_sin8, scale8};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let speed = u32::from(runtime.params.speed.max(1));
    let phase_a = ((now_ms.saturating_mul(speed) / 181) & 0xff) as u8;
    let phase_b = ((now_ms.saturating_mul(speed) / 313) & 0xff) as u8;
    let phase_c = ((now_ms.saturating_mul(speed) / 89) & 0xff) as u8;
    let shimmer_gain = runtime.params.intensity.max(32);

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let pos = ((index * 256) / N.max(1)) as u8;
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

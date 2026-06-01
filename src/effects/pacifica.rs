use crate::{EffectRuntime, half_sin8, scale8};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let speed = u32::from(runtime.params.speed.max(1));
    let phase_a = ((now_ms.saturating_mul(speed) / 128) & 0xff) as u8;
    let phase_b = ((now_ms.saturating_mul(speed) / 197) & 0xff) as u8;

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let pos = ((index * 256) / N.max(1)) as u8;
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

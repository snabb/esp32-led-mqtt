use crate::{EffectRuntime, fade_to_black, hsv_rainbow, speed_interval};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(24, runtime.params.speed)) {
        for pixel in &mut runtime.frame {
            *pixel = fade_to_black(*pixel, 32);
        }

        let index = (runtime.rng.next_u16() as usize) % N;
        let hue = runtime
            .params
            .primary
            .r
            .wrapping_add(runtime.rng.next_u8() & 0x3f);
        runtime.frame[index] = hsv_rainbow(hue, 200, 255);
    }
}

use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, fade_to_black, hsv_rainbow};

const TICK_MS: u32 = 30;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, _| {
        for pixel in &mut runtime.tick_frame {
            *pixel = fade_to_black(*pixel, 24);
        }

        let index = (runtime.rng.next_u16() as usize) % N;
        let hue = runtime
            .params
            .primary
            .r
            .wrapping_add(runtime.rng.next_u8() & 0x3f);
        runtime.tick_frame[index] = hsv_rainbow(hue, 200, 255);
    });
}

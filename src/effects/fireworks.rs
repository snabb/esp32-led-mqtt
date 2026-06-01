use crate::{EffectRuntime, fade_to_black, speed_interval};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(32, runtime.params.speed)) {
        for pixel in runtime.frame.as_mut_slice() {
            *pixel = fade_to_black(*pixel, 120);
        }

        let sparks = (u16::from(runtime.params.intensity) * N as u16) / 2550 + 1;
        for _ in 0..sparks {
            if runtime.chance(runtime.params.intensity, 10) {
                let index = (runtime.rng.next_u16() as usize) % N.max(1);
                runtime.frame.as_mut_slice()[index] = runtime.params.primary;
            }
        }
    }
}

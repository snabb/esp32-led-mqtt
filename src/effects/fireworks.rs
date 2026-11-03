use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, fade_to_black};

const TICK_MS: u32 = 40;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, _| {
        for pixel in &mut runtime.tick_frame {
            *pixel = fade_to_black(*pixel, 120);
        }

        let sparks = (u16::from(runtime.params.intensity) * N as u16) / 2550 + 1;
        for _ in 0..sparks {
            if runtime.chance(runtime.params.intensity, 10) {
                let index = (runtime.rng.next_u16() as usize) % N;
                runtime.tick_frame[index] = runtime.params.primary;
            }
        }
    });
}

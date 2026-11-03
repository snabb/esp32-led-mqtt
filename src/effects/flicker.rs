use crate::{EffectRuntime, scale_rgb};

const TICK_MS: u32 = 80;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    // Each tick replaces the whole frame, so skipped ticks are never visible.
    runtime.simulate(TICK_MS, 1, |runtime, _| {
        for index in 0..N {
            let flicker = 224_u8.saturating_add(runtime.rng.next_u8() >> 3);
            runtime.tick_frame[index] = scale_rgb(runtime.params.primary, flicker);
        }
    });
}

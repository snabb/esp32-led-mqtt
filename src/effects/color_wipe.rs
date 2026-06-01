use crate::{EffectRuntime, speed_interval};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(40, runtime.params.speed)) {
        runtime.wipe_index = (runtime.wipe_index + 1) % (N + 1);
    }

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        *pixel = if index < runtime.wipe_index {
            runtime.params.primary
        } else {
            RGB8 { r: 0, g: 0, b: 0 }
        };
    }
}

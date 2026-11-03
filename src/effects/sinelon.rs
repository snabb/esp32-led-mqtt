use super::util::beat_position;
use crate::{EffectRuntime, fade_to_black, speed_interval};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(20, runtime.params.speed)) {
        for pixel in &mut runtime.frame {
            *pixel = fade_to_black(*pixel, 40);
        }
    }

    if let Some(position) = beat_position(now_ms / 5, runtime.params.speed, N) {
        runtime.frame[position] = runtime.params.primary;
    }
}

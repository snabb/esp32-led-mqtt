use super::strip_position;
use crate::{EffectRuntime, half_sin16, hsv_rainbow, scale_rgb};

const BEAT_MS: u32 = 1_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let level = half_sin16(runtime.phase(BEAT_MS)).saturating_add(48);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let hue = strip_position::<N>(index);
        let base = hsv_rainbow(hue.wrapping_add(runtime.params.primary.r), 220, 255);
        *pixel = scale_rgb(base, level);
    }
}

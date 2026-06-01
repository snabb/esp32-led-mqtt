use crate::{EffectRuntime, half_sin8, hsv_rainbow, scale_rgb};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let bpm = 30 + u32::from(runtime.params.speed) / 2;
    let wave = (((now_ms / 4).saturating_mul(bpm) / 60) & 0xff) as u8;
    let level = half_sin8(wave).saturating_add(48);

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let hue = ((index * 256) / N.max(1)) as u8;
        let base = hsv_rainbow(hue.wrapping_add(runtime.params.primary.r), 220, 255);
        *pixel = scale_rgb(base, level);
    }
}

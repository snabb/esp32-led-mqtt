use crate::{EffectRuntime, hsv_rainbow};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let step = u16::from(runtime.params.speed.max(1)) * 3;
    runtime.phase = (now_ms as u16).wrapping_mul(step);
    let width = 50_u16.max(N as u16);
    let hue_delta = u16::MAX / width;

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let hue = runtime
            .phase
            .wrapping_add((index as u16).wrapping_mul(hue_delta));
        *pixel = hsv_rainbow((hue >> 8) as u8, 240, 255);
    }
}

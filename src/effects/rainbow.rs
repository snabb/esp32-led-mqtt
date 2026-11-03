use crate::{EffectRuntime, hsv_rainbow16};

const HUE_CYCLE_MS: u32 = 8_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let phase = runtime.phase(HUE_CYCLE_MS);
    let width = 50_u16.max(N as u16);
    let hue_delta = u16::MAX / width;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let hue = phase.wrapping_add((index as u16).wrapping_mul(hue_delta));
        *pixel = hsv_rainbow16(hue, 240, 255);
    }
}

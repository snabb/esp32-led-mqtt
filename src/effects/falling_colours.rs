//! One colour after another falls from the top (the last LED) and covers the
//! strip, the next starting as soon as the last reaches LED 0.

use crate::{EffectRuntime, blend_rgb, hsv_rainbow16};

const FALL_US: u64 = 2_500_000;
/// Golden-ratio hue step, so consecutive colours always differ widely and the
/// sequence doesn't repeat soon.
const HUE_STEP: u16 = 40_503;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let (index, t) = (runtime.clock_us / FALL_US, runtime.clock_us % FALL_US);
    let color = |index: u64| hsv_rainbow16((index as u16).wrapping_mul(HUE_STEP), 255, 255);
    let (below, falling) = (color(index), color(index + 1));
    // Accelerates from rest like a dropped object, reaching LED 0 at FALL_US.
    let t = u128::from(t);
    let fallen = (N as u128 * 256 * t * t / u128::from(FALL_US * FALL_US)) as i64;
    let front_q8 = (N * 256) as i64 - fallen;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        // About one LED of soft edge at the front.
        let weight = (index as i64 * 256 - front_q8 + 256).clamp(0, 255);
        *pixel = blend_rgb(below, falling, weight as u8);
    }
}

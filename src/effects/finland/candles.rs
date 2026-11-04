use super::{noise_time, value_noise};
use crate::effects::add_rgb;
use crate::{EffectRuntime, scale_rgb, scale8};
use smart_leds::RGB8;

const WINDOWS: usize = 3;
const CANDLES: usize = 2 * WINDOWS;
const FLAME: RGB8 = RGB8 {
    r: 255,
    g: 158,
    b: 56,
};
const ROOM_GLOW: RGB8 = RGB8 { r: 72, g: 30, b: 6 };
const DUSK: RGB8 = RGB8 { r: 0, g: 10, b: 52 };
/// Distances are in 1/256 LEDs.
const FLAME_REACH: i32 = 3 * 256;
const ROOM_REACH: i32 = 8 * 256;
const LIGHT_EVERY_MS: u64 = 1_200;
const LIGHT_MS: u64 = 1_000;

/// Two candles in every window on Independence Day evening: three windows along a
/// blue dusk, whose candles are lit one by one and then keep flickering.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_us = runtime.clock_us;
    let clock_ms = clock_us / 1000;
    let window_width = ((N << 8) / WINDOWS) as i32;

    let flames: [(i32, u8); CANDLES] = core::array::from_fn(|candle| {
        let seed = candle as u32 * 0x1000;
        let burn = (clock_ms.saturating_sub(candle as u64 * LIGHT_EVERY_MS) * 255 / LIGHT_MS)
            .min(255) as u8;
        let quick = value_noise(seed, noise_time(clock_us, 110));
        let slow = value_noise(seed + 1, noise_time(clock_us, 620));
        let flicker = 255 - quick / 9 - slow / 4;
        let sway = i32::from(value_noise(seed + 2, noise_time(clock_us, 520))) / 2 - 64;

        let window = (candle / 2) as i32;
        // 7.5 and 12.5 LEDs into a 20-LED window.
        let spot = if candle % 2 == 0 { 3 } else { 5 };
        let center = window * window_width + window_width * spot / 8 + sway;
        (center, scale8(burn, flicker))
    });

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let at = (index as i32 * 256) + 128;
        let mut color = DUSK;
        for pair in flames.as_chunks::<2>().0 {
            let center = (pair[0].0 + pair[1].0) / 2;
            let room = ((ROOM_REACH - (at - center).abs()).max(0) * 255 / ROOM_REACH) as u8;
            let lit = (u16::from(pair[0].1) + u16::from(pair[1].1)) / 2;
            color = add_rgb(color, scale_rgb(ROOM_GLOW, scale8(room, lit as u8)));
        }
        for &(center, level) in &flames {
            let near = ((FLAME_REACH - (at - center).abs()).max(0) * 255 / FLAME_REACH) as u8;
            color = add_rgb(color, scale_rgb(FLAME, scale8(scale8(near, near), level)));
        }
        *pixel = color;
    }
}

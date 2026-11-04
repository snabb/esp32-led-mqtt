use super::{band, noise_time, value_noise};
use crate::effects::add_rgb;
use crate::{EffectRuntime, blend_rgb, scale_rgb, scale8};
use smart_leds::RGB8;

const CANDLES: usize = 2;
const DUSK: RGB8 = RGB8 { r: 0, g: 10, b: 52 };
const WAX: RGB8 = RGB8 {
    r: 150,
    g: 116,
    b: 80,
};
const FLAME_CORE: RGB8 = RGB8 {
    r: 255,
    g: 206,
    b: 120,
};
const FLAME_TIP: RGB8 = RGB8 {
    r: 255,
    g: 76,
    b: 6,
};
const HALO: RGB8 = RGB8 { r: 64, g: 26, b: 4 };
const LIGHT_EVERY_MS: u64 = 1_500;
const LIGHT_MS: u64 = 1_000;

struct Candle {
    base: i32,
    wick: i32,
    flame_length: i32,
    level: u8,
}

/// Two candles, as in windows on Independence Day evening: stacked upright against
/// the blue dusk, lit one after the other, with flames tapering from core to tip.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_us = runtime.clock_us;
    let clock_ms = clock_us / 1000;
    // Lengths are in 1/256 LEDs.
    let section = ((N << 8) / CANDLES) as i32;

    let candles: [Candle; CANDLES] = core::array::from_fn(|candle| {
        let seed = candle as u32 * 0x1000;
        let lit_ms = clock_ms.saturating_sub(candle as u64 * LIGHT_EVERY_MS);
        let burn = (lit_ms * 255 / LIGHT_MS).min(255) as u8;
        let quick = value_noise(seed, noise_time(clock_us, 110));
        let slow = value_noise(seed + 1, noise_time(clock_us, 620));
        let start = candle as i32 * section;
        let longest = section * 3 / 10;
        let stretch = 170 + i32::from(slow) / 4 + i32::from(quick) / 8;
        Candle {
            base: start + section * 18 / 100,
            wick: start + section * 58 / 100,
            flame_length: (longest * stretch / 255 * i32::from(burn) / 255).max(1),
            level: scale8(burn, 255 - quick / 9 - slow / 4),
        }
    });

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let at = index as i32 * 256 + 128;
        let mut color = DUSK;
        for candle in &candles {
            let above_wick = at - candle.wick;

            let halo_reach = section * 30 / 100;
            let near = (halo_reach - above_wick.abs()).max(0) * 255 / halo_reach.max(1);
            let halo = scale8(scale8(near as u8, near as u8), candle.level);
            color = add_rgb(color, scale_rgb(HALO, halo));

            // Wax glows through near the flame and darkens towards the base.
            let height = candle.wick - candle.base;
            let up_wax = ((at - candle.base).clamp(0, height) * 255 / height.max(1)) as u8;
            let lit = scale8(scale8(up_wax, up_wax), candle.level);
            let wax = scale_rgb(WAX, 60 + scale8(lit, 195));
            color = blend_rgb(color, wax, band(at, candle.base, candle.wick, 256));

            if (0..candle.flame_length).contains(&above_wick) {
                let along = (above_wick * 255 / candle.flame_length) as u8;
                let body = 255 - scale8(along, along);
                let shade = blend_rgb(FLAME_CORE, FLAME_TIP, along);
                // Replaces rather than adds: dusk blue would wash the flame out to white.
                color = blend_rgb(color, shade, scale8(body, candle.level));
            }
        }
        *pixel = color;
    }
}

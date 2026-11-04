use super::{BLUE, WHITE, noise_time, value_noise};
use crate::{EffectRuntime, blend_rgb, scale8};
use smart_leds::RGB8;

/// One LED of landscape scrolled by per this time.
const SCROLL_MS_PER_LED: u64 = 320;
const FOREST: RGB8 = RGB8 { r: 2, g: 62, b: 16 };
const PINE: RGB8 = RGB8 { r: 0, g: 26, b: 10 };
const SHORE: RGB8 = RGB8 {
    r: 60,
    g: 92,
    b: 44,
};
/// Land below this height is lake; `SHORE_WIDTH` above it is the shoreline.
const WATER_LINE: u8 = 104;
const SHORE_WIDTH: u16 = 18;

/// Flying over the land of a thousand lakes: blue lakes with sun glints scroll by
/// between dark green forests.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_us = runtime.clock_us;
    let scroll = clock_us * 256 / (SCROLL_MS_PER_LED * 1000);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let ground = scroll + (index as u64) * 256;
        let at = |leds_per_cell: u64| (ground / leds_per_cell) as u32;
        let height =
            ((u16::from(value_noise(1, at(8))) * 2 + u16::from(value_noise(2, at(3)))) / 3) as u8;

        let forest = blend_rgb(PINE, FOREST, value_noise(3, at(2)));
        *pixel = if height < WATER_LINE {
            // Per LED, not per ground cell: glints moving with the ground would hop.
            let ripple = value_noise(index as u32 * 0x77, noise_time(clock_us, 260));
            let glint = ripple.saturating_sub(205).saturating_mul(5);
            // Open water only, so a glint can't be cut off at the shoreline.
            let open = (WATER_LINE - height).saturating_mul(12);
            blend_rgb(BLUE, WHITE, scale8(glint, open))
        } else {
            let inland = (u16::from(height - WATER_LINE) * 255 / SHORE_WIDTH).min(255) as u8;
            if inland < 128 {
                blend_rgb(BLUE, SHORE, inland * 2)
            } else {
                blend_rgb(SHORE, forest, (inland - 128) * 2)
            }
        };
    }
}

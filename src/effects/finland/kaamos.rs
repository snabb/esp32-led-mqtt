use super::{band, hash, noise_time, value_noise};
use crate::effects::{add_rgb, strip_position16};
use crate::{EffectRuntime, blend_rgb, ease_wave16, scale8};
use smart_leds::RGB8;

const DAY_MS: u32 = 80_000;
/// Starts in the morning twilight rather than in the dark.
const DAY_START: u16 = 0x3000;
/// Height of the glow above the horizon at night and at noon, as `u16` strip position.
const GLOW_LOW: u32 = 14_000;
const GLOW_HIGH: u32 = 56_000;
/// Top of the dark treeline: two to three LEDs of 60.
const TREELINE: i32 = 65536 / 24;
/// Stars only show above this height, clear of the glow.
const STARS_FROM: u16 = 16_384;
const FOREST: RGB8 = RGB8 { r: 0, g: 3, b: 8 };
const NIGHT: RGB8 = RGB8 { r: 2, g: 6, b: 36 };
const BLUE_HOUR: RGB8 = RGB8 {
    r: 14,
    g: 40,
    b: 140,
};
const ROSE: RGB8 = RGB8 {
    r: 190,
    g: 56,
    b: 110,
};
const PEACH: RGB8 = RGB8 {
    r: 255,
    g: 140,
    b: 62,
};

/// A polar night day: the sun stays below the horizon, so a peach and rose glow
/// only climbs into the blue hour above the treeline and sinks again before the
/// stars come back.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let day = runtime.phase(DAY_MS).wrapping_add(DAY_START);
    let light = (ease_wave16(day) >> 8) as u8;
    let reach = GLOW_LOW + (GLOW_HIGH - GLOW_LOW) * u32::from(light) / 255;
    let clock_us = runtime.clock_us;
    let soft = 65536 / N as i32;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let y = strip_position16::<N>(index);
        let near = (reach.saturating_sub(u32::from(y)) * 255 / reach) as u8;
        let rose = scale8(scale8(near, light), 235);
        let peach = scale8(
            near.saturating_sub(120).saturating_mul(2),
            scale8(light, light),
        );

        let sky = blend_rgb(blend_rgb(NIGHT, BLUE_HOUR, light), ROSE, rose);
        let glow = blend_rgb(sky, PEACH, peach);

        let star = if y > STARS_FROM && hash(index as u32 ^ 0xcafe).is_multiple_of(7) {
            let twinkle = value_noise(index as u32, noise_time(clock_us, 700));
            scale8(twinkle, 150_u8.saturating_sub(light.saturating_mul(3)))
        } else {
            0
        };
        let lit = add_rgb(
            glow,
            RGB8 {
                r: star,
                g: star,
                b: star,
            },
        );
        let trees = band(i32::from(y) + soft / 2, -soft, TREELINE, soft);
        *pixel = blend_rgb(lit, FOREST, trees);
    }
}

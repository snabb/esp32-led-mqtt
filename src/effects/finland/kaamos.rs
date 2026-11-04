use super::{hash, noise_time, value_noise};
use crate::effects::{add_rgb, strip_position16};
use crate::{EffectRuntime, blend_rgb, ease_wave16, scale8};
use smart_leds::RGB8;

const DAY_MS: u32 = 80_000;
/// Starts in the morning twilight rather than in the dark.
const DAY_START: u16 = 0x3000;
/// How far the glow reaches from the hidden sun, as `u16` strip position.
const GLOW_REACH: u32 = 42_000;
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

/// A polar night day: the sun passes unseen below the horizon, giving only a rose
/// and peach glow in the blue hour before the stars come back.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let day = runtime.phase(DAY_MS).wrapping_add(DAY_START);
    let light = (ease_wave16(day) >> 8) as u8;
    // Crosses the strip around noon, from well off one end to well off the other.
    let sun = i64::from(day) * 2 - 32768;
    let clock_us = runtime.clock_us;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position16::<N>(index);
        let distance = (i64::from(pos) - sun).unsigned_abs() as u32;
        let near = (GLOW_REACH.saturating_sub(distance) * 255 / GLOW_REACH) as u8;
        let rose = scale8(scale8(near, light), 235);
        let peach = scale8(
            near.saturating_sub(120).saturating_mul(2),
            scale8(light, light),
        );

        let sky = blend_rgb(blend_rgb(NIGHT, BLUE_HOUR, light), ROSE, rose);
        let glow = blend_rgb(sky, PEACH, peach);

        let star = if hash(index as u32 ^ 0xcafe).is_multiple_of(7) {
            let twinkle = value_noise(index as u32, noise_time(clock_us, 700));
            scale8(twinkle, 150_u8.saturating_sub(light.saturating_mul(3)))
        } else {
            0
        };
        *pixel = add_rgb(
            glow,
            RGB8 {
                r: star,
                g: star,
                b: star,
            },
        );
    }
}

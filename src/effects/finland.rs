// Directional effects here draw a vertical strip with the first LED at the bottom.
pub(super) mod frost;
pub(super) mod kaamos;
pub(super) mod revontulet;
pub(super) mod snowflakes;

use super::{MAX_TICKS_PER_FRAME, strip_position16};
use crate::{EffectRuntime, blend_rgb, ease_wave16, half_sin8, half_sin16, scale_rgb};
use smart_leds::RGB8;

const BLUE: RGB8 = RGB8 {
    r: 0,
    g: 48,
    b: 160,
};
const WHITE: RGB8 = RGB8 {
    r: 255,
    g: 255,
    b: 255,
};

/// Murmur3 finalizer: stateless randomness keeps effects pure functions of the clock.
fn hash(value: u32) -> u32 {
    let mut h = value;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// Smooth 1D noise in `0..=255`; one random value per 256 units of `position`.
fn value_noise(seed: u32, position: u32) -> u8 {
    let sample = |cell: u32| hash(seed.wrapping_add(cell.wrapping_mul(0x9e37_79b1))) as u8;
    let cell = position >> 8;
    let weight = (ease_wave16(((position & 0xff) as u16) << 7) >> 8) as u8;
    let (a, b) = (sample(cell), sample(cell.wrapping_add(1)));
    (i32::from(a) + (i32::from(b) - i32::from(a)) * i32::from(weight) / 255) as u8
}

/// `value_noise` position advancing one cell per `cell_ms` of effect time.
fn noise_time(clock_us: u64, cell_ms: u32) -> u32 {
    (clock_us * 256 / (u64::from(cell_ms) * 1000)) as u32
}

/// Coverage in `0..=255` of `x` by the band `start..end`, with edges ramped over
/// `soft` so bands move between LEDs without hopping.
fn band(x: i32, start: i32, end: i32, soft: i32) -> u8 {
    let soft = soft.max(1);
    let rise = (x - start) * 255 / soft + 128;
    let fall = (end - x) * 255 / soft + 128;
    rise.min(fall).clamp(0, 255) as u8
}

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, kind: u8) {
    if kind == 2 || kind == 3 {
        if runtime.ticks_done == 0 {
            runtime.tick_frame.fill(scale_rgb(BLUE, 32));
            runtime.prev_tick_frame.fill(scale_rgb(BLUE, 32));
        }
        runtime.simulate(20, MAX_TICKS_PER_FRAME, |r, _| {
            for i in 0..N {
                r.effect_data[i] = r.effect_data[i].saturating_sub(3);
                if r.effect_data[i] == 0 && r.chance(128, if kind == 2 { 2 } else { 5 }) {
                    r.effect_data[i] = 255;
                    r.twinkle_color[i] = if kind == 3 && r.rng.next_u8() < 128 {
                        BLUE
                    } else {
                        WHITE
                    };
                }
                r.tick_frame[i] = blend_rgb(
                    scale_rgb(BLUE, 32),
                    r.twinkle_color[i],
                    half_sin8(r.effect_data[i]),
                );
            }
        });
        return;
    }
    let a = runtime.phase(8000);
    for (i, pixel) in runtime.frame.iter_mut().enumerate() {
        let pos = strip_position16::<N>(i);
        let wave = half_sin16(pos.wrapping_mul(2).wrapping_sub(a));
        *pixel = match kind {
            5 => {
                let flake = half_sin16(pos.wrapping_mul(7).wrapping_add(a));
                blend_rgb(scale_rgb(BLUE, 40), WHITE, flake.saturating_sub(170) * 3)
            }
            _ => scale_rgb(
                blend_rgb(BLUE, WHITE, half_sin16(pos.wrapping_mul(3).wrapping_sub(a))),
                150 + wave / 3,
            ),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectId, EffectParams};

    const FRAME_US: u32 = 16_667;
    const BLUE_WHITE: [EffectId; 6] = [
        EffectId::FinlandSnow,
        EffectId::FinlandBlueWhiteTwinkle,
        EffectId::FinlandSnowfall,
        EffectId::FinlandRibbon,
        EffectId::FinlandFrost,
        EffectId::FinlandSnowflakes,
    ];

    fn runtime(id: EffectId) -> EffectRuntime<60> {
        EffectRuntime::new(EffectParams {
            id,
            ..EffectParams::default()
        })
    }

    #[test]
    fn finland_palette_and_small_strips() {
        fn check<const N: usize>() {
            for id in BLUE_WHITE {
                let mut runtime = EffectRuntime::<N>::new(EffectParams {
                    id,
                    ..EffectParams::default()
                });
                for frame in 0..1200 {
                    for p in runtime.render(frame * FRAME_US) {
                        assert!(p.r <= p.g && p.g <= p.b);
                        if id == EffectId::FinlandSnow || id == EffectId::FinlandBlueWhiteTwinkle {
                            assert!(p.b > 0);
                        }
                    }
                }
            }
        }
        check::<1>();
        check::<2>();
        check::<60>();
        check::<512>();
    }

    #[test]
    fn snowflakes_do_not_pile_up_at_the_bottom() {
        let mut runtime = runtime(EffectId::FinlandSnowflakes);
        let (mut bottom, mut middle) = (0_u32, 0_u32);
        // Two minutes: longer than any flake's fall.
        for index in 0..7200 {
            let frame = runtime.render(index * FRAME_US);
            bottom += u32::from(frame[0].r);
            middle += u32::from(frame[30].r);
        }
        assert!(bottom < middle);
    }
}

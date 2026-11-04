// Directional effects here draw a vertical strip with the first LED at the bottom.
pub(super) mod candles;
pub(super) mod flag;
pub(super) mod frost;
pub(super) mod kaamos;
pub(super) mod lakeland;
pub(super) mod mosaic;
pub(super) mod revontulet;
pub(super) mod sauna;
pub(super) mod snowdrift;

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

/// Sine-like wave in `-32768..=32767`, lowest at phase 0.
fn signed_wave(phase: u16) -> i32 {
    i32::from(ease_wave16(phase)) - 32768
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
    const BLUE_WHITE: [EffectId; 10] = [
        EffectId::FinlandSnow,
        EffectId::FinlandBlueWhiteTwinkle,
        EffectId::FinlandSnowfall,
        EffectId::FinlandRibbon,
        EffectId::FinlandFlagWave,
        EffectId::FinlandCrossWave,
        EffectId::FinlandFlagRaising,
        EffectId::FinlandFrost,
        EffectId::FinlandMosaic,
        EffectId::FinlandSnowdrift,
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
    fn flag_wave_keeps_the_arm_in_the_middle_of_the_strip() {
        let mut runtime = runtime(EffectId::FinlandFlagWave);
        let mut whiteness = [0_u32; 60];
        for frame in 0..1200 {
            for (sum, p) in whiteness.iter_mut().zip(runtime.render(frame * FRAME_US)) {
                *sum += u32::from(p.r);
            }
        }
        // 4:3:4 on 60 LEDs puts the arm on LEDs 22..38.
        assert!(whiteness[30] < whiteness[8] / 4);
        assert!(whiteness[30] < whiteness[52] / 4);
    }

    #[test]
    fn snowdrift_piles_up_at_the_bottom() {
        let mut runtime = runtime(EffectId::FinlandSnowdrift);
        let mut frame = [RGB8::default(); 60];
        // 70 s in, the drift has grown but not yet melted.
        for index in 0..4200 {
            frame = *runtime.render(index * FRAME_US);
        }
        assert!(frame[0].r > 150);
        assert!(frame[59].r < 100);
    }

    #[test]
    fn candles_burn_in_two_warm_flames() {
        let mut runtime = runtime(EffectId::FinlandCandles);
        let mut frame = [RGB8::default(); 60];
        for index in 0..1200 {
            frame = *runtime.render(index * FRAME_US);
        }
        for candle in [&frame[..30], &frame[30..]] {
            let flame = candle
                .iter()
                .max_by_key(|p| u16::from(p.r) + u16::from(p.g));
            let flame = flame.unwrap();
            assert!(flame.r > 150 && u16::from(flame.r) > u16::from(flame.b) * 2);
        }
        // Dusk shows below the lower candle.
        assert!(frame[0].b > frame[0].r);
    }
}

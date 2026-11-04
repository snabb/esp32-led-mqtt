use super::{MAX_TICKS_PER_FRAME, strip_position16};
use crate::{EffectRuntime, blend_rgb, half_sin8, half_sin16, scale_rgb};
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
    use crate::{EFFECTS, EffectId, EffectParams};

    #[test]
    fn finland_palette_and_small_strips() {
        fn check<const N: usize>() {
            for (id, _) in EFFECTS {
                if (id as usize) < EffectId::FinlandSnow as usize {
                    continue;
                }
                let mut runtime = EffectRuntime::<N>::new(EffectParams {
                    id,
                    ..EffectParams::default()
                });
                for frame in 0..1200 {
                    for p in runtime.render(frame * 16667) {
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
}

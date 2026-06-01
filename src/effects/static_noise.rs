use crate::{EffectRuntime, speed_interval};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if !runtime.elapsed(now_ms, speed_interval(24, runtime.params.speed)) {
        return;
    }

    let color_chance = 8 + runtime.params.intensity / 12;

    for pixel in runtime.frame.as_mut_slice() {
        let level = if runtime.rng.next_u8() & 1 == 0 {
            runtime.rng.next_u8() >> 2
        } else {
            192_u8.saturating_add(runtime.rng.next_u8() >> 2)
        };

        *pixel = if runtime.rng.next_u8() < color_chance {
            match runtime.rng.next_u8() & 0x03 {
                0 => RGB8 {
                    r: 255,
                    g: level,
                    b: level,
                },
                1 => RGB8 {
                    r: level,
                    g: 255,
                    b: level,
                },
                2 => RGB8 {
                    r: level,
                    g: level,
                    b: 255,
                },
                _ => RGB8 {
                    r: 255,
                    g: 255,
                    b: level,
                },
            }
        } else {
            RGB8 {
                r: level,
                g: level,
                b: level,
            }
        };
    }
}

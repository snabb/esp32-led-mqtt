use crate::EffectRuntime;
use smart_leds::RGB8;

const TICK_MS: u32 = 40;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    // Each tick replaces the whole frame, so skipped ticks are never visible.
    runtime.simulate(TICK_MS, 1, |runtime, _| tick(runtime));
}

fn tick<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let color_chance = 8 + runtime.params.intensity / 12;

    for index in 0..N {
        let level = if runtime.rng.next_u8() & 1 == 0 {
            runtime.rng.next_u8() >> 2
        } else {
            192_u8.saturating_add(runtime.rng.next_u8() >> 2)
        };

        runtime.tick_frame[index] = if runtime.rng.next_u8() < color_chance {
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

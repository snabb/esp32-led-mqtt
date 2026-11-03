use super::MAX_TICKS_PER_FRAME;
use crate::EffectRuntime;
use smart_leds::RGB8;

/// The original Fire2012 runs at 60 FPS.
const TICK_MS: u32 = 17;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, _| tick(runtime));
}

fn tick<const N: usize>(runtime: &mut EffectRuntime<N>) {
    for index in 0..N {
        let cooling = runtime.rng.next_u8() % (u8::try_from((55 * 10) / N + 2).unwrap_or(u8::MAX));
        runtime.effect_data[index] = runtime.effect_data[index].saturating_sub(cooling);
    }

    for index in (2..N).rev() {
        let heat = (u16::from(runtime.effect_data[index - 1])
            + u16::from(runtime.effect_data[index - 2])
            + u16::from(runtime.effect_data[index - 2]))
            / 3;
        runtime.effect_data[index] = heat as u8;
    }

    if runtime.chance(runtime.params.intensity, 45) {
        let spark_index = (runtime.rng.next_u8() as usize) % fire_spark_height(N);
        let spark = 160_u8.saturating_add(runtime.rng.next_u8() % 96);
        runtime.effect_data[spark_index] = runtime.effect_data[spark_index].saturating_add(spark);
    }

    for index in 0..N {
        runtime.tick_frame[index] = heat_color(runtime.effect_data[index]);
    }
}

fn heat_color(heat: u8) -> RGB8 {
    let ramp = (heat & 0x3f) << 2;
    match heat >> 6 {
        0 => RGB8 {
            r: ramp,
            g: 0,
            b: 0,
        },
        1 => RGB8 {
            r: 255,
            g: ramp,
            b: 0,
        },
        2 => RGB8 {
            r: 255,
            g: 255,
            b: ramp,
        },
        _ => RGB8 {
            r: 255,
            g: 255,
            b: 255,
        },
    }
}

fn fire_spark_height(count: usize) -> usize {
    (count / 6).clamp(1, 10)
}

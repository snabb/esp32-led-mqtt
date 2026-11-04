use super::{band, hash, value_noise};
use crate::effects::{MAX_TICKS_PER_FRAME, add_rgb, add_smooth_dot, strip_position16};
use crate::{EffectRuntime, blend_rgb, scale_rgb, scale8};
use smart_leds::RGB8;

const TICK_MS: u32 = 20;
const CYCLE_US: u64 = 18_000_000;
const RUN_US: u64 = 2_800_000;
const NIGHT: RGB8 = RGB8 { r: 0, g: 3, b: 16 };
const SNOW: RGB8 = RGB8 {
    r: 52,
    g: 66,
    b: 104,
};
const GREEN: RGB8 = RGB8 {
    r: 20,
    g: 255,
    b: 110,
};
const VIOLET: RGB8 = RGB8 {
    r: 110,
    g: 24,
    b: 170,
};
const FOX: RGB8 = RGB8 {
    r: 255,
    g: 96,
    b: 8,
};
const FOX_HEAD: RGB8 = RGB8 {
    r: 255,
    g: 190,
    b: 110,
};
const TAIL_SEGMENTS: i32 = 5;
/// Top of the snow, as `u16` strip position: four LEDs of 60.
const GROUND: i32 = 65536 / 15;
/// Where the curtain reaches full strength above the ground.
const CURTAIN_BASE: u32 = 65536 / 5;

/// "Revontulet" means fox fires: an arctic fox leaps up from the snow and its tail
/// sweeps sparks into the sky, leaving aurora that is green low down and violet
/// high up, as real curtains are, and fades to violet as it dies.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let soft = 65536 / N as i32;
    let tail_step = 65535 * 5 / 4 / (N.max(2) as i32 - 1);

    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |r, tick_us| {
        let in_cycle = tick_us % CYCLE_US;
        // Runs on past the top until the tail has left the strip too.
        let travel = 65535 + i64::from(tail_step * TAIL_SEGMENTS);
        let fox_at = (in_cycle < RUN_US).then(|| (in_cycle as i64 * travel / RUN_US as i64) as i32);

        for energy in &mut r.effect_data {
            if *energy > 0 && r.rng.next_u8() < 110 {
                *energy -= 1;
            }
        }
        if let Some(at) = fox_at.filter(|at| *at <= 65535) {
            let head = at as usize * (N - 1) / 65535;
            let reach = N / 60;
            for i in head.saturating_sub(reach)..=(head + reach).min(N - 1) {
                r.effect_data[i] = r.effect_data[i].max(190 + (r.rng.next_u8() >> 2) + 1);
            }
        }

        let drift = (tick_us * 256 / 1_300_000) as u32;
        let star_time = (tick_us * 256 / 400_000) as u32;
        for i in 0..N {
            let energy = r.effect_data[i];
            let y = u32::from(strip_position16::<N>(i));
            let across = y * 15 / 256;
            let ray = value_noise(0x51, across.wrapping_sub(drift));

            // Squared for the hue and inverse-squared for the level, so the green
            // gives way to violet while the curtain is still bright enough to see it.
            let altitude = (y >> 8) as u8;
            let green_limit = 90 + scale8(255 - altitude, 165);
            let hue = scale8(scale8(energy, energy).saturating_add(ray / 4), green_limit);
            let spent = 255 - energy;
            let level = if energy == 0 {
                0
            } else {
                255 - scale8(spent, spent)
            };
            let above_ground = (y.min(CURTAIN_BASE) * 255 / CURTAIN_BASE) as u8;
            let curtain = scale_rgb(
                blend_rgb(VIOLET, GREEN, hue),
                scale8(scale8(level, above_ground), 110 + scale8(ray, 145)),
            );

            let sparks = scale8(energy.saturating_sub(238).saturating_mul(12), above_ground);
            let star = if y > CURTAIN_BASE && hash(i as u32 ^ 0x5eed).is_multiple_of(12) {
                value_noise(i as u32, star_time) / 4
            } else {
                0
            };
            let white = sparks.saturating_add(star);
            let ground = blend_rgb(NIGHT, SNOW, band(y as i32 + soft / 2, -soft, GROUND, soft));
            r.tick_frame[i] = add_rgb(
                add_rgb(ground, curtain),
                RGB8 {
                    r: white,
                    g: white,
                    b: white,
                },
            );
        }

        if let Some(at) = fox_at {
            for segment in (1..=TAIL_SEGMENTS).rev() {
                let tail_at = at - tail_step * segment;
                if (0..=65535).contains(&tail_at) {
                    let fade = 255 - (segment * 255 / (TAIL_SEGMENTS + 1)) as u8;
                    add_smooth_dot(&mut r.tick_frame, tail_at as u16, scale_rgb(FOX, fade));
                }
            }
            if at <= 65535 {
                add_smooth_dot(&mut r.tick_frame, at as u16, FOX_HEAD);
            }
        }
    });
}

use super::value_noise;
use crate::effects::{MAX_TICKS_PER_FRAME, strip_position16};
use crate::{EffectRuntime, blend_rgb, scale_rgb, scale8};
use smart_leds::RGB8;

const TICK_MS: u32 = 20;
/// Time between ladlefuls of water, and how long one takes to pour.
const THROW_EVERY_US: u64 = 15_000_000;
const POUR_US: u64 = 800_000;
/// Time for the stones to recover after the water hits them.
const REHEAT_US: u64 = 7_000_000;
const WOOD: RGB8 = RGB8 { r: 62, g: 24, b: 5 };
const EMBER_DARK: RGB8 = RGB8 {
    r: 110,
    g: 10,
    b: 0,
};
const EMBER_HOT: RGB8 = RGB8 {
    r: 255,
    g: 96,
    b: 10,
};
const STEAM: RGB8 = RGB8 {
    r: 225,
    g: 228,
    b: 235,
};

/// Glowing stones on the sauna stove in a dark log room. Every so often comes
/// löyly: water hits the stones, they dim, and steam billows out to both walls.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let center = N / 2;
    let reach = (N / 6).max(1);
    let stones = center.saturating_sub(reach)..(center + reach).min(N);

    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |r, tick_us| {
        let since_throw = tick_us % THROW_EVERY_US;
        if since_throw < POUR_US {
            for i in stones.clone() {
                let puff = 12 + (r.rng.next_u8() >> 3);
                r.effect_data[i] = r.effect_data[i].saturating_add(puff);
            }
        }

        // Steam drifts outwards from the middle, moved far end first so that one
        // tick carries it a single LED. Moving only a little of each LED per tick
        // makes the cloud widen as it goes instead of travelling as a thin front.
        for i in (center + 1..N).rev() {
            let flux = r.effect_data[i - 1] / 14;
            r.effect_data[i - 1] -= flux;
            r.effect_data[i] = r.effect_data[i].saturating_add(flux);
        }
        for i in 0..center.saturating_sub(1) {
            let flux = r.effect_data[i + 1] / 14;
            r.effect_data[i + 1] -= flux;
            r.effect_data[i] = r.effect_data[i].saturating_add(flux);
        }
        for i in 0..N {
            // Condenses on the cold walls rather than piling up against them.
            let wall = usize::from(i == 0 || i == N - 1) * 12;
            let condensed = u8::from(r.rng.next_u8() < 64) + (r.effect_data[i] / 128) + wall as u8;
            r.effect_data[i] = r.effect_data[i].saturating_sub(condensed);
        }

        let chill = 255 - (since_throw.min(REHEAT_US) * 255 / REHEAT_US) as u8;
        let heat = 255 - scale8(chill, 190);
        let mottle_time = (tick_us * 256 / 1_300_000) as u32;
        let shimmer_time = (tick_us * 256 / 620_000) as u32;
        let billow_time = (tick_us * 256 / 900_000) as u32;

        for i in 0..N {
            let pos = u32::from(strip_position16::<N>(i));
            let from_center = pos.abs_diff(32768);
            let reflected = 255 - (from_center * 200 / 32768) as u8;
            let room = scale_rgb(WOOD, scale8(reflected, scale8(heat, 200) + 55));

            let base = if stones.contains(&i) {
                let mottle = value_noise(0x20, (pos * 20 / 256).wrapping_add(mottle_time));
                let shimmer = value_noise(i as u32 * 0x101, shimmer_time);
                let glow = ((u16::from(mottle) * 3 + u16::from(shimmer)) / 4) as u8;
                let ember = scale_rgb(blend_rgb(EMBER_DARK, EMBER_HOT, glow), heat);
                // Tapered so the stove doesn't end in a hard block edge.
                let edge = (i - stones.start).min(stones.end - 1 - i) as u8;
                blend_rgb(room, ember, 120_u8.saturating_add(edge.saturating_mul(70)))
            } else {
                room
            };

            let billow = value_noise(0x10, (pos * 12 / 256).wrapping_add(billow_time));
            let density = scale8(r.effect_data[i], 150 + scale8(billow, 105));
            r.tick_frame[i] = blend_rgb(base, STEAM, density);
        }
    });
}

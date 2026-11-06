use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, blend_rgb};
use smart_leds::RGB8;

const TICK_MS: u64 = 17;
/// Each slot of about 1 s holds at most one drop, taken by chance at the intensity.
const SLOT_TICKS: u64 = 64;
/// A drop grows over this many ticks so that it doesn't pop in at full height.
const DROP_TICKS: u64 = 8;
/// A raised cosine in Q16: a sharper drop would ring on the coarse grid.
const DROP_SHAPE: [i64; 9] = [
    6_226, 22_610, 42_926, 59_310, 65_536, 59_310, 42_926, 22_610, 6_226,
];
/// Ripples of height 0.5 in Q16 reach full colour.
const FULL_COLOR_SHIFT: u32 = 7;

const REST: RGB8 = RGB8 { r: 0, g: 8, b: 36 };
const CREST: RGB8 = RGB8 {
    r: 150,
    g: 255,
    b: 235,
};
const TROUGH: RGB8 = RGB8 {
    r: 170,
    g: 0,
    b: 255,
};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS as u32, MAX_TICKS_PER_FRAME, tick);
}

/// Damped 1D wave equation with reflecting ends; `field` holds height and velocity.
fn tick<const N: usize>(runtime: &mut EffectRuntime<N>, time_us: u64) {
    add_drop(runtime, time_us / (TICK_MS * 1000));

    for index in 0..N {
        let height = runtime.field[index][0];
        let left = runtime.field[index.saturating_sub(1)][0];
        let right = runtime.field[(index + 1).min(N - 1)][0];
        let velocity = runtime.field[index][1];
        // Wave speed sqrt(1/8) LED per tick, about 20 LEDs/s at 1x.
        runtime.field[index][1] = velocity + ((left + right - 2 * height) >> 3) - (velocity >> 7);
    }
    // Reflecting ends conserve the mean height, so drops would raise the whole strip.
    // Removing it gradually avoids a visible step after each drop.
    let mean = runtime
        .field
        .iter()
        .map(|cell| i64::from(cell[0]))
        .sum::<i64>()
        / N as i64;
    let drain = (mean >> 5) as i32;
    for index in 0..N {
        let [height, velocity] = runtime.field[index];
        let height = height + velocity - drain;
        runtime.field[index][0] = height;
        let level = (height.unsigned_abs() >> FULL_COLOR_SHIFT).min(255) as u8;
        let peak = if height >= 0 { CREST } else { TROUGH };
        runtime.tick_frame[index] = blend_rgb(REST, peak, level);
    }
}

fn add_drop<const N: usize>(runtime: &mut EffectRuntime<N>, tick_index: u64) {
    let hash = mix((tick_index / SLOT_TICKS) as u32);
    if (hash & 0xff) >= u32::from(runtime.params.intensity) {
        return;
    }
    let start = u64::from(hash >> 8) % (SLOT_TICKS - DROP_TICKS);
    if !(start..start + DROP_TICKS).contains(&(tick_index % SLOT_TICKS)) {
        return;
    }
    let centre = (hash >> 16) as usize % N;
    let height = 45_000 + i64::from((hash >> 12) & 0x7fff);
    // Mixing raised and sunken drops shows both colours.
    let height = if hash >> 31 == 0 { height } else { -height } / DROP_TICKS as i64;
    for (offset, shape) in DROP_SHAPE.iter().enumerate() {
        if let Some(cell) = (centre + offset)
            .checked_sub(DROP_SHAPE.len() / 2)
            .and_then(|index| runtime.field.get_mut(index))
        {
            cell[0] += ((shape * height) >> 16) as i32;
        }
    }
}

fn mix(value: u32) -> u32 {
    let hash = value.wrapping_add(1).wrapping_mul(2_654_435_761);
    (hash ^ (hash >> 15)).wrapping_mul(2_246_822_519)
}

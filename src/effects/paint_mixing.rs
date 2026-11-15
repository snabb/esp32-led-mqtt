//! Drops of paint land on the strip and bleed into their neighbours. Colours
//! mix as vectors on the colour wheel, so opposite hues mix to a pale tint
//! instead of a dark one. The variation sets how often drops fall.

use super::{MAX_TICKS_PER_FRAME, hash, sin_q16};
use crate::{EffectRuntime, hsv_rainbow16, phase_at};

const TICK_MS: u32 = 40;
const ONE: i32 = 1 << 16;
/// Diffusion per tick in Q16: colours spread about 8 LEDs in 10 s.
const DIFFUSION: i64 = 7_864;
/// A drop may fall at the start of each slot, poured over a few ticks so it
/// never appears at once.
const SLOT_TICKS: u64 = 15;
const POUR_TICKS: u64 = 8;
const PALETTE_MS: u32 = 90_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, tick);
}

/// `field[i]` is the colour as `[x, y]` in Q16: hue is the angle, saturation
/// the length.
fn tick<const N: usize>(runtime: &mut EffectRuntime<N>, time_us: u64) {
    let palette = phase_at(time_us, PALETTE_MS);
    if runtime.field.iter().all(|cell| *cell == [0, 0]) {
        for (index, cell) in runtime.field.iter_mut().enumerate() {
            *cell = chroma(palette.wrapping_add((index * 0x6000 / N) as u16));
        }
    }

    let tick = time_us / (u64::from(TICK_MS) * 1000);
    let (slot, age) = (tick / SLOT_TICKS, tick % SLOT_TICKS);
    let random = hash(slot as u32);
    let threshold = 16 + u32::from(runtime.params.variation) * 7 / 8;
    if age < POUR_TICKS && random % 256 < threshold {
        // Mostly neighbouring hues, which mix vividly; sometimes the opposite
        // one, which mixes pale.
        let mut hue = palette
            .wrapping_add(((random >> 16) % 0x6000) as u16)
            .wrapping_sub(0x3000);
        if random >> 30 == 0 {
            hue = hue.wrapping_add(0x8000);
        }
        let centre = ((random >> 8) % N as u32) as usize;
        let radius = 2 + ((random >> 12) % 4) as usize;
        pour(&mut runtime.field, centre, radius, chroma(hue));
    }

    diffuse(&mut runtime.field);

    for (pixel, &[x, y]) in runtime.tick_frame.iter_mut().zip(&runtime.field) {
        let length = isqrt((i64::from(x).pow(2) + i64::from(y).pow(2)) as u64).min(ONE as u64);
        let s = ((length * 255) >> 16) as u32;
        // Stays vivid until mixed well past half way; mixed white is dimmed
        // so it doesn't glare beside the colours.
        let sat = 255 - (255 - s) * (255 - s) / 255;
        let value = 160 + 95 * sat / 255;
        *pixel = hsv_rainbow16(atan2_turn(y, x), sat as u8, value as u8);
    }
}

fn pour<const N: usize>(field: &mut [[i32; 2]; N], centre: usize, radius: usize, target: [i32; 2]) {
    let reach = radius + 1;
    let from = centre.saturating_sub(radius);
    for (index, cell) in field.iter_mut().enumerate().take(centre + reach).skip(from) {
        let falloff = (reach - centre.abs_diff(index)) * 256 / reach;
        // At most a quarter of the way per tick.
        let weight = (falloff / 4) as i64;
        for (value, target) in cell.iter_mut().zip(target) {
            *value += ((i64::from(target - *value) * weight) >> 8) as i32;
        }
    }
}

/// One explicit diffusion step with reflecting ends. The left neighbour is kept
/// from before its update, so all cells step from the same instant.
fn diffuse<const N: usize>(field: &mut [[i32; 2]; N]) {
    let mut left = field[0];
    for index in 0..N {
        let current = field[index];
        let right = field[(index + 1).min(N - 1)];
        for channel in 0..2 {
            let laplacian = i64::from(left[channel] + right[channel] - 2 * current[channel]);
            field[index][channel] = current[channel] + ((laplacian * DIFFUSION) >> 16) as i32;
        }
        left = current;
    }
}

/// Unit colour vector of `hue` in Q16.
fn chroma(hue: u16) -> [i32; 2] {
    [sin_q16(hue.wrapping_add(0x4000)), sin_q16(hue)]
}

/// Angle of `(x, y)` in u16 turns, within about 0.07 % of a turn.
fn atan2_turn(y: i32, x: i32) -> u16 {
    let (ax, ay) = (u64::from(x.unsigned_abs()), u64::from(y.unsigned_abs()));
    if ax == 0 && ay == 0 {
        return 0;
    }
    let ratio = (ax.min(ay) << 16) / ax.max(ay);
    // atan(r) ≈ π/4·r + 0.273·r·(1 − r), in turns.
    let curve = (ratio * (65_536 - ratio)) >> 16;
    let octant = ((8_192 * ratio + 2_847 * curve) >> 16) as u16;
    let angle = if ay > ax { 16_384 - octant } else { octant };
    match (x < 0, y < 0) {
        (false, false) => angle,
        (true, false) => 32_768 - angle,
        (true, true) => 32_768 + angle,
        (false, true) => 0_u16.wrapping_sub(angle),
    }
}

fn isqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    // A power of two at or above the root, so Newton's steps only decrease.
    let mut root = 1_u64 << (64 - value.leading_zeros()).div_ceil(2);
    loop {
        let next = (root + value / root) / 2;
        if next >= root {
            return root;
        }
        root = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{atan2_turn, chroma, isqrt};

    #[test]
    fn angle_round_trips() {
        for hue in (0..=u16::MAX).step_by(61) {
            let [x, y] = chroma(hue);
            let error = atan2_turn(y, x).wrapping_sub(hue) as i16;
            assert!(error.unsigned_abs() < 48, "hue {hue}: error {error}");
        }
    }

    #[test]
    fn square_root_is_exact() {
        for value in [0, 1, 2, 3, 4, 15, 16, 17, 1 << 32, (1 << 32) + 1, u64::MAX] {
            let root = isqrt(value);
            assert!(
                root * root <= value && (root + 1).checked_mul(root + 1).is_none_or(|s| s > value)
            );
        }
    }
}

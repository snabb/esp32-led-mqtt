use super::{BLUE, WHITE, hash};
use crate::{EffectRuntime, blend_rgb, step_crossfade};
use smart_leds::RGB8;

const TILE_LEDS: usize = 4;
const FLIP_MS: u64 = 1_900;

fn tile_color(tile: u32, step: u64) -> RGB8 {
    // 40 % white keeps blue the ground, as on the flag.
    if hash(tile.wrapping_mul(0x0101_0101) ^ step as u32) % 5 < 2 {
        WHITE
    } else {
        BLUE
    }
}

/// Blocks of blue and white, each independently fading to a new random color.
pub(in crate::effects) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_ms = runtime.clock_us / 1000;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let tile = (index / TILE_LEDS) as u32;
        let shifted = clock_ms + u64::from(hash(tile)) % FLIP_MS;
        let step = shifted / FLIP_MS;
        let progress = ((shifted % FLIP_MS) * 65536 / FLIP_MS) as u16;
        *pixel = blend_rgb(
            tile_color(tile, step),
            tile_color(tile, step + 1),
            step_crossfade(progress),
        );
    }
}

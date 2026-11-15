//! Christmas effects, drawn for a vertical strip with LED 0 at the bottom.

use super::{hash, sin_q16};
use crate::{EffectRuntime, blend_rgb, scale_rgb, step_crossfade};
use smart_leds::RGB8;

const fn rgb(r: u8, g: u8, b: u8) -> RGB8 {
    RGB8 { r, g, b }
}

/// `sin_q16(phase)` mapped to 0..=255.
fn wave8(phase: u16) -> u32 {
    ((sin_q16(phase) + 65_536) * 255 / 131_072) as u32
}

const CANE_RED: RGB8 = rgb(230, 0, 12);
const CANE_WHITE: RGB8 = rgb(255, 235, 215);
const CANE_PERIOD_LEDS: u32 = 10;
const CANE_MS: u32 = 3_000;

/// Red and white stripes rising up the strip. The variation sets how much of
/// each stripe is white.
pub(super) fn candy_cane<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let time = runtime.phase(CANE_MS);
    let bias = (i32::from(runtime.params.variation) - 128) * 512;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let space = (index as u32 * 65_536 / CANE_PERIOD_LEDS) as u16;
        // Steepened so the edges are about one LED wide.
        let field = (sin_q16(space.wrapping_sub(time)) * 3 + bias).clamp(-65_536, 65_536);
        *pixel = blend_rgb(
            CANE_RED,
            CANE_WHITE,
            ((field + 65_536) * 255 / 131_072) as u8,
        );
    }
}

/// Incandescent C9 bulb colours, repeating along the string.
const BULBS: [RGB8; 5] = [
    rgb(255, 8, 0),
    rgb(0, 200, 20),
    rgb(255, 80, 0),
    rgb(10, 40, 255),
    rgb(255, 165, 15),
];
/// Dimmest bulb level, so the string never shows dark gaps.
const BULB_FLOOR: u32 = 90;

/// Colour of LED `position` when runs of `width` LEDs share a bulb colour.
fn bulb_color(position: usize, width: usize) -> RGB8 {
    BULBS[position / width % BULBS.len()]
}

/// Consecutive LEDs sharing a colour, 1 to 10, from the variation.
fn bulb_width<const N: usize>(runtime: &EffectRuntime<N>) -> usize {
    1 + usize::from(runtime.params.variation) * 10 / 256
}

/// A multicolour bulb string with a brightness wave chasing up it, one crest
/// every four LEDs.
pub(super) fn christmas_chase<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let width = bulb_width(runtime);
    let time = runtime.phase(1_200);
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let level = wave8(((index as u32 * 65_536 / 4) as u16).wrapping_sub(time));
        let level = BULB_FLOOR + level * (255 - BULB_FLOOR) / 255;
        *pixel = scale_rgb(bulb_color(index, width), level as u8);
    }
}

/// A multicolour bulb string whose colours move up one LED a second.
pub(super) fn christmas_flow<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let width = bulb_width(runtime);
    let shift = (runtime.clock_us / 1_000_000) as usize;
    let progress = (runtime.clock_us % 1_000_000 * 255 / 1_000_000) as u32;
    let weight = (progress * progress * (3 * 255 - 2 * progress) / (255 * 255)) as u8;
    // Offset by a whole colour cycle so the subtraction never wraps.
    let cycle = width * BULBS.len();
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let position = index + cycle - shift % cycle;
        *pixel = blend_rgb(
            bulb_color(position, width),
            bulb_color(position + cycle - 1, width),
            weight,
        );
    }
}

const TINSEL: [RGB8; 4] = [
    rgb(255, 150, 20),
    rgb(220, 10, 10),
    rgb(0, 170, 40),
    rgb(20, 60, 255),
];
/// Each colour holds, then crossfades into the next during the last 30 %.
const COLOR_MS: u32 = 10_000;
const GLINT_MS: u32 = 2_500;
const SWAY_MS: u32 = 7_000;
const BAND_MS: u32 = 5_000;

/// A metallic strand: the body brightness sways slowly while short glints
/// flash where bands of light drift upward. The colour cycles through gold,
/// red, green and blue.
pub(super) fn tinsel<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let step = runtime.step(COLOR_MS) as usize;
    let base = blend_rgb(
        TINSEL[step % TINSEL.len()],
        TINSEL[(step + 1) % TINSEL.len()],
        step_crossfade(runtime.phase(COLOR_MS)),
    );
    let highlight = blend_rgb(base, rgb(255, 250, 240), 170);
    let (glint, sway, band) = (
        runtime.phase(GLINT_MS),
        runtime.phase(SWAY_MS),
        runtime.phase(BAND_MS),
    );
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let body = 150 + wave8(sway.wrapping_add((index as u32 * 65_536 / 17) as u16)) * 80 / 255;
        // Each LED is a facet at a random angle that flashes as the light
        // passes it; c^16 keeps a flash to about a tenth of the turn.
        let facet = hash(index as u32 ^ 0x7153) as u16;
        let c = i64::from(sin_q16(facet.wrapping_add(glint)).max(0));
        let c2 = (c * c) >> 16;
        let c4 = (c2 * c2) >> 16;
        let c8 = (c4 * c4) >> 16;
        let c16 = (c8 * c8) >> 16;
        let envelope = wave8(((index as u32 * 65_536 / 25) as u16).wrapping_sub(band));
        let weight = ((c16 * 255) >> 16) as u32 * envelope / 255;
        *pixel = blend_rgb(scale_rgb(base, body as u8), highlight, weight as u8);
    }
}

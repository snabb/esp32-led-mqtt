//! Christmas effects, drawn for a vertical strip with LED 0 at the bottom.

use super::{hash, sin_q16};
use crate::{EffectRuntime, blend_rgb, half_sin16, phase_at, scale_rgb};
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
const PROGRAMME_MS: u32 = 12_000;
const PROGRAMMES: u32 = 5;
/// The last sixth of each programme crossfades into the next.
const PROGRAMME_FADE_FROM: u32 = 65_536 * 5 / 6;
/// Dimmest bulb level, so the string never shows dark gaps.
const BULB_FLOOR: u32 = 90;

/// A multicolour bulb string stepping through the programmes of an old light
/// controller: steady, slow glow, chase, dips and colours flowing upward.
pub(super) fn christmas_lights<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let clock_us = runtime.clock_us;
    let programme = runtime.step(PROGRAMME_MS);
    let progress = u32::from(runtime.phase(PROGRAMME_MS));
    let fade = progress.saturating_sub(PROGRAMME_FADE_FROM) * 255 / (65_536 - PROGRAMME_FADE_FROM);
    let fade = (fade * fade * (3 * 255 - 2 * fade) / (255 * 255)) as u8;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let current = bulb(programme % PROGRAMMES, index, clock_us);
        *pixel = if fade == 0 {
            current
        } else {
            let next = bulb((programme + 1) % PROGRAMMES, index, clock_us);
            blend_rgb(current, next, fade)
        };
    }
}

fn bulb(programme: u32, index: usize, clock_us: u64) -> RGB8 {
    let color = BULBS[index % BULBS.len()];
    let level = match programme {
        // Nearly steady, with a faint glow so a lit string looks alive.
        0 => {
            215 + wave8(phase_at(clock_us, 6_000).wrapping_add(hash(index as u32) as u16)) * 40
                / 255
        }
        // Alternate bulbs glow up and down in turn.
        1 => wave8(
            phase_at(clock_us, 4_000).wrapping_add(if index.is_multiple_of(2) {
                0
            } else {
                0x8000
            }),
        ),
        // A brightness wave chasing up the string, one crest every four bulbs.
        2 => wave8(((index as u32 * 65_536 / 4) as u16).wrapping_sub(phase_at(clock_us, 1_200))),
        // Each bulb dips briefly now and then, on its own rhythm.
        3 => {
            let random = hash(index as u32 ^ 0xc0de);
            let period_ms = 3_000 + random % 4_000;
            let phase = phase_at(clock_us + u64::from(random >> 12), period_ms);
            if phase < 0x4000 {
                255 - u32::from(half_sin16(phase << 2))
            } else {
                255
            }
        }
        // The colours move up one bulb a second.
        _ => {
            let shift = (clock_us / 1_000_000) as usize;
            let progress = (clock_us % 1_000_000 * 255 / 1_000_000) as u32;
            let weight = progress * progress * (3 * 255 - 2 * progress) / (255 * 255);
            let len = BULBS.len();
            let from = BULBS[(index + len - shift % len) % len];
            let to = BULBS[(index + 2 * len - shift % len - 1) % len];
            return blend_rgb(from, to, weight as u8);
        }
    };
    scale_rgb(color, (BULB_FLOOR + level * (255 - BULB_FLOOR) / 255) as u8)
}

/// Tinsel colours the variation slides between.
const TINSEL: [RGB8; 5] = [
    rgb(20, 60, 255),
    rgb(200, 205, 215),
    rgb(255, 150, 20),
    rgb(220, 10, 10),
    rgb(0, 170, 40),
];
const GLINT_MS: u32 = 2_500;
const SWAY_MS: u32 = 7_000;
const BAND_MS: u32 = 5_000;

/// A metallic strand: the body brightness sways slowly while short glints
/// flash where bands of light drift upward. The variation picks the colour:
/// blue, silver, gold (the default), red and green.
pub(super) fn tinsel<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let stops = u32::from(runtime.params.variation) * (TINSEL.len() as u32 - 1) * 256 / 255;
    let stop = (stops >> 8) as usize;
    let base = blend_rgb(
        TINSEL[stop],
        TINSEL[(stop + 1).min(TINSEL.len() - 1)],
        stops as u8,
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

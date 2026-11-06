use super::strip_position16;
use crate::{EffectRuntime, ease_wave16, hsv_rainbow16, scale8};

/// Travel and size periods of each blob, with no common rhythm.
const BLOBS: [(u32, u32); 5] = [
    (23_000, 7_100),
    (31_000, 11_300),
    (41_000, 13_700),
    (53_000, 17_900),
    (67_000, 19_300),
];
/// Blob radius in Q16 strip lengths.
const MIN_RADIUS: u32 = 3_600;
const RADIUS_SPAN: u32 = 2_400;
const HUE_MS: u32 = 240_000;
const FIELD_FROM: u32 = 16_000;
const FIELD_TO: u32 = 62_000;
const MIN_VALUE: u8 = 24;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let mut blobs = [(0_i32, 0_u32); BLOBS.len()];
    for (index, (blob, (travel_ms, size_ms))) in blobs.iter_mut().zip(BLOBS).enumerate() {
        // Spread the starting points so the blobs don't begin stacked at one end.
        let travel = runtime.phase(travel_ms).wrapping_add(index as u16 * 13_107);
        let size = u32::from(ease_wave16(runtime.phase(size_ms)));
        let radius = MIN_RADIUS + ((size * RADIUS_SPAN) >> 16);
        *blob = (i32::from(ease_wave16(travel)), (radius * radius) >> 16);
    }
    let base_hue = runtime.phase(HUE_MS);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let position = i32::from(strip_position16::<N>(index));
        let field = blobs
            .iter()
            .map(|&(centre, radius2)| {
                let distance = (position - centre).unsigned_abs();
                let lorentzian = (radius2 << 16) / (((distance * distance) >> 16) + radius2);
                // Squared, so that blobs have edges and don't blur into a plasma.
                (lorentzian * (lorentzian >> 1)) >> 15
            })
            .sum::<u32>();
        let level =
            (field.clamp(FIELD_FROM, FIELD_TO) - FIELD_FROM) * 255 / (FIELD_TO - FIELD_FROM);
        let level = (level * level * (3 * 255 - 2 * level) / (255 * 255)) as u8;
        let hue = base_hue.wrapping_add(u16::from(level) * 40);
        let value = MIN_VALUE + scale8(level, 255 - MIN_VALUE);
        *pixel = hsv_rainbow16(hue, 245, value);
    }
}

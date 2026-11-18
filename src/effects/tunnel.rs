use crate::{EffectRuntime, blend_rgb, ease_wave16, scale_rgb, scale8};
use smart_leds::RGB8;

const ONE: i32 = 65_536;
const BAND_MS: u32 = 1_250;
/// Perspective scale in Q16 half-strip lengths.
const PERSPECTIVE: i32 = ONE / 3;

const fn rgb(r: u8, g: u8, b: u8) -> RGB8 {
    RGB8 { r, g, b }
}

pub(super) const PRISM: [RGB8; 6] = [
    rgb(44, 190, 178),
    rgb(59, 117, 224),
    rgb(150, 70, 215),
    rgb(216, 81, 142),
    rgb(229, 148, 63),
    rgb(125, 195, 99),
];

pub(super) const STEEL: [RGB8; 6] = [
    rgb(104, 139, 172),
    rgb(128, 158, 181),
    rgb(89, 116, 151),
    rgb(149, 168, 187),
    rgb(97, 135, 157),
    rgb(117, 141, 166),
];

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, bands: &[RGB8; 6]) {
    // Keep the ribs prominent even when the selected background is white.
    let wall = scale_rgb(runtime.params.primary, 80);
    let band_time = runtime.clock_us / (u64::from(BAND_MS) * 1000);
    let band = (band_time % bands.len() as u64) as i32;
    let phase = i32::from(runtime.phase(BAND_MS));
    let half_width = 13_000 + i32::from(runtime.params.variation) * 62;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let position = if N == 1 {
            ONE / 2
        } else {
            (2 * index as i32 + 1 - N as i32) * ONE / N as i32
        };
        let radius = position.abs();
        // Reciprocal perspective packs the bands at the vanishing point and
        // makes each one cover more LEDs in the same time as it approaches.
        let depth =
            (i64::from(radius) * i64::from(4 * ONE) / i64::from(radius + PERSPECTIVE)) as i32;
        let travel = phase - depth;
        let color = (band + travel.div_euclid(ONE)).rem_euclid(bands.len() as i32);
        let within_band = travel.rem_euclid(ONE);
        let from_crest = (within_band - ONE / 2).abs();
        let band_phase = ((half_width - from_crest).max(0) * (ONE / 2) / half_width) as u16;
        // Dimming the distant ribs softens the convergence at the vanishing point.
        let visibility = (65 + radius.min(ONE / 4) * 190 / (ONE / 4)) as u8;
        let weight = scale8((ease_wave16(band_phase) >> 8) as u8, visibility);
        *pixel = blend_rgb(wall, bands[color as usize], weight);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectId, EffectParams};

    #[test]
    fn both_tunnel_backgrounds_follow_the_live_color() {
        for id in [EffectId::PrismTunnel, EffectId::SteelTunnel] {
            let params = EffectParams {
                id,
                primary: rgb(255, 0, 0),
                ..EffectParams::default()
            };
            let mut runtime = EffectRuntime::<3>::new(params);
            // At phase zero the center LED is between bands.
            assert_eq!(runtime.render(0)[1], rgb(80, 0, 0));
            runtime.set_effect(EffectParams {
                primary: rgb(0, 0, 255),
                ..params
            });
            assert_eq!(runtime.render(0)[1], rgb(0, 0, 80));
        }
    }
}

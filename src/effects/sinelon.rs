use super::{MAX_TICKS_PER_FRAME, add_smooth_dot};
use crate::{EffectRuntime, ease_wave16, fade_to_black, hsv_rainbow16, phase_at};

/// 13 BPM, one full back-and-forth.
const PERIOD_MS: u32 = 4_600;
const TICK_MS: u32 = 10;
/// Golden-ratio hue step, so consecutive turns always differ widely.
const HUE_STEP: u16 = 40_503;

/// A dot sweeping back and forth with a fading tail. Plain Sinelon uses the
/// primary colour; with `colour_per_turn` the colour changes at each end.
pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, colour_per_turn: bool) {
    let primary = runtime.params.primary;
    let fade = tail_fade(runtime.params.variation);
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, |runtime, time_us| {
        for pixel in &mut runtime.tick_frame {
            *pixel = fade_to_black(*pixel, fade);
        }
        let color = if colour_per_turn {
            let turn = time_us / (u64::from(PERIOD_MS) * 500);
            hsv_rainbow16((turn as u16).wrapping_mul(HUE_STEP), 255, 255)
        } else {
            primary
        };
        let wave = ease_wave16(phase_at(time_us, PERIOD_MS));
        add_smooth_dot(&mut runtime.tick_frame, wave, color);
    });
}

/// Fade per tick from the variation: 20 at the default 128, halving the fade
/// (doubling the tail) every 32 steps up and doubling it every 32 steps down.
fn tail_fade(variation: u8) -> u8 {
    let (octave, fraction) = (u32::from(variation) / 32, u32::from(variation) % 32);
    // 320 is 20 × 2^4, so variation 128 (octave 4) lands on 20.
    let high = 320_u32 << 8 >> octave;
    let low = high / 2;
    let fade = (high - (high - low) * fraction / 32) >> 8;
    fade.clamp(1, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::tail_fade;

    #[test]
    fn default_variation_keeps_the_original_tail() {
        assert_eq!(tail_fade(128), 20);
        assert_eq!(tail_fade(0), 255);
        assert_eq!(tail_fade(255), 1);
        assert!((1..=255).all(|v| tail_fade(v) <= tail_fade(v - 1)));
    }
}

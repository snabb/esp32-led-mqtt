use super::{BLUE, WHITE, band, signed_wave};
use crate::effects::strip_position16;
use crate::{EffectRuntime, blend_rgb, ease_wave16, scale_rgb};
use smart_leds::RGB8;

const Q16: i32 = 65536;
// The law's proportions: 5:3:10 along the fly and 4:3:4 up the hoist.
const BAR: (i32, i32) = (Q16 * 5 / 18, Q16 * 8 / 18);
const ARM: (i32, i32) = (Q16 * 4 / 11, Q16 * 7 / 11);
/// Row through the upper white field, far enough that waving never reaches the arm.
const UPPER_FIELD_ROW: i32 = Q16 * 2 / 11;
/// Softens the arm's edges, which the waving row crosses at about one flag % per LED.
const ARM_SOFTNESS: i32 = 2000;

const RIPPLE_MS: u32 = 2_100;
const BILLOW_MS: u32 = 3_370;
const GUST_MS: u32 = 17_000;
const ROW_DRIFT_MS: u32 = 37_000;
/// Deepest fold shadow, out of 255. Gamma correction makes it look about twice as deep.
const FOLD_SHADOW: i32 = 135;

/// A row of LEDs along the top of a flag pinned at the first LED: the blue bar
/// sways and light rolls through the folds.
pub(in crate::effects) fn wave<const N: usize>(runtime: &mut EffectRuntime<N>) {
    render_cloth(runtime, UPPER_FIELD_ROW);
}

/// As `wave`, but the row drifts down through the whole flag, so the waving cloth
/// carries the cross's horizontal arm in and out of view as moving blue patches.
pub(in crate::effects) fn cross<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let row = Q16 / 2 + signed_wave(runtime.phase(ROW_DRIFT_MS)) * 6 / 10;
    render_cloth(runtime, row);
}

fn render_cloth<const N: usize>(runtime: &mut EffectRuntime<N>, row: i32) {
    let ripple = runtime.phase(RIPPLE_MS);
    let billow = runtime.phase(BILLOW_MS);
    // Calm spells keep some movement, so the flag never hangs frozen.
    let gust = 130 + ((i32::from(ease_wave16(runtime.phase(GUST_MS))) * 125) >> 16);
    let soft = Q16 / N as i32;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let x = i32::from(strip_position16::<N>(index));
        let from_fly = (Q16 - 1 - x) as u32;
        // Pinned at the hoist, with slack growing towards the fly.
        let slack = 255 - ((from_fly * from_fly) >> 24) as i32;
        let swing = slack * gust / 255;

        // Phase falls with time, so the waves run from the hoist to the fly.
        let ripple_at = ((x as u32 * 3 / 2) as u16).wrapping_sub(ripple);
        let billow_at = ((x as u32 * 5 / 2) as u16).wrapping_sub(billow);
        let displacement = (signed_wave(ripple_at) * 5 + signed_wave(billow_at) * 3) / 8;
        // The folds' lighting follows the surface slope, a quarter cycle from its height.
        let slope = (signed_wave(ripple_at.wrapping_add(0x4000)) * 5
            + signed_wave(billow_at.wrapping_add(0x4000)) * 3)
            / 8;

        let u = x + displacement * swing / 255 / 5;
        let v = row + displacement * swing / 255 / 3;
        let cover = band(u, BAR.0, BAR.1, soft).max(band(v, ARM.0, ARM.1, ARM_SOFTNESS));

        let shadow = (32767 - slope) / 256 * swing * FOLD_SHADOW / (255 * 256);
        *pixel = scale_rgb(blend_rgb(WHITE, BLUE, cover), (255 - shadow) as u8);
    }
}

/// Three small flags, 18 LEDs long with a two-LED gap on a 60-LED strip, carried
/// along pole first with each flag fluttering on its own.
pub(in crate::effects) fn parade<const N: usize>(runtime: &mut EffectRuntime<N>) {
    const FLAGS: usize = 3;
    // Flag units of 1/20 period; gaps at the ends keep the wrap point off the flag.
    const UNIT: i32 = Q16 / 20;
    const MARCH_MS: u32 = 16_000;
    const FLUTTER_MS: u32 = 1_400;
    // Not `phase`: a wrap would renumber the flags and jump their flutter.
    let march = runtime.clock_us * 65536 / (u64::from(MARCH_MS) * 1000);
    let flutter = runtime.phase(FLUTTER_MS);
    let soft = (FLAGS as i32 * Q16) / N as i32;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let at = (index * FLAGS * 65536 / N) as u64 + march;
        let flag = at >> 16;
        let u = (at & 0xffff) as i32;

        let cloth = band(u, UNIT, UNIT * 19, soft);
        let bar = band(u, UNIT * 6, UNIT * 9, soft);
        let color: RGB8 = blend_rgb(WHITE, BLUE, bar);

        let along = (u - UNIT).clamp(0, 18 * UNIT);
        let phase = ((along * 2) as u16)
            .wrapping_sub(flutter)
            .wrapping_add((flag % FLAGS as u64 * 21_845) as u16);
        let depth = along / 256 * 100 / (18 * UNIT / 256);
        let shadow = (32767 - signed_wave(phase)) / 256 * depth / 256;

        *pixel = blend_rgb(
            scale_rgb(BLUE, 36),
            scale_rgb(color, (255 - shadow) as u8),
            cloth,
        );
    }
}

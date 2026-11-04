use super::{BLUE, WHITE, band, signed_wave};
use crate::effects::strip_position16;
use crate::{EffectRuntime, blend_rgb, ease_wave16, scale_rgb};

const Q16: i32 = 65536;
// The law's proportions: 5:3:10 along the fly and 4:3:4 up the hoist.
const BAR: (i32, i32) = (Q16 * 5 / 18, Q16 * 8 / 18);
const ARM: (i32, i32) = (Q16 * 4 / 11, Q16 * 7 / 11);
/// Column through the fly, far enough that waving never reaches the bar.
const FLY_COLUMN: i32 = Q16 * 13 / 18;
/// Softens the bar's edges, which the waving column crosses gradually.
const BAR_SOFTNESS: i32 = 2000;

const RIPPLE_MS: u32 = 2_100;
const BILLOW_MS: u32 = 3_370;
const GUST_MS: u32 = 17_000;
const COLUMN_DRIFT_MS: u32 = 37_000;
/// Deepest fold shadow, out of 255. Gamma correction makes it look about twice as deep.
const FOLD_SHADOW: i32 = 135;

/// A column of LEDs up the fly of a flag: white, blue arm, white. The arm sways
/// and light rolls through the folds.
pub(in crate::effects) fn wave<const N: usize>(runtime: &mut EffectRuntime<N>) {
    render_cloth(runtime, FLY_COLUMN);
}

/// As `wave`, but the column drifts across the whole flag: over the cross's upright
/// bar the strip floods blue, and near the hoist the cloth hangs almost still.
pub(in crate::effects) fn cross<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let column = Q16 / 2 + signed_wave(runtime.phase(COLUMN_DRIFT_MS)) * 9 / 10;
    render_cloth(runtime, column);
}

fn render_cloth<const N: usize>(runtime: &mut EffectRuntime<N>, column: i32) {
    let ripple = runtime.phase(RIPPLE_MS);
    let billow = runtime.phase(BILLOW_MS);
    // Calm spells keep some movement, so the flag never hangs frozen.
    let gust = 130 + ((i32::from(ease_wave16(runtime.phase(GUST_MS))) * 125) >> 16);
    let soft = Q16 / N as i32;

    let from_fly = (Q16 - 1 - column) as u32;
    // Pinned at the hoist, with slack growing towards the fly.
    let slack = 255 - ((from_fly * from_fly) >> 24) as i32;
    let swing = slack * gust / 255;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let y = i32::from(strip_position16::<N>(index));

        // Phase falls with time, so the folds roll up the strip.
        let ripple_at = ((y as u32 * 3 / 2) as u16).wrapping_sub(ripple);
        let billow_at = ((y as u32 * 5 / 2) as u16).wrapping_sub(billow);
        let displacement = (signed_wave(ripple_at) * 5 + signed_wave(billow_at) * 3) / 8;
        // The folds' lighting follows the surface slope, a quarter cycle from its height.
        let slope = (signed_wave(ripple_at.wrapping_add(0x4000)) * 5
            + signed_wave(billow_at.wrapping_add(0x4000)) * 3)
            / 8;

        let shifted = displacement * swing / 255 / 5;
        let u = column + shifted;
        let v = y + soft / 2 + shifted;
        let cover = band(v, ARM.0, ARM.1, soft).max(band(u, BAR.0, BAR.1, BAR_SOFTNESS));

        let shadow = (32767 - slope) / 256 * swing * FOLD_SHADOW / (255 * 256);
        *pixel = scale_rgb(blend_rgb(WHITE, BLUE, cover), (255 - shadow) as u8);
    }
}

/// The strip is the flagpole: the flag is hoisted, flies at the top, and is
/// lowered again, leaving the pole bare for a moment.
pub(in crate::effects) fn raising<const N: usize>(runtime: &mut EffectRuntime<N>) {
    const RISE_MS: u64 = 10_000;
    const FLY_MS: u64 = 30_000;
    const BARE_MS: u64 = 4_000;
    const CYCLE_MS: u64 = 2 * RISE_MS + FLY_MS + BARE_MS;
    const FLAG_HEIGHT: i32 = Q16 / 3;
    const FLUTTER_MS: u32 = 1_400;

    let eased = |elapsed_ms: u64| {
        let half_wave = (elapsed_ms * 32767 / RISE_MS) as u16;
        (i64::from(ease_wave16(half_wave)) * i64::from(Q16) / 65535) as i32
    };
    let at = runtime.clock_us / 1000 % CYCLE_MS;
    let top = if at < RISE_MS {
        eased(at)
    } else if at < RISE_MS + FLY_MS {
        Q16
    } else if at < 2 * RISE_MS + FLY_MS {
        Q16 - eased(at - RISE_MS - FLY_MS)
    } else {
        0
    };
    let bottom = top - FLAG_HEIGHT;

    let flutter = runtime.phase(FLUTTER_MS);
    let soft = Q16 / N as i32;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let y = i32::from(strip_position16::<N>(index)) + soft / 2;
        let phase = (((y - bottom) * 3) as u16).wrapping_sub(flutter);
        let shift = signed_wave(phase) / 16;
        let cloth = band(y, bottom, top, soft);
        let arm = band(
            y + shift,
            bottom + FLAG_HEIGHT * 4 / 11,
            bottom + FLAG_HEIGHT * 7 / 11,
            soft,
        );
        let shadow = (32767 - signed_wave(phase.wrapping_add(0x4000))) / 256 * 100 / 256;
        *pixel = blend_rgb(
            scale_rgb(BLUE, 36),
            scale_rgb(blend_rgb(WHITE, BLUE, arm), (255 - shadow) as u8),
            cloth,
        );
    }
}

use super::{MAX_TICKS_PER_FRAME, sin_q16, strip_position16};
use crate::{EffectRuntime, ease_wave16, hsv_rainbow16, phase_at, scale8};

const TICK_MS: u32 = 33;
/// Coupling rises and falls over this time: order forms, then falls apart.
const COUPLING_MS: u32 = 45_000;
/// Coupling per unit of summed sine at the peak. Much stronger locks the whole strip
/// into one blink, which hides the patches and waves.
const MAX_COUPLING: i64 = 350;
/// Natural rate around 0.6 Hz, as a 32-bit phase per tick.
const MEAN_RATE: i32 = 85_000_000;
/// About ±15 % per LED, uncorrelated between neighbours.
const RATE_NOISE: i32 = 390;
/// About ±20 % along the strip: locked patches then carry phase waves from fast to slow.
const RATE_WAVE: i32 = 260;
const REACH: usize = 2;
/// Amber while disordered, moving towards green as coupling takes hold.
const MIN_HUE: u16 = 0x1800;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, tick);
}

/// Kuramoto oscillators coupled to nearby LEDs; `field[i][0]` holds the phase, and
/// `field[i][1]` the next one so that all LEDs update from the same instant.
fn tick<const N: usize>(runtime: &mut EffectRuntime<N>, time_us: u64) {
    let order = ease_wave16(phase_at(time_us, COUPLING_MS));
    let coupling = (i64::from(order) * MAX_COUPLING) >> 16;
    let hue = MIN_HUE + (order >> 3);

    for index in 0..N {
        let phase = runtime.field[index][0];
        let pull: i64 = runtime.field[index.saturating_sub(REACH)..(index + REACH + 1).min(N)]
            .iter()
            .map(|other| i64::from(sin_q16((other[0].wrapping_sub(phase) >> 16) as u16)))
            .sum();
        runtime.field[index][1] = phase
            .wrapping_add(natural_rate::<N>(index))
            .wrapping_add((pull * coupling) as i32);
    }
    for index in 0..N {
        let phase = runtime.field[index][1];
        runtime.field[index][0] = phase;
        let wave = (ease_wave16((phase >> 16) as u16) >> 8) as u8;
        // Squared for a short flash with a long dark gap.
        runtime.tick_frame[index] = hsv_rainbow16(hue, 230, scale8(wave, wave));
    }
}

fn natural_rate<const N: usize>(index: usize) -> i32 {
    // Multiplicative hash: fixed per LED and uncorrelated between neighbours.
    let noise = ((index as u32).wrapping_mul(2_654_435_761) >> 16) as i32 - 32_768;
    let wave = sin_q16(strip_position16::<N>(index));
    MEAN_RATE + noise * RATE_NOISE + wave * RATE_WAVE
}

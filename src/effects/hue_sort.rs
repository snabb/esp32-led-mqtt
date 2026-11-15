//! The rainbow is shuffled into random colours, then sorted back by swapping
//! neighbours (odd-even transposition sort), so the colours flow into place.

use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, hsv_rainbow16, phase_at};

const TICK_MS: u32 = 120;
const SCRAMBLE_TICKS: u64 = 30;
/// Ticks of the sorted rainbow before the next shuffle.
const HOLD_TICKS: u64 = 20;
const HUE_MS: u32 = 60_000;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, tick);
}

/// `effect_data[i]` is the hue of LED `i` relative to the drifting base hue.
fn tick<const N: usize>(runtime: &mut EffectRuntime<N>, time_us: u64) {
    let values = &mut runtime.effect_data;
    if values.iter().all(|&value| value == 0) {
        for (index, value) in values.iter_mut().enumerate() {
            *value = (index * 256 / N) as u8;
        }
    }

    // Odd-even transposition sorts any order in N passes.
    let step = (time_us / (u64::from(TICK_MS) * 1000)) % (SCRAMBLE_TICKS + N as u64 + HOLD_TICKS);
    if step < SCRAMBLE_TICKS {
        // About N swaps in all, enough to look fully random.
        for _ in 0..N.div_ceil(SCRAMBLE_TICKS as usize) {
            let a = usize::from(runtime.rng.next_u16()) % N;
            let b = usize::from(runtime.rng.next_u16()) % N;
            values.swap(a, b);
        }
    } else if step < SCRAMBLE_TICKS + N as u64 {
        let parity = ((step - SCRAMBLE_TICKS) % 2) as usize;
        for index in (parity..N.saturating_sub(1)).step_by(2) {
            if values[index] > values[index + 1] {
                values.swap(index, index + 1);
            }
        }
    }

    let base = phase_at(time_us, HUE_MS);
    for (pixel, &value) in runtime.tick_frame.iter_mut().zip(values.iter()) {
        *pixel = hsv_rainbow16(base.wrapping_add(u16::from(value) << 8), 255, 255);
    }
}

use crate::{EffectRuntime, ease_wave16, hsv_rainbow16, scale8};

/// Every LED completes a whole number of cycles in this time, so they all realign.
const PERIOD_MS: u64 = 60_000;
const FIRST_CYCLES: u64 = 40;
const CYCLE_SPREAD: u64 = 59;
const HUE_MS: u32 = 180_000;
const MIN_VALUE: u8 = 48;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let period_us = PERIOD_MS * 1000;
    let time_us = runtime.clock_us % period_us;
    let base_hue = runtime.phase(HUE_MS);

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let cycles = FIRST_CYCLES + index as u64 * CYCLE_SPREAD / (N.max(2) as u64 - 1);
        let phase = (time_us * cycles * 65_536 / period_us) as u16;
        let wave = ease_wave16(phase);
        let value = MIN_VALUE + scale8((wave >> 8) as u8, 255 - MIN_VALUE);
        *pixel = hsv_rainbow16(base_hue.wrapping_add(wave / 4), 240, value);
    }
}

#[cfg(test)]
mod tests {
    use crate::{EffectId, EffectParams, EffectRuntime};

    #[test]
    fn all_leds_realign_after_a_full_period() {
        let mut runtime = EffectRuntime::<60>::new(EffectParams {
            id: EffectId::PendulumWave,
            ..EffectParams::default()
        });
        for frame in 0..=600 {
            runtime.render(frame * 100_000);
        }
        assert_eq!(runtime.clock_us, 60_000_000);
        let first = runtime.frame[0];
        assert!(runtime.frame.iter().all(|&pixel| pixel == first));
    }
}

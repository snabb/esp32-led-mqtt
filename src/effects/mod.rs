use crate::{EffectId, EffectRuntime, scale_rgb};
use smart_leds::RGB8;

// To add an effect: create a file in this directory, add it below, dispatch it
// in render(), then add its EffectId variant and EFFECTS entry in lib.rs.
mod aurora;
mod breathe;
mod color_wipe;
mod confetti;
mod finland;
mod fire2012;
mod fireflies;
mod fireworks;
mod flicker;
mod juggle;
mod lava_lamp;
mod pacifica;
mod pendulum_wave;
mod plasma_flow;
mod rainbow;
mod reaction_diffusion;
mod ripples;
mod scan;
mod sinelon;
mod static_noise;
mod theater_chase;
mod twinkle;

pub(crate) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    match runtime.params.id {
        EffectId::Rainbow => rainbow::render(runtime),
        EffectId::ColorWipe => color_wipe::render(runtime),
        EffectId::Scan => scan::render(runtime),
        EffectId::Twinkle => twinkle::render(runtime, false),
        EffectId::RandomTwinkle => twinkle::render(runtime, true),
        EffectId::Fireworks => fireworks::render(runtime),
        EffectId::Flicker => flicker::render(runtime),
        EffectId::Breathe => breathe::render(runtime),
        EffectId::TheaterChase => theater_chase::render(runtime),
        EffectId::Confetti => confetti::render(runtime),
        EffectId::Sinelon => sinelon::render(runtime),
        EffectId::Juggle => juggle::render(runtime),
        EffectId::Fire2012 => fire2012::render(runtime),
        EffectId::Pacifica => pacifica::render(runtime),
        EffectId::Aurora => aurora::render(runtime),
        EffectId::PlasmaFlow => plasma_flow::render(runtime),
        EffectId::StaticNoise => static_noise::render(runtime),
        EffectId::PendulumWave => pendulum_wave::render(runtime),
        EffectId::Ripples => ripples::render(runtime),
        EffectId::LavaLamp => lava_lamp::render(runtime),
        EffectId::Fireflies => fireflies::render(runtime),
        EffectId::ReactionDiffusion => reaction_diffusion::render(runtime),
        EffectId::FinlandSnow => finland::render(runtime, 2),
        EffectId::FinlandBlueWhiteTwinkle => finland::render(runtime, 3),
        EffectId::FinlandSnowfall => finland::render(runtime, 5),
        EffectId::FinlandRibbon => finland::render(runtime, 6),
        EffectId::FinlandFrost => finland::frost::render(runtime),
        EffectId::FinlandSnowflakes => finland::snowflakes::render(runtime),
        EffectId::FinlandRevontulet => finland::revontulet::render(runtime),
        EffectId::FinlandKaamos => finland::kaamos::render(runtime),
    }
}

/// Caps the simulation work of one frame after a stall or at high speed.
const MAX_TICKS_PER_FRAME: u64 = 64;

/// `index` scaled to 0..=u16::MAX across the strip.
fn strip_position16<const N: usize>(index: usize) -> u16 {
    ((index * 65536) / N) as u16
}

/// Position at `step` of a dot bouncing between both strip ends, one LED per step.
fn bounce(step: u32, count: usize) -> usize {
    if count == 1 {
        return 0;
    }

    let span = (count - 1) * 2;
    let step = step as usize % span;
    if step < count { step } else { span - step }
}

fn add_rgb(left: RGB8, right: RGB8) -> RGB8 {
    RGB8 {
        r: left.r.saturating_add(right.r),
        g: left.g.saturating_add(right.g),
        b: left.b.saturating_add(right.b),
    }
}

/// Adds `color` at `wave` (0 = first LED, u16::MAX = last), split between the
/// two nearest LEDs so the dot moves without hopping.
fn add_smooth_dot(frame: &mut [RGB8], wave: u16, color: RGB8) {
    let span = frame.len().saturating_sub(1) as u32;
    let position = u64::from(wave) * u64::from(span) * 256 / u64::from(u16::MAX);
    let index = (position >> 8) as usize;
    let next_weight = position as u8;
    frame[index] = add_rgb(frame[index], scale_rgb(color, 255 - next_weight));
    if let Some(next) = frame.get_mut(index + 1) {
        *next = add_rgb(*next, scale_rgb(color, next_weight));
    }
}

/// Sine of a full-circle phase in Q16, from an odd quintic on the quarter wave with
/// the true slope at zero and no slope jump at the peaks.
fn sin_q16(phase: u16) -> i32 {
    // pi/2 and the two terms that make the curve reach 1 with zero slope, in Q16.
    const A: i64 = 102_944;
    const B: i64 = -42_047;
    const C: i64 = 4_639;
    let signed = i64::from(phase as i16);
    let folded = if signed > 16_384 {
        32_768 - signed
    } else if signed < -16_384 {
        -32_768 - signed
    } else {
        signed
    };
    let x = folded << 2;
    let x2 = (x * x) >> 16;
    let x4 = (x2 * x2) >> 16;
    ((x * (A + ((B * x2 + C * x4) >> 16))) >> 16) as i32
}

#[cfg(test)]
mod tests {
    use super::sin_q16;

    #[test]
    fn sine_is_accurate() {
        for phase in (0..=u16::MAX).step_by(97) {
            let exact = (f64::from(phase) / 65_536.0 * core::f64::consts::TAU).sin();
            let error = (f64::from(sin_q16(phase)) / 65_536.0 - exact).abs();
            assert!(error < 0.001, "phase {phase}: error {error}");
        }
    }
}

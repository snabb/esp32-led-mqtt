use crate::{EffectId, EffectRuntime, scale_rgb};
use smart_leds::RGB8;

// To add an effect: create a file in this directory, add it below, dispatch it
// in render(), then add its EffectId variant and EFFECTS entry in lib.rs.
mod aurora;
mod bpm;
mod breathe;
mod circus;
mod color_wipe;
mod confetti;
mod fire2012;
mod fireworks;
mod flicker;
mod juggle;
mod pacifica;
mod plasma_flow;
mod police_car;
mod rainbow;
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
        EffectId::Bpm => bpm::render(runtime),
        EffectId::Fire2012 => fire2012::render(runtime),
        EffectId::Pacifica => pacifica::render(runtime),
        EffectId::Aurora => aurora::render(runtime),
        EffectId::PlasmaFlow => plasma_flow::render(runtime),
        EffectId::Circus => circus::render(runtime),
        EffectId::StaticNoise => static_noise::render(runtime),
        EffectId::PoliceCar => police_car::render(runtime),
    }
}

/// Caps the simulation work of one frame after a stall or at high speed.
const MAX_TICKS_PER_FRAME: u64 = 64;

/// `index` scaled to 0..=255 across the strip.
fn strip_position<const N: usize>(index: usize) -> u8 {
    ((index * 256) / N) as u8
}

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

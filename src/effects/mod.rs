use crate::{EffectId, EffectRuntime, speed_interval};

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

pub(crate) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    match runtime.params.id {
        EffectId::Rainbow => rainbow::render(runtime, now_ms),
        EffectId::ColorWipe => color_wipe::render(runtime, now_ms),
        EffectId::Scan => scan::render(runtime, now_ms),
        EffectId::Twinkle => twinkle::render(runtime, now_ms, false),
        EffectId::RandomTwinkle => twinkle::render(runtime, now_ms, true),
        EffectId::Fireworks => fireworks::render(runtime, now_ms),
        EffectId::Flicker => flicker::render(runtime, now_ms),
        EffectId::Breathe => breathe::render(runtime, now_ms),
        EffectId::TheaterChase => theater_chase::render(runtime, now_ms),
        EffectId::Confetti => confetti::render(runtime, now_ms),
        EffectId::Sinelon => sinelon::render(runtime, now_ms),
        EffectId::Juggle => juggle::render(runtime, now_ms),
        EffectId::Bpm => bpm::render(runtime, now_ms),
        EffectId::Fire2012 => fire2012::render(runtime, now_ms),
        EffectId::Pacifica => pacifica::render(runtime, now_ms),
        EffectId::Aurora => aurora::render(runtime, now_ms),
        EffectId::PlasmaFlow => plasma_flow::render(runtime, now_ms),
        EffectId::Circus => circus::render(runtime, now_ms),
        EffectId::StaticNoise => static_noise::render(runtime, now_ms),
        EffectId::PoliceCar => police_car::render(runtime, now_ms),
    }
}

/// `index` scaled to 0..=255 across the strip.
fn strip_position<const N: usize>(index: usize) -> u8 {
    ((index * 256) / N) as u8
}

/// Wrapping 8-bit wave phase; a higher speed or smaller divisor moves faster.
fn wave_phase(now_ms: u32, speed: u8, divisor: u32) -> u8 {
    (now_ms.saturating_mul(u32::from(speed.max(1))) / divisor) as u8
}

/// A position bouncing between both strip ends.
fn beat_position(now_ms: u32, speed: u8, count: usize) -> usize {
    if count == 1 {
        return 0;
    }

    let span = (count - 1) * 2;
    let step = ((now_ms / speed_interval(16, speed)) as usize) % span;
    if step < count { step } else { span - step }
}

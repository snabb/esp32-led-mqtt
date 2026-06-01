use crate::{EffectId, EffectRuntime};

// To add an effect: create a file in this directory, add it below, dispatch it
// in render(), then add its EffectId and EffectDefinition in lib.rs.
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
mod random_twinkle;
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
        EffectId::Twinkle => twinkle::render(runtime, now_ms),
        EffectId::RandomTwinkle => random_twinkle::render(runtime, now_ms),
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

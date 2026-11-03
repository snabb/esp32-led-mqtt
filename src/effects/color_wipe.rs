use crate::EffectRuntime;
use smart_leds::RGB8;

const STEP_MS: u32 = 50;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let primary = runtime.params.primary;
    runtime.render_steps(STEP_MS, |step, index| {
        if index < step as usize % (N + 1) {
            primary
        } else {
            RGB8 { r: 0, g: 0, b: 0 }
        }
    });
}

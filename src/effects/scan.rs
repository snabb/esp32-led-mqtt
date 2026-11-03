use super::bounce;
use crate::EffectRuntime;
use smart_leds::RGB8;

const STEP_MS: u32 = 40;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let primary = runtime.params.primary;
    let width = scan_width(N);
    runtime.render_steps(STEP_MS, |step, index| {
        let start = bounce(step, N);
        if (start..start + width).contains(&index) {
            primary
        } else {
            RGB8 { r: 0, g: 0, b: 0 }
        }
    });
}

fn scan_width(count: usize) -> usize {
    (count / 20).clamp(1, 4)
}

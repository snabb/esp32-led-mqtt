use crate::{EffectRuntime, scale_rgb};
use smart_leds::RGB8;

const STEP_MS: u32 = 70;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let center = N / 2;

    runtime.render_steps(STEP_MS, |step, index| {
        let flash = step % 12;
        let left_on = matches!(flash, 0 | 2 | 4);
        let right_on = matches!(flash, 6 | 8 | 10);
        let white_strobe = matches!(flash, 5 | 11);
        let side_on = if index < center { left_on } else { right_on };
        let base = if index < center {
            RGB8 { r: 255, g: 0, b: 0 }
        } else {
            RGB8 { r: 0, g: 0, b: 255 }
        };

        if white_strobe && police_strobe_pixel(index, N) {
            RGB8 {
                r: 255,
                g: 255,
                b: 255,
            }
        } else if side_on {
            base
        } else {
            scale_rgb(base, 20)
        }
    });
}

fn police_strobe_pixel(index: usize, count: usize) -> bool {
    if count <= 2 {
        return true;
    }

    let edge_width = (count / 12).clamp(1, 4);
    let center = count / 2;
    index < edge_width || index + edge_width >= count || index.abs_diff(center) <= edge_width / 2
}

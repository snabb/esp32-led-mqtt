use crate::{EffectRuntime, police_strobe_pixel, scale_rgb, speed_interval};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let step = (now_ms / speed_interval(45, runtime.params.speed)) as usize;
    let flash = step % 12;
    let left_on = matches!(flash, 0 | 2 | 4);
    let right_on = matches!(flash, 6 | 8 | 10);
    let white_strobe = matches!(flash, 5 | 11);
    let center = N / 2;

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let side_on = if index < center { left_on } else { right_on };
        let base = if index < center {
            RGB8 { r: 255, g: 0, b: 0 }
        } else {
            RGB8 { r: 0, g: 0, b: 255 }
        };

        *pixel = if white_strobe && police_strobe_pixel(index, N) {
            RGB8 {
                r: 255,
                g: 255,
                b: 255,
            }
        } else if side_on {
            base
        } else {
            scale_rgb(base, 20)
        };
    }
}

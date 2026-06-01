use crate::{EffectRuntime, circus_panel_color, speed_interval};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    let panel_width = (N / 12).clamp(3, 6);
    let border_width = 1 + usize::from(N >= 48);
    let group_width = panel_width + border_width;
    let step = (now_ms / speed_interval(120, runtime.params.speed)) as usize;
    let marquee_on = (step & 1) == 0;

    for (index, pixel) in runtime.frame.as_mut_slice().iter_mut().enumerate() {
        let moving = index + step;
        let position = moving % group_width;

        *pixel = if position < border_width {
            if (moving / group_width + usize::from(marquee_on)) & 1 == 0 {
                RGB8 {
                    r: 255,
                    g: 255,
                    b: 255,
                }
            } else {
                RGB8 {
                    r: 255,
                    g: 192,
                    b: 0,
                }
            }
        } else {
            circus_panel_color((moving / group_width) as u8)
        };
    }
}

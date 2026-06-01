use crate::{EffectRuntime, scan_width, speed_interval};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    if runtime.elapsed(now_ms, speed_interval(50, runtime.params.speed)) {
        if runtime.scan_forward {
            if runtime.scan_position + 1 >= N {
                runtime.scan_forward = false;
                runtime.scan_position = runtime.scan_position.saturating_sub(1);
            } else {
                runtime.scan_position += 1;
            }
        } else if runtime.scan_position == 0 {
            runtime.scan_forward = true;
            runtime.scan_position = (runtime.scan_position + 1).min(N.saturating_sub(1));
        } else {
            runtime.scan_position -= 1;
        }
    }

    runtime.frame.set_all(RGB8 { r: 0, g: 0, b: 0 });
    for offset in 0..scan_width(N) {
        let index = runtime.scan_position.saturating_add(offset);
        if index < N {
            runtime.frame.as_mut_slice()[index] = runtime.params.primary;
        }
    }
}

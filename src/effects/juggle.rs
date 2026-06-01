use crate::{EffectRuntime, add_rgb, beat_position, fade_to_black, hsv_rainbow, juggle_dot_speed};

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    for pixel in runtime.frame.as_mut_slice() {
        *pixel = fade_to_black(*pixel, 48);
    }

    if N == 0 {
        return;
    }

    let dots = N.min(6);
    for dot in 0..dots {
        let speed = juggle_dot_speed(runtime.params.speed, dot as u8);
        let time = now_ms / 4;
        if let Some(position) = beat_position(time.wrapping_add((dot * 73) as u32), speed, N) {
            let hue = (dot as u8)
                .wrapping_mul(32)
                .wrapping_add(runtime.phase as u8);
            let color = hsv_rainbow(hue, 220, 255);
            let blended = add_rgb(runtime.frame.as_slice()[position], color);
            runtime.frame.as_mut_slice()[position] = blended;
        }
    }
    runtime.phase = runtime.phase.wrapping_add(1);
}

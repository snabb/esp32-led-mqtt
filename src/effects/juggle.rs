use super::beat_position;
use crate::{EffectRuntime, fade_to_black, hsv_rainbow};
use smart_leds::RGB8;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    for pixel in &mut runtime.frame {
        *pixel = fade_to_black(*pixel, 48);
    }

    let dots = N.min(6);
    for dot in 0..dots {
        let speed = juggle_dot_speed(runtime.params.speed, dot as u8);
        let time = now_ms / 4;
        let position = beat_position(time.wrapping_add((dot * 73) as u32), speed, N);
        let hue = (dot as u8)
            .wrapping_mul(32)
            .wrapping_add(runtime.phase as u8);
        let color = hsv_rainbow(hue, 220, 255);
        runtime.frame[position] = add_rgb(runtime.frame[position], color);
    }
    runtime.phase = runtime.phase.wrapping_add(1);
}

fn juggle_dot_speed(base_speed: u8, dot: u8) -> u8 {
    let speed = u16::from(base_speed) + u16::from(dot) * 3;
    speed.min(u16::from(u8::MAX)) as u8
}

fn add_rgb(left: RGB8, right: RGB8) -> RGB8 {
    RGB8 {
        r: left.r.saturating_add(right.r),
        g: left.g.saturating_add(right.g),
        b: left.b.saturating_add(right.b),
    }
}

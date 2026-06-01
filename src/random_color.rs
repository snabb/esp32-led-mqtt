use smart_leds::RGB8;

const RANDOM_COLOR_MIN_DISTANCE: u16 = 160;
const RANDOM_COLOR_MIN_BRIGHTNESS: u16 = 260;
const RANDOM_COLOR_ATTEMPTS: usize = 12;
const DISTINCT_COLOR_FALLBACKS: [RGB8; 8] = [
    RGB8 { r: 255, g: 0, b: 0 },
    RGB8 { r: 0, g: 255, b: 0 },
    RGB8 { r: 0, g: 0, b: 255 },
    RGB8 {
        r: 255,
        g: 255,
        b: 0,
    },
    RGB8 {
        r: 0,
        g: 255,
        b: 255,
    },
    RGB8 {
        r: 255,
        g: 0,
        b: 255,
    },
    RGB8 {
        r: 255,
        g: 96,
        b: 0,
    },
    RGB8 {
        r: 96,
        g: 0,
        b: 255,
    },
];

pub fn from_seed(previous: RGB8, seed: u32) -> RGB8 {
    let mut rng = crate::XorShift32::new(seed ^ 0xa5a5_5a5a);

    for _ in 0..RANDOM_COLOR_ATTEMPTS {
        let candidate = RGB8 {
            r: rng.next_u8(),
            g: rng.next_u8(),
            b: rng.next_u8(),
        };
        if color_distance(previous, candidate) >= RANDOM_COLOR_MIN_DISTANCE
            && color_brightness(candidate) >= RANDOM_COLOR_MIN_BRIGHTNESS
        {
            return candidate;
        }
    }

    farthest_fallback_color(previous)
}

pub fn color_distance(left: RGB8, right: RGB8) -> u16 {
    u16::from(left.r.abs_diff(right.r))
        + u16::from(left.g.abs_diff(right.g))
        + u16::from(left.b.abs_diff(right.b))
}

pub fn color_brightness(color: RGB8) -> u16 {
    u16::from(color.r) + u16::from(color.g) + u16::from(color.b)
}

fn farthest_fallback_color(previous: RGB8) -> RGB8 {
    let mut best = DISTINCT_COLOR_FALLBACKS[0];
    let mut best_distance = color_distance(previous, best);

    for candidate in DISTINCT_COLOR_FALLBACKS.iter().copied().skip(1) {
        let distance = color_distance(previous, candidate);
        if distance > best_distance {
            best = candidate;
            best_distance = distance;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_color_moves_far_from_previous_color() {
        for previous in [
            RGB8 { r: 0, g: 0, b: 0 },
            RGB8 {
                r: 255,
                g: 96,
                b: 24,
            },
            RGB8 {
                r: 128,
                g: 128,
                b: 128,
            },
            RGB8 {
                r: 255,
                g: 255,
                b: 255,
            },
        ] {
            for seed in 0..64 {
                let next = from_seed(previous, seed);
                assert!(
                    color_distance(previous, next) >= RANDOM_COLOR_MIN_DISTANCE,
                    "{previous:?} -> {next:?} was too close"
                );
                assert!(
                    color_brightness(next) >= RANDOM_COLOR_MIN_BRIGHTNESS,
                    "{next:?} was too dark"
                );
            }
        }
    }
}

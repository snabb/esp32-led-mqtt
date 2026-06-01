#![cfg_attr(not(test), no_std)]

mod effects;

use smart_leds::{
    RGB8,
    hsv::{Hsv, hsv2rgb},
};

pub const DEFAULT_BRIGHTNESS: u8 = 8;
pub const EFFECT_DISABLED_NAME: &str = "None";
pub const EFFECT_NONE_CODE: u8 = 0;
pub const EFFECT_MAX_CODE: u8 = 20;
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
pub const EFFECT_DEFINITIONS: [EffectDefinition; 20] = [
    EffectDefinition::new(EffectId::Rainbow, 1, "Rainbow"),
    EffectDefinition::new(EffectId::ColorWipe, 2, "Color Wipe"),
    EffectDefinition::new(EffectId::Scan, 3, "Scan"),
    EffectDefinition::new(EffectId::Twinkle, 4, "Twinkle"),
    EffectDefinition::new(EffectId::RandomTwinkle, 5, "Random Twinkle"),
    EffectDefinition::new(EffectId::Fireworks, 6, "Fireworks"),
    EffectDefinition::new(EffectId::Flicker, 7, "Flicker"),
    EffectDefinition::new(EffectId::Breathe, 8, "Breathe"),
    EffectDefinition::new(EffectId::TheaterChase, 9, "Theater Chase"),
    EffectDefinition::new(EffectId::Confetti, 10, "Confetti"),
    EffectDefinition::new(EffectId::Sinelon, 11, "Sinelon"),
    EffectDefinition::new(EffectId::Juggle, 12, "Juggle"),
    EffectDefinition::new(EffectId::Bpm, 13, "BPM"),
    EffectDefinition::new(EffectId::Fire2012, 14, "Fire 2012"),
    EffectDefinition::new(EffectId::Pacifica, 15, "Pacifica"),
    EffectDefinition::new(EffectId::Aurora, 16, "Aurora"),
    EffectDefinition::new(EffectId::PlasmaFlow, 17, "Plasma Flow"),
    EffectDefinition::new(EffectId::Circus, 18, "Circus"),
    EffectDefinition::new(EffectId::StaticNoise, 19, "Static Noise"),
    EffectDefinition::new(EffectId::PoliceCar, EFFECT_MAX_CODE, "Police Car"),
];
pub const EFFECT_IDS: [EffectId; 20] = [
    EFFECT_DEFINITIONS[0].id,
    EFFECT_DEFINITIONS[1].id,
    EFFECT_DEFINITIONS[2].id,
    EFFECT_DEFINITIONS[3].id,
    EFFECT_DEFINITIONS[4].id,
    EFFECT_DEFINITIONS[5].id,
    EFFECT_DEFINITIONS[6].id,
    EFFECT_DEFINITIONS[7].id,
    EFFECT_DEFINITIONS[8].id,
    EFFECT_DEFINITIONS[9].id,
    EFFECT_DEFINITIONS[10].id,
    EFFECT_DEFINITIONS[11].id,
    EFFECT_DEFINITIONS[12].id,
    EFFECT_DEFINITIONS[13].id,
    EFFECT_DEFINITIONS[14].id,
    EFFECT_DEFINITIONS[15].id,
    EFFECT_DEFINITIONS[16].id,
    EFFECT_DEFINITIONS[17].id,
    EFFECT_DEFINITIONS[18].id,
    EFFECT_DEFINITIONS[19].id,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectDefinition {
    pub id: EffectId,
    pub code: u8,
    pub name: &'static str,
}

impl EffectDefinition {
    const fn new(id: EffectId, code: u8, name: &'static str) -> Self {
        Self { id, code, name }
    }
}

#[derive(Clone, Copy)]
pub struct RgbFrame<const N: usize> {
    pixels: [RGB8; N],
}

impl<const N: usize> RgbFrame<N> {
    pub const fn new() -> Self {
        Self {
            pixels: [RGB8 { r: 0, g: 0, b: 0 }; N],
        }
    }

    pub fn as_slice(&self) -> &[RGB8] {
        &self.pixels
    }

    pub fn as_mut_slice(&mut self) -> &mut [RGB8] {
        &mut self.pixels
    }

    pub fn set_all(&mut self, color: RGB8) {
        self.pixels.fill(color);
    }

    pub fn fill_wheel(&mut self, base_hue: u8) {
        fill_wheel(&mut self.pixels, base_hue);
    }

    pub fn corrected(&self, brightness: u8) -> [RGB8; N] {
        let mut corrected = [RGB8 { r: 0, g: 0, b: 0 }; N];
        apply_brightness_gamma(&self.pixels, &mut corrected, brightness);
        corrected
    }
}

impl<const N: usize> Default for RgbFrame<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectId {
    Rainbow,
    ColorWipe,
    Scan,
    Twinkle,
    RandomTwinkle,
    Fireworks,
    Flicker,
    Breathe,
    TheaterChase,
    Confetti,
    Sinelon,
    Juggle,
    Bpm,
    Fire2012,
    Pacifica,
    Aurora,
    PlasmaFlow,
    Circus,
    StaticNoise,
    PoliceCar,
}

impl EffectId {
    pub fn name(self) -> &'static str {
        effect_definition(self).name
    }

    pub fn from_name(name: &str) -> Option<Self> {
        for definition in EFFECT_DEFINITIONS {
            if name.eq_ignore_ascii_case(definition.name) {
                return Some(definition.id);
            }
        }
        None
    }
}

pub fn effect_definition(id: EffectId) -> EffectDefinition {
    for definition in EFFECT_DEFINITIONS {
        if definition.id == id {
            return definition;
        }
    }
    EFFECT_DEFINITIONS[0]
}

pub fn effect_id_from_code(code: u8) -> Option<EffectId> {
    if code == EFFECT_NONE_CODE {
        return None;
    }

    for definition in EFFECT_DEFINITIONS {
        if definition.code == code {
            return Some(definition.id);
        }
    }

    Some(EffectId::Rainbow)
}

pub fn effect_code_from_id(id: EffectId) -> u8 {
    effect_definition(id).code
}

#[derive(Clone, Copy, Debug)]
pub struct EffectParams {
    pub id: EffectId,
    pub primary: RGB8,
    pub speed: u8,
    pub intensity: u8,
}

impl Default for EffectParams {
    fn default() -> Self {
        Self {
            id: EffectId::Rainbow,
            primary: RGB8 {
                r: 255,
                g: 96,
                b: 24,
            },
            speed: 10,
            intensity: 128,
        }
    }
}

pub struct EffectRuntime<const N: usize> {
    frame: RgbFrame<N>,
    params: EffectParams,
    last_step_ms: u32,
    phase: u16,
    wipe_index: usize,
    scan_position: usize,
    scan_forward: bool,
    effect_data: [u8; N],
    twinkle_color: [RGB8; N],
    rng: XorShift32,
}

impl<const N: usize> EffectRuntime<N> {
    pub const fn new(params: EffectParams) -> Self {
        Self {
            frame: RgbFrame::new(),
            params,
            last_step_ms: 0,
            phase: 0,
            wipe_index: 0,
            scan_position: 0,
            scan_forward: true,
            effect_data: [0; N],
            twinkle_color: [RGB8 { r: 0, g: 0, b: 0 }; N],
            rng: XorShift32::new(0x1234_abcd),
        }
    }

    pub fn set_effect(&mut self, params: EffectParams) {
        if self.params.id != params.id {
            self.reset();
        }
        self.params = params;
    }

    pub fn render(&mut self, now_ms: u32) -> &RgbFrame<N> {
        effects::render(self, now_ms);
        &self.frame
    }

    fn reset(&mut self) {
        self.last_step_ms = 0;
        self.phase = 0;
        self.wipe_index = 0;
        self.scan_position = 0;
        self.scan_forward = true;
        self.effect_data.fill(0);
        self.twinkle_color.fill(RGB8 { r: 0, g: 0, b: 0 });
        self.frame.set_all(RGB8 { r: 0, g: 0, b: 0 });
    }

    fn elapsed(&mut self, now_ms: u32, interval_ms: u32) -> bool {
        if self.last_step_ms == 0 || now_ms.wrapping_sub(self.last_step_ms) >= interval_ms {
            self.last_step_ms = now_ms;
            true
        } else {
            false
        }
    }

    fn chance(&mut self, intensity: u8, percent_at_full: u8) -> bool {
        let threshold = (u32::from(intensity) * u32::from(percent_at_full) * 255) / 25500;
        u32::from(self.rng.next_u8()) < threshold
    }
}

pub fn fill_solid(frame: &mut [RGB8], color: RGB8) {
    frame.fill(color);
}

pub fn fill_wheel(frame: &mut [RGB8], base_hue: u8) {
    let count = frame.len().max(1);

    for (index, pixel) in frame.iter_mut().enumerate() {
        let hue = base_hue.wrapping_add(((index * 256) / count) as u8);
        *pixel = hsv2rgb(Hsv {
            hue,
            sat: 255,
            val: 255,
        });
    }
}

pub fn apply_brightness_gamma(input: &[RGB8], output: &mut [RGB8], brightness: u8) {
    for (source, target) in input.iter().zip(output.iter_mut()) {
        *target = RGB8 {
            r: scale8(gamma_correct(source.r), brightness),
            g: scale8(gamma_correct(source.g), brightness),
            b: scale8(gamma_correct(source.b), brightness),
        };
    }
}

pub fn hsv_rainbow(hue: u8, sat: u8, val: u8) -> RGB8 {
    let region = hue / 43;
    let remainder = (u16::from(hue % 43) * 6) as u8;
    let p = scale8(val, 255 - sat);
    let q = scale8(val, 255 - scale8(sat, remainder));
    let t = scale8(val, 255 - scale8(sat, 255 - remainder));

    match region {
        0 => RGB8 { r: val, g: t, b: p },
        1 => RGB8 { r: q, g: val, b: p },
        2 => RGB8 { r: p, g: val, b: t },
        3 => RGB8 { r: p, g: q, b: val },
        4 => RGB8 { r: t, g: p, b: val },
        _ => RGB8 { r: val, g: p, b: q },
    }
}

pub fn scale8(value: u8, scale: u8) -> u8 {
    ((u16::from(value) * u16::from(scale)) / 255) as u8
}

pub fn fade_to_black(color: RGB8, amount: u8) -> RGB8 {
    let keep = 255_u8.saturating_sub(amount);
    scale_rgb(color, keep)
}

pub fn scale_rgb(color: RGB8, scale: u8) -> RGB8 {
    RGB8 {
        r: scale8(color.r, scale),
        g: scale8(color.g, scale),
        b: scale8(color.b, scale),
    }
}

pub fn half_sin8(value: u8) -> u8 {
    let value = u16::from(value);
    if value < 128 {
        let x = value * 2;
        ((x * x) / 255) as u8
    } else {
        let x = (255 - value) * 2;
        ((x * x) / 255) as u8
    }
}

pub fn random_distinct_color(previous: RGB8, seed: u32) -> RGB8 {
    let mut rng = XorShift32::new(seed ^ 0xa5a5_5a5a);

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

fn gamma_correct(value: u8) -> u8 {
    let value = u16::from(value);
    ((value * value) / u16::from(u8::MAX)) as u8
}

fn scan_width(count: usize) -> usize {
    (count / 20).clamp(1, 4)
}

fn police_strobe_pixel(index: usize, count: usize) -> bool {
    if count <= 2 {
        return true;
    }

    let edge_width = (count / 12).clamp(1, 4);
    let center = count / 2;
    index < edge_width || index + edge_width >= count || index.abs_diff(center) <= edge_width / 2
}

fn speed_interval(base_ms: u32, speed: u8) -> u32 {
    ((base_ms * 10) / u32::from(speed.max(1))).max(1)
}

fn juggle_dot_speed(base_speed: u8, dot: u8) -> u8 {
    let speed = u16::from(base_speed) + u16::from(dot) * 3;
    speed.min(u16::from(u8::MAX)) as u8
}

fn beat_position(now_ms: u32, speed: u8, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    if count == 1 {
        return Some(0);
    }

    let span = (count - 1) * 2;
    let step = ((now_ms / speed_interval(16, speed)) as usize) % span;
    Some(if step < count { step } else { span - step })
}

fn add_rgb(left: RGB8, right: RGB8) -> RGB8 {
    RGB8 {
        r: left.r.saturating_add(right.r),
        g: left.g.saturating_add(right.g),
        b: left.b.saturating_add(right.b),
    }
}

fn heat_color(heat: u8) -> RGB8 {
    let ramp = (heat & 0x3f) << 2;
    match heat >> 6 {
        0 => RGB8 {
            r: ramp,
            g: 0,
            b: 0,
        },
        1 => RGB8 {
            r: 255,
            g: ramp,
            b: 0,
        },
        2 => RGB8 {
            r: 255,
            g: 255,
            b: ramp,
        },
        _ => RGB8 {
            r: 255,
            g: 255,
            b: 255,
        },
    }
}

fn circus_panel_color(index: u8) -> RGB8 {
    match index % 4 {
        0 => RGB8 { r: 255, g: 0, b: 0 },
        1 => RGB8 {
            r: 0,
            g: 64,
            b: 255,
        },
        2 => RGB8 {
            r: 255,
            g: 224,
            b: 0,
        },
        _ => RGB8 {
            r: 255,
            g: 0,
            b: 160,
        },
    }
}

fn fire_spark_height(count: usize) -> usize {
    (count / 6).clamp(1, 10)
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

#[derive(Clone, Copy)]
struct XorShift32 {
    state: u32,
}

impl XorShift32 {
    const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn next_u16(&mut self) -> u16 {
        self.next_u32() as u16
    }

    fn next_u8(&mut self) -> u8 {
        self.next_u32() as u8
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_sets_every_pixel() {
        let mut frame = RgbFrame::<4>::new();
        let color = RGB8 { r: 1, g: 2, b: 3 };

        frame.set_all(color);

        assert_eq!(frame.as_slice(), &[color; 4]);
    }

    #[test]
    fn brightness_gamma_never_increases_channels() {
        let input = [RGB8 {
            r: 128,
            g: 200,
            b: 255,
        }];
        let mut output = [RGB8 { r: 0, g: 0, b: 0 }];

        apply_brightness_gamma(&input, &mut output, 128);

        assert!(output[0].r <= input[0].r);
        assert!(output[0].g <= input[0].g);
        assert!(output[0].b <= input[0].b);
    }

    #[test]
    fn random_distinct_color_moves_far_from_previous_color() {
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
                let next = random_distinct_color(previous, seed);
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

    #[test]
    fn rainbow_effect_changes_over_time() {
        let mut runtime = EffectRuntime::<8>::new(EffectParams::default());
        let first = runtime.render(0).as_slice().to_vec();
        let second = runtime.render(100).as_slice().to_vec();

        assert_ne!(first, second);
    }

    #[test]
    fn effect_names_round_trip() {
        for effect in EFFECT_IDS {
            assert_eq!(EffectId::from_name(effect.name()), Some(effect));
        }
    }

    #[test]
    fn effect_names_ignore_case() {
        assert_eq!(
            EffectId::from_name("random twinkle"),
            Some(EffectId::RandomTwinkle)
        );
        assert_eq!(EffectId::from_name("bpm"), Some(EffectId::Bpm));
        assert_eq!(EffectId::from_name("fire 2012"), Some(EffectId::Fire2012));
        assert_eq!(EffectId::from_name("missing"), None);
    }

    #[test]
    fn new_effects_change_over_time() {
        for effect in [
            EffectId::Breathe,
            EffectId::TheaterChase,
            EffectId::Confetti,
            EffectId::Sinelon,
            EffectId::Juggle,
            EffectId::Bpm,
            EffectId::Fire2012,
            EffectId::Pacifica,
            EffectId::Aurora,
            EffectId::PlasmaFlow,
            EffectId::Circus,
            EffectId::StaticNoise,
            EffectId::PoliceCar,
        ] {
            let mut runtime = EffectRuntime::<8>::new(EffectParams {
                id: effect,
                speed: 10,
                intensity: 255,
                ..EffectParams::default()
            });
            let first = runtime.render(1).as_slice().to_vec();
            let second = runtime.render(240).as_slice().to_vec();

            assert_ne!(first, second, "{effect:?} should animate");
        }
    }

    #[test]
    fn new_effects_render_small_strips() {
        for effect in [
            EffectId::Fire2012,
            EffectId::Pacifica,
            EffectId::Aurora,
            EffectId::PlasmaFlow,
            EffectId::Circus,
            EffectId::StaticNoise,
            EffectId::PoliceCar,
        ] {
            let mut runtime = EffectRuntime::<1>::new(EffectParams {
                id: effect,
                speed: 10,
                intensity: 255,
                ..EffectParams::default()
            });

            runtime.render(1);
            runtime.render(80);
        }
    }
}

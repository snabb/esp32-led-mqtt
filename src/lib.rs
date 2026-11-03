#![cfg_attr(not(test), no_std)]

#[cfg(test)]
mod c_shims;
pub mod config_store;
pub mod device_config;
mod effects;
pub mod ota_protocol;
pub mod provisioning;
pub mod random_color;

use smart_leds::RGB8;

/// Home Assistant name for "no effect", which shows the solid color.
pub const EFFECT_DISABLED_NAME: &str = "None";
/// Display names in `EffectId` declaration order; also the button cycle order.
pub const EFFECTS: [(EffectId, &str); 20] = [
    (EffectId::Rainbow, "Rainbow"),
    (EffectId::ColorWipe, "Color Wipe"),
    (EffectId::Scan, "Scan"),
    (EffectId::Twinkle, "Twinkle"),
    (EffectId::RandomTwinkle, "Random Twinkle"),
    (EffectId::Fireworks, "Fireworks"),
    (EffectId::Flicker, "Flicker"),
    (EffectId::Breathe, "Breathe"),
    (EffectId::TheaterChase, "Theater Chase"),
    (EffectId::Confetti, "Confetti"),
    (EffectId::Sinelon, "Sinelon"),
    (EffectId::Juggle, "Juggle"),
    (EffectId::Bpm, "BPM"),
    (EffectId::Fire2012, "Fire 2012"),
    (EffectId::Pacifica, "Pacifica"),
    (EffectId::Aurora, "Aurora"),
    (EffectId::PlasmaFlow, "Plasma Flow"),
    (EffectId::Circus, "Circus"),
    (EffectId::StaticNoise, "Static Noise"),
    (EffectId::PoliceCar, "Police Car"),
];

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
        EFFECTS[self as usize].1
    }

    pub fn from_name(name: &str) -> Option<Self> {
        EFFECTS
            .iter()
            .find(|(_, effect_name)| name.eq_ignore_ascii_case(effect_name))
            .map(|&(id, _)| id)
    }

    /// The next effect in `EFFECTS` order, with `None` between the last and first.
    pub fn cycle(current: Option<Self>) -> Option<Self> {
        let next = current.map_or(0, |id| id as usize + 1);
        EFFECTS.get(next).map(|&(id, _)| id)
    }
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
    frame: [RGB8; N],
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
        // Effects index and divide by N; build.rs requires at least one LED.
        const { assert!(N > 0) };
        Self {
            frame: [RGB8 { r: 0, g: 0, b: 0 }; N],
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

    pub fn render(&mut self, now_ms: u32) -> &[RGB8; N] {
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
        self.frame.fill(RGB8 { r: 0, g: 0, b: 0 });
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
        let threshold = u32::from(intensity) * u32::from(percent_at_full) / 100;
        u32::from(self.rng.next_u8()) < threshold
    }
}

/// Gamma-correct and scale a frame for output.
pub fn corrected<const N: usize>(frame: &[RGB8; N], brightness: u8) -> [RGB8; N] {
    frame.map(|pixel| RGB8 {
        r: scale8(gamma_correct(pixel.r), brightness),
        g: scale8(gamma_correct(pixel.g), brightness),
        b: scale8(gamma_correct(pixel.b), brightness),
    })
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

fn gamma_correct(value: u8) -> u8 {
    let value = u16::from(value);
    ((value * value) / u16::from(u8::MAX)) as u8
}

fn speed_interval(base_ms: u32, speed: u8) -> u32 {
    ((base_ms * 10) / u32::from(speed.max(1))).max(1)
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
    fn brightness_gamma_never_increases_channels() {
        let input = [RGB8 {
            r: 128,
            g: 200,
            b: 255,
        }];
        let output = corrected(&input, 128);

        assert!(output[0].r <= input[0].r);
        assert!(output[0].g <= input[0].g);
        assert!(output[0].b <= input[0].b);
    }

    #[test]
    fn effect_table_matches_declaration_order() {
        for (index, (id, name)) in EFFECTS.into_iter().enumerate() {
            assert_eq!(id as usize, index, "{name} is out of order");
            assert_eq!(id.name(), name);
            assert_eq!(EffectId::from_name(name), Some(id));
        }
    }

    #[test]
    fn button_cycle_visits_every_effect_then_none() {
        let mut current = None;
        for (id, _) in EFFECTS {
            current = EffectId::cycle(current);
            assert_eq!(current, Some(id));
        }
        assert_eq!(EffectId::cycle(current), None);
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

    fn runtime<const N: usize>(id: EffectId) -> EffectRuntime<N> {
        EffectRuntime::new(EffectParams {
            id,
            speed: 10,
            intensity: 255,
            ..EffectParams::default()
        })
    }

    #[test]
    fn every_effect_animates() {
        for (id, name) in EFFECTS {
            let mut runtime = runtime::<8>(id);
            let first = runtime.render(1).to_vec();
            // Random effects may repeat a frame, so allow two seconds of frames.
            let animates = (1..=120).any(|frame| runtime.render(frame * 16 + 1)[..] != first[..]);
            assert!(animates, "{name} should animate");
        }
    }

    #[test]
    fn every_effect_renders_a_single_led() {
        for (id, _) in EFFECTS {
            let mut runtime = runtime::<1>(id);
            for now_ms in [1, 80, 2_000] {
                runtime.render(now_ms);
            }
        }
    }
}

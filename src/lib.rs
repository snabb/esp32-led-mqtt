#![cfg_attr(not(test), no_std)]

#[cfg(test)]
mod c_shims;
#[cfg(feature = "coap")]
pub mod coap_keys;
pub mod config_store;
pub mod device_config;
mod effects;
pub mod light_command;
pub mod ota_protocol;
pub mod provisioning;
pub mod random_color;

use smart_leds::RGB8;

/// The one name a device goes by everywhere: mDNS host, MQTT client ID and topic prefix,
/// and Home Assistant IDs. Hyphenated because DNS host names can't contain underscores.
pub fn device_slug(mac_suffix: [u8; 3]) -> heapless::String<16> {
    let [a, b, c] = mac_suffix;
    heapless::format!("esp32-led-{a:02x}{b:02x}{c:02x}").expect("fixed-length device slug")
}

/// Home Assistant name for "no effect", which shows the solid color.
pub const EFFECT_DISABLED_NAME: &str = "None";
/// Display names in `EffectId` declaration order; also the button cycle order.
pub const EFFECTS: [(EffectId, &str); 25] = [
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
    (EffectId::Fire2012, "Fire 2012"),
    (EffectId::Pacifica, "Pacifica"),
    (EffectId::Aurora, "Aurora"),
    (EffectId::PlasmaFlow, "Plasma Flow"),
    (EffectId::StaticNoise, "Static Noise"),
    (EffectId::FinlandSnow, "Finland Snow"),
    (
        EffectId::FinlandBlueWhiteTwinkle,
        "Finland Blue White Twinkle",
    ),
    (EffectId::FinlandSnowfall, "Finland Snowfall"),
    (EffectId::FinlandRibbon, "Finland Ribbon"),
    (EffectId::FinlandFrost, "Finland Frost"),
    (EffectId::FinlandSnowflakes, "Finland Snowflakes"),
    (EffectId::FinlandRevontulet, "Finland Revontulet"),
    (EffectId::FinlandKaamos, "Finland Kaamos"),
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
    Fire2012,
    Pacifica,
    Aurora,
    PlasmaFlow,
    StaticNoise,
    FinlandSnow,
    FinlandBlueWhiteTwinkle,
    FinlandSnowfall,
    FinlandRibbon,
    FinlandFrost,
    FinlandSnowflakes,
    FinlandRevontulet,
    FinlandKaamos,
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

/// Slider value for 1× effect speed.
pub const NORMAL_EFFECT_SPEED: u8 = 64;
pub const MAX_EFFECT_SPEED: u8 = 128;

/// Longest wall-clock gap one frame may advance the effect clock by, so a stall
/// (flash writes, OTA) doesn't make the animation jump.
const MAX_FRAME_STEP_US: u32 = 100_000;

#[derive(Clone, Copy, Debug)]
pub struct EffectParams {
    pub id: EffectId,
    pub primary: RGB8,
    /// Slider value `1..=MAX_EFFECT_SPEED`; see `speed_rate_q16`.
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
            speed: NORMAL_EFFECT_SPEED,
            intensity: 128,
        }
    }
}

/// Effect clock rate in Q16: `2^((slider - 64) / 16)`, i.e. 1/16× to 16×.
/// Exponential because perceived speed is roughly logarithmic, so every slider
/// step feels like the same change (≈4.4 %).
fn speed_rate_q16(slider: u8) -> u32 {
    // round(65536 * 2^(k / 16))
    const MANTISSA: [u32; 16] = [
        65536, 68438, 71468, 74632, 77936, 81386, 84990, 88752, 92682, 96785, 101070, 105545,
        110218, 115098, 120194, 125515,
    ];
    let exponent = i32::from(slider.clamp(1, MAX_EFFECT_SPEED)) - i32::from(NORMAL_EFFECT_SPEED);
    let mantissa = MANTISSA[exponent.rem_euclid(16) as usize];
    let octave = exponent.div_euclid(16);
    if octave >= 0 {
        mantissa << octave
    } else {
        mantissa >> -octave
    }
}

pub struct EffectRuntime<const N: usize> {
    frame: [RGB8; N],
    /// Simulation state after the latest tick; see `simulate`.
    tick_frame: [RGB8; N],
    prev_tick_frame: [RGB8; N],
    params: EffectParams,
    last_us: Option<u32>,
    /// Effect time: wall time scaled by the speed rate, so effects only see
    /// time at 1× and speed changes never jump.
    clock_us: u64,
    clock_frac_q16: u16,
    ticks_done: u64,
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
            tick_frame: [RGB8 { r: 0, g: 0, b: 0 }; N],
            prev_tick_frame: [RGB8 { r: 0, g: 0, b: 0 }; N],
            params,
            last_us: None,
            clock_us: 0,
            clock_frac_q16: 0,
            ticks_done: 0,
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

    /// Renders the frame for wall time `now_us`, which may wrap.
    pub fn render(&mut self, now_us: u32) -> &[RGB8; N] {
        self.advance_clock(now_us);
        effects::render(self);
        &self.frame
    }

    fn reset(&mut self) {
        self.clock_us = 0;
        self.clock_frac_q16 = 0;
        self.ticks_done = 0;
        self.effect_data.fill(0);
        self.twinkle_color.fill(RGB8 { r: 0, g: 0, b: 0 });
        self.frame.fill(RGB8 { r: 0, g: 0, b: 0 });
        self.tick_frame.fill(RGB8 { r: 0, g: 0, b: 0 });
        self.prev_tick_frame.fill(RGB8 { r: 0, g: 0, b: 0 });
    }

    fn advance_clock(&mut self, now_us: u32) {
        if let Some(last_us) = self.last_us {
            let elapsed = now_us.wrapping_sub(last_us).min(MAX_FRAME_STEP_US);
            let scaled = u64::from(elapsed) * u64::from(speed_rate_q16(self.params.speed))
                + u64::from(self.clock_frac_q16);
            self.clock_us += scaled >> 16;
            self.clock_frac_q16 = scaled as u16;
        }
        self.last_us = Some(now_us);
    }

    /// Wrapping phase of a cycle lasting `period_ms` at 1×.
    fn phase(&self, period_ms: u32) -> u16 {
        phase_at(self.clock_us, period_ms)
    }

    /// Index of the current step of a pattern that advances every `step_ms` at 1×.
    fn step(&self, step_ms: u32) -> u32 {
        (self.clock_us / (u64::from(step_ms) * 1000)) as u32
    }

    /// Fills the frame from `pixel(step, index)` for a pattern that advances one
    /// step every `step_ms` at 1×, crossfading into the next step at its end.
    fn render_steps(&mut self, step_ms: u32, pixel: impl Fn(u32, usize) -> RGB8) {
        let step_us = u64::from(step_ms) * 1000;
        let step = self.step(step_ms);
        let progress = (((self.clock_us % step_us) << 16) / step_us) as u16;
        let weight = step_crossfade(progress);
        for (index, out) in self.frame.iter_mut().enumerate() {
            let current = pixel(step, index);
            *out = if weight == 0 {
                current
            } else {
                blend_rgb(current, pixel(step.wrapping_add(1), index), weight)
            };
        }
    }

    /// Runs `tick(self, tick_time_us)` on `tick_frame` once per `tick_ms` of
    /// effect time (1×) since the last frame, at most `max_ticks` times, then
    /// shows the previous tick blended into the latest one by the progress
    /// towards the next tick. That keeps slow ticks continuous, one tick late.
    fn simulate(&mut self, tick_ms: u32, max_ticks: u64, mut tick: impl FnMut(&mut Self, u64)) {
        let tick_us = u64::from(tick_ms) * 1000;
        let target = self.clock_us / tick_us;
        let run = (target - self.ticks_done).min(max_ticks);
        for index in target - run..target {
            if index + 1 == target {
                self.prev_tick_frame = self.tick_frame;
            }
            tick(self, (index + 1) * tick_us);
        }
        self.ticks_done = target;

        let progress = (((self.clock_us % tick_us) << 8) / tick_us) as u8;
        for ((out, prev), next) in self
            .frame
            .iter_mut()
            .zip(&self.prev_tick_frame)
            .zip(&self.tick_frame)
        {
            *out = blend_rgb(*prev, *next, progress);
        }
    }

    fn chance(&mut self, intensity: u8, percent_at_full: u8) -> bool {
        let threshold = u32::from(intensity) * u32::from(percent_at_full) / 100;
        u32::from(self.rng.next_u8()) < threshold
    }
}

/// Wrapping phase at effect time `clock_us` of a cycle lasting `period_ms`.
fn phase_at(clock_us: u64, period_ms: u32) -> u16 {
    let period_us = u64::from(period_ms) * 1000;
    (((clock_us % period_us) << 16) / period_us) as u16
}

/// Weight of the next step at `progress` through the current one: hold for 70 %,
/// then smoothstep. The ramp scales with the step, so normal speed keeps crisp
/// steps while slow speeds fade.
fn step_crossfade(progress: u16) -> u8 {
    const HOLD: u32 = 65536 * 7 / 10;
    let progress = u32::from(progress);
    if progress < HOLD {
        return 0;
    }
    let x = (progress - HOLD) * 255 / (65536 - HOLD);
    (x * x * (3 * 255 - 2 * x) / (255 * 255)) as u8
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
    hsv_rainbow16(u16::from(hue) << 8, sat, val)
}

/// `hsv_rainbow` with 256 sub-steps per hue region, for slow hue drifts.
pub fn hsv_rainbow16(hue: u16, sat: u8, val: u8) -> RGB8 {
    let scaled = u32::from(hue) * 6;
    let region = scaled >> 16;
    let remainder = (scaled >> 8) as u8;
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

/// `a` blended towards `b` by `weight` (0 = `a`, 255 = `b`).
pub fn blend_rgb(a: RGB8, b: RGB8, weight: u8) -> RGB8 {
    let mix = |a: u8, b: u8| {
        ((u16::from(a) * u16::from(255 - weight) + u16::from(b) * u16::from(weight)) / 255) as u8
    };
    RGB8 {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
    }
}

pub fn half_sin8(value: u8) -> u8 {
    half_sin16(u16::from(value) << 8)
}

/// A pulse over one cycle: 0 at both ends, rising quadratically to a peak at
/// the middle.
pub fn half_sin16(value: u16) -> u8 {
    let x = u64::from(if value < 0x8000 {
        value
    } else {
        u16::MAX - value
    }) * 2;
    (x * x * 255 / (65535 * 65535)) as u8
}

/// A smooth, sine-like wave over one cycle: 0 at both ends, `u16::MAX` at the
/// middle, with no cusp at either turn.
pub fn ease_wave16(value: u16) -> u16 {
    let x = u64::from(if value < 0x8000 {
        value
    } else {
        u16::MAX - value
    }) * 2;
    (x * x * (3 * 65535 - 2 * x) / (65535 * 65535)) as u16
}

fn gamma_correct(value: u8) -> u8 {
    let value = u16::from(value);
    ((value * value) / u16::from(u8::MAX)) as u8
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
    fn device_slug_is_a_valid_host_name_label() {
        assert_eq!(device_slug([0x3e, 0xf8, 0xd0]).as_str(), "esp32-led-3ef8d0");
        assert_eq!(device_slug([0, 1, 2]).as_str(), "esp32-led-000102");
    }

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
        assert_eq!(EffectId::from_name("fire 2012"), Some(EffectId::Fire2012));
        assert_eq!(EffectId::from_name("missing"), None);
    }

    const FRAME_US: u32 = 16_667;

    fn runtime<const N: usize>(id: EffectId, speed: u8) -> EffectRuntime<N> {
        EffectRuntime::new(EffectParams {
            id,
            speed,
            intensity: 255,
            ..EffectParams::default()
        })
    }

    fn largest_channel_change(a: &[RGB8], b: &[RGB8]) -> u8 {
        a.iter()
            .zip(b)
            .flat_map(|(a, b)| [a.r.abs_diff(b.r), a.g.abs_diff(b.g), a.b.abs_diff(b.b)])
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn speed_rate_doubles_every_16_steps() {
        assert_eq!(speed_rate_q16(NORMAL_EFFECT_SPEED), 1 << 16);
        assert_eq!(speed_rate_q16(48), 1 << 15);
        assert_eq!(speed_rate_q16(80), 2 << 16);
        assert_eq!(speed_rate_q16(MAX_EFFECT_SPEED), 16 << 16);
        assert!((4096..4400).contains(&speed_rate_q16(1)));
        assert!((1..MAX_EFFECT_SPEED).all(|s| speed_rate_q16(s) < speed_rate_q16(s + 1)));
    }

    #[test]
    fn every_effect_animates() {
        for (id, name) in EFFECTS {
            let mut runtime = runtime::<8>(id, NORMAL_EFFECT_SPEED);
            let first = runtime.render(0).to_vec();
            // Random effects may repeat a frame, so allow two seconds of frames.
            let animates = (1..=120).any(|frame| runtime.render(frame * FRAME_US)[..] != first[..]);
            assert!(animates, "{name} should animate");
        }
    }

    #[test]
    fn every_effect_renders_a_single_led() {
        for (id, _) in EFFECTS {
            let mut runtime = runtime::<1>(id, MAX_EFFECT_SPEED);
            for now_us in [0, 80_000, 2_000_000, u32::MAX, 50_000] {
                runtime.render(now_us);
            }
        }
    }

    #[test]
    fn every_effect_follows_the_effect_clock() {
        for (id, name) in EFFECTS {
            let mut double = runtime::<16>(id, 80);
            let mut normal = runtime::<16>(id, NORMAL_EFFECT_SPEED);
            for frame in 0..600 {
                assert_eq!(
                    double.render(frame * FRAME_US),
                    normal.render(frame * 2 * FRAME_US),
                    "{name} differs at frame {frame}"
                );
            }
        }
    }

    #[test]
    fn speed_changes_do_not_jump() {
        for (id, name) in EFFECTS {
            let mut runtime = runtime::<16>(id, 1);
            for frame in 0..300 {
                runtime.render(frame * FRAME_US);
            }
            let now_us = 300 * FRAME_US;
            let before = *runtime.render(now_us);
            runtime.set_effect(EffectParams {
                speed: MAX_EFFECT_SPEED,
                ..runtime.params
            });
            assert_eq!(runtime.render(now_us), &before, "{name}");
        }
    }

    #[test]
    fn slowest_speed_changes_gradually() {
        // A full-scale change spread over at least ~7 frames; the shortest
        // crossfades (Scan, Color Wipe) set the bound.
        const MAX_CHANGE: u8 = 40;
        for (id, name) in EFFECTS {
            let mut runtime = runtime::<60>(id, 1);
            let mut previous = *runtime.render(0);
            // One minute of frames.
            for frame in 1..=3_600 {
                let current = *runtime.render(frame * FRAME_US);
                let change = largest_channel_change(&previous, &current);
                assert!(
                    change <= MAX_CHANGE,
                    "{name} changes by {change} at frame {frame}"
                );
                previous = current;
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "tls/roots.rs"]
mod tls_roots;

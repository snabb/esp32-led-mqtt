#![cfg_attr(not(test), no_std)]

use smart_leds::{
    RGB8,
    hsv::{Hsv, hsv2rgb},
};

pub const DEFAULT_BRIGHTNESS: u8 = 8;
pub const EFFECT_DISABLED_NAME: &str = "None";
pub const EFFECT_IDS: [EffectId; 17] = [
    EffectId::Rainbow,
    EffectId::ColorWipe,
    EffectId::Scan,
    EffectId::Twinkle,
    EffectId::RandomTwinkle,
    EffectId::Fireworks,
    EffectId::Flicker,
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
];

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
}

impl EffectId {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rainbow => "Rainbow",
            Self::ColorWipe => "Color Wipe",
            Self::Scan => "Scan",
            Self::Twinkle => "Twinkle",
            Self::RandomTwinkle => "Random Twinkle",
            Self::Fireworks => "Fireworks",
            Self::Flicker => "Flicker",
            Self::Breathe => "Breathe",
            Self::TheaterChase => "Theater Chase",
            Self::Confetti => "Confetti",
            Self::Sinelon => "Sinelon",
            Self::Juggle => "Juggle",
            Self::Bpm => "BPM",
            Self::Fire2012 => "Fire 2012",
            Self::Pacifica => "Pacifica",
            Self::Aurora => "Aurora",
            Self::PlasmaFlow => "Plasma Flow",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case(Self::Rainbow.name()) {
            Some(Self::Rainbow)
        } else if name.eq_ignore_ascii_case(Self::ColorWipe.name()) {
            Some(Self::ColorWipe)
        } else if name.eq_ignore_ascii_case(Self::Scan.name()) {
            Some(Self::Scan)
        } else if name.eq_ignore_ascii_case(Self::Twinkle.name()) {
            Some(Self::Twinkle)
        } else if name.eq_ignore_ascii_case(Self::RandomTwinkle.name()) {
            Some(Self::RandomTwinkle)
        } else if name.eq_ignore_ascii_case(Self::Fireworks.name()) {
            Some(Self::Fireworks)
        } else if name.eq_ignore_ascii_case(Self::Flicker.name()) {
            Some(Self::Flicker)
        } else if name.eq_ignore_ascii_case(Self::Breathe.name()) {
            Some(Self::Breathe)
        } else if name.eq_ignore_ascii_case(Self::TheaterChase.name()) {
            Some(Self::TheaterChase)
        } else if name.eq_ignore_ascii_case(Self::Confetti.name()) {
            Some(Self::Confetti)
        } else if name.eq_ignore_ascii_case(Self::Sinelon.name()) {
            Some(Self::Sinelon)
        } else if name.eq_ignore_ascii_case(Self::Juggle.name()) {
            Some(Self::Juggle)
        } else if name.eq_ignore_ascii_case(Self::Bpm.name()) {
            Some(Self::Bpm)
        } else if name.eq_ignore_ascii_case(Self::Fire2012.name()) {
            Some(Self::Fire2012)
        } else if name.eq_ignore_ascii_case(Self::Pacifica.name()) {
            Some(Self::Pacifica)
        } else if name.eq_ignore_ascii_case(Self::Aurora.name()) {
            Some(Self::Aurora)
        } else if name.eq_ignore_ascii_case(Self::PlasmaFlow.name()) {
            Some(Self::PlasmaFlow)
        } else {
            None
        }
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
        match self.params.id {
            EffectId::Rainbow => self.render_rainbow(now_ms),
            EffectId::ColorWipe => self.render_color_wipe(now_ms),
            EffectId::Scan => self.render_scan(now_ms),
            EffectId::Twinkle => self.render_twinkle(now_ms, false),
            EffectId::RandomTwinkle => self.render_twinkle(now_ms, true),
            EffectId::Fireworks => self.render_fireworks(now_ms),
            EffectId::Flicker => self.render_flicker(now_ms),
            EffectId::Breathe => self.render_breathe(now_ms),
            EffectId::TheaterChase => self.render_theater_chase(now_ms),
            EffectId::Confetti => self.render_confetti(now_ms),
            EffectId::Sinelon => self.render_sinelon(now_ms),
            EffectId::Juggle => self.render_juggle(now_ms),
            EffectId::Bpm => self.render_bpm(now_ms),
            EffectId::Fire2012 => self.render_fire2012(now_ms),
            EffectId::Pacifica => self.render_pacifica(now_ms),
            EffectId::Aurora => self.render_aurora(now_ms),
            EffectId::PlasmaFlow => self.render_plasma_flow(now_ms),
        }

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

    fn render_rainbow(&mut self, now_ms: u32) {
        let step = u16::from(self.params.speed.max(1)) * 3;
        self.phase = (now_ms as u16).wrapping_mul(step);
        let width = 50_u16.max(N as u16);
        let hue_delta = u16::MAX / width;

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            let hue = self
                .phase
                .wrapping_add((index as u16).wrapping_mul(hue_delta));
            *pixel = hsv_rainbow((hue >> 8) as u8, 240, 255);
        }
    }

    fn render_color_wipe(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(40, self.params.speed)) {
            self.wipe_index = (self.wipe_index + 1) % (N + 1);
        }

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            *pixel = if index < self.wipe_index {
                self.params.primary
            } else {
                RGB8 { r: 0, g: 0, b: 0 }
            };
        }
    }

    fn render_scan(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(50, self.params.speed)) {
            if self.scan_forward {
                if self.scan_position + 1 >= N {
                    self.scan_forward = false;
                    self.scan_position = self.scan_position.saturating_sub(1);
                } else {
                    self.scan_position += 1;
                }
            } else if self.scan_position == 0 {
                self.scan_forward = true;
                self.scan_position = (self.scan_position + 1).min(N.saturating_sub(1));
            } else {
                self.scan_position -= 1;
            }
        }

        self.frame.set_all(RGB8 { r: 0, g: 0, b: 0 });
        for offset in 0..scan_width(N) {
            let index = self.scan_position.saturating_add(offset);
            if index < N {
                self.frame.as_mut_slice()[index] = self.params.primary;
            }
        }
    }

    fn render_twinkle(&mut self, now_ms: u32, random_color: bool) {
        let interval = if random_color { 32 } else { 4 };
        if self.elapsed(now_ms, speed_interval(interval, self.params.speed)) {
            for index in 0..N {
                self.effect_data[index] = self.effect_data[index].saturating_sub(8);
                if self.effect_data[index] == 0 && self.chance(self.params.intensity, 20) {
                    self.effect_data[index] = 255;
                    self.twinkle_color[index] = if random_color {
                        hsv_rainbow(self.rng.next_u8(), 220, 255)
                    } else {
                        self.params.primary
                    };
                }
            }
        }

        for index in 0..N {
            let level = half_sin8(self.effect_data[index]);
            self.frame.as_mut_slice()[index] = scale_rgb(self.twinkle_color[index], level);
        }
    }

    fn render_fireworks(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(32, self.params.speed)) {
            for pixel in self.frame.as_mut_slice() {
                *pixel = fade_to_black(*pixel, 120);
            }

            let sparks = (u16::from(self.params.intensity) * N as u16) / 2550 + 1;
            for _ in 0..sparks {
                if self.chance(self.params.intensity, 10) {
                    let index = (self.rng.next_u16() as usize) % N.max(1);
                    self.frame.as_mut_slice()[index] = self.params.primary;
                }
            }
        }
    }

    fn render_flicker(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(16, self.params.speed)) {
            for index in 0..N {
                let flicker = 224_u8.saturating_add(self.rng.next_u8() >> 3);
                self.frame.as_mut_slice()[index] = scale_rgb(self.params.primary, flicker);
            }
        }
    }

    fn render_breathe(&mut self, now_ms: u32) {
        let speed = u32::from(self.params.speed.max(1));
        let wave = ((now_ms.saturating_mul(speed) / 96) & 0xff) as u8;
        let level = half_sin8(wave).saturating_add(8);
        self.frame.set_all(scale_rgb(self.params.primary, level));
    }

    fn render_theater_chase(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(120, self.params.speed)) {
            self.phase = (self.phase + 1) % 3;
        }

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            *pixel = if (index + usize::from(self.phase)) % 3 == 0 {
                self.params.primary
            } else {
                RGB8 { r: 0, g: 0, b: 0 }
            };
        }
    }

    fn render_confetti(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(24, self.params.speed)) {
            for pixel in self.frame.as_mut_slice() {
                *pixel = fade_to_black(*pixel, 32);
            }

            if N > 0 {
                let index = (self.rng.next_u16() as usize) % N;
                let hue = self
                    .params
                    .primary
                    .r
                    .wrapping_add(self.rng.next_u8() & 0x3f);
                self.frame.as_mut_slice()[index] = hsv_rainbow(hue, 200, 255);
            }
        }
    }

    fn render_sinelon(&mut self, now_ms: u32) {
        if self.elapsed(now_ms, speed_interval(20, self.params.speed)) {
            for pixel in self.frame.as_mut_slice() {
                *pixel = fade_to_black(*pixel, 40);
            }
        }

        if let Some(position) = beat_position(now_ms / 5, self.params.speed, N) {
            self.frame.as_mut_slice()[position] = self.params.primary;
        }
    }

    fn render_juggle(&mut self, now_ms: u32) {
        for pixel in self.frame.as_mut_slice() {
            *pixel = fade_to_black(*pixel, 48);
        }

        if N == 0 {
            return;
        }

        let dots = N.min(6);
        for dot in 0..dots {
            let speed = juggle_dot_speed(self.params.speed, dot as u8);
            let time = now_ms / 4;
            if let Some(position) = beat_position(time.wrapping_add((dot * 73) as u32), speed, N) {
                let hue = (dot as u8).wrapping_mul(32).wrapping_add(self.phase as u8);
                let color = hsv_rainbow(hue, 220, 255);
                let blended = add_rgb(self.frame.as_slice()[position], color);
                self.frame.as_mut_slice()[position] = blended;
            }
        }
        self.phase = self.phase.wrapping_add(1);
    }

    fn render_bpm(&mut self, now_ms: u32) {
        let bpm = 30 + u32::from(self.params.speed) / 2;
        let wave = (((now_ms / 4).saturating_mul(bpm) / 60) & 0xff) as u8;
        let level = half_sin8(wave).saturating_add(48);

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            let hue = ((index * 256) / N.max(1)) as u8;
            let base = hsv_rainbow(hue.wrapping_add(self.params.primary.r), 220, 255);
            *pixel = scale_rgb(base, level);
        }
    }

    fn render_fire2012(&mut self, now_ms: u32) {
        if !self.elapsed(now_ms, speed_interval(90, self.params.speed)) {
            return;
        }

        for index in 0..N {
            let cooling =
                self.rng.next_u8() % (u8::try_from((55 * 10) / N.max(1) + 2).unwrap_or(u8::MAX));
            self.effect_data[index] = self.effect_data[index].saturating_sub(cooling);
        }

        for index in (2..N).rev() {
            let heat = (u16::from(self.effect_data[index - 1])
                + u16::from(self.effect_data[index - 2])
                + u16::from(self.effect_data[index - 2]))
                / 3;
            self.effect_data[index] = heat as u8;
        }

        if N > 0 && self.chance(self.params.intensity, 45) {
            let spark_index = (self.rng.next_u8() as usize) % fire_spark_height(N);
            let spark = 160_u8.saturating_add(self.rng.next_u8() % 96);
            self.effect_data[spark_index] = self.effect_data[spark_index].saturating_add(spark);
        }

        for index in 0..N {
            self.frame.as_mut_slice()[index] = heat_color(self.effect_data[index]);
        }
    }

    fn render_pacifica(&mut self, now_ms: u32) {
        let speed = u32::from(self.params.speed.max(1));
        let phase_a = ((now_ms.saturating_mul(speed) / 128) & 0xff) as u8;
        let phase_b = ((now_ms.saturating_mul(speed) / 197) & 0xff) as u8;

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            let pos = ((index * 256) / N.max(1)) as u8;
            let wave_a = half_sin8(pos.wrapping_add(phase_a));
            let wave_b = half_sin8(pos.wrapping_mul(2).wrapping_sub(phase_b));
            let green = scale8(wave_a, 140).saturating_add(16);
            let blue = scale8(wave_b.saturating_add(wave_a / 2), 210).saturating_add(32);
            let white = scale8(wave_a.saturating_sub(190), 80);
            *pixel = RGB8 {
                r: white,
                g: green.saturating_add(white),
                b: blue.saturating_add(white),
            };
        }
    }

    fn render_aurora(&mut self, now_ms: u32) {
        let speed = u32::from(self.params.speed.max(1));
        let phase_a = ((now_ms.saturating_mul(speed) / 181) & 0xff) as u8;
        let phase_b = ((now_ms.saturating_mul(speed) / 313) & 0xff) as u8;
        let phase_c = ((now_ms.saturating_mul(speed) / 89) & 0xff) as u8;
        let shimmer_gain = self.params.intensity.max(32);

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            let pos = ((index * 256) / N.max(1)) as u8;
            let curtain = half_sin8(pos.wrapping_add(phase_a));
            let fold = half_sin8(pos.wrapping_mul(3).wrapping_sub(phase_b));
            let shimmer_wave = half_sin8(pos.wrapping_mul(7).wrapping_add(phase_c));
            let shimmer = scale8(shimmer_wave.saturating_sub(212), shimmer_gain);

            let green = scale8(curtain, 170).saturating_add(scale8(fold, 80));
            let blue = scale8(curtain, 95).saturating_add(scale8(fold, 185));
            let violet = scale8(fold.saturating_sub(curtain / 2), 130);

            *pixel = RGB8 {
                r: violet.saturating_add(shimmer),
                g: green.saturating_add(shimmer),
                b: blue.saturating_add(shimmer),
            };
        }
    }

    fn render_plasma_flow(&mut self, now_ms: u32) {
        let speed = u32::from(self.params.speed.max(1));
        let phase_a = ((now_ms.saturating_mul(speed) / 97) & 0xff) as u8;
        let phase_b = ((now_ms.saturating_mul(speed) / 151) & 0xff) as u8;
        let phase_c = ((now_ms.saturating_mul(speed) / 211) & 0xff) as u8;
        let brightness_floor = 64_u8.saturating_add(self.params.intensity / 8);

        for (index, pixel) in self.frame.as_mut_slice().iter_mut().enumerate() {
            let pos = ((index * 256) / N.max(1)) as u8;
            let wave_a = half_sin8(pos.wrapping_add(phase_a));
            let wave_b = half_sin8(pos.wrapping_mul(2).wrapping_sub(phase_b));
            let wave_c = half_sin8(pos.wrapping_mul(5).wrapping_add(phase_c));
            let mixed = ((u16::from(wave_a) + u16::from(wave_b) + u16::from(wave_c)) / 3) as u8;
            let hue = mixed
                .wrapping_add(phase_b)
                .wrapping_add(self.params.primary.r / 2);
            let value = brightness_floor.saturating_add(scale8(mixed, 255 - brightness_floor));

            *pixel = hsv_rainbow(hue, 240, value);
        }
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

fn gamma_correct(value: u8) -> u8 {
    let value = u16::from(value);
    ((value * value) / u16::from(u8::MAX)) as u8
}

fn scan_width(count: usize) -> usize {
    (count / 20).clamp(1, 4)
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

fn fire_spark_height(count: usize) -> usize {
    (count / 6).clamp(1, 10)
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

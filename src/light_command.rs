//! Light and effect-speed commands, shared by every control plane.

use log::warn;
use serde::Deserialize;
use smart_leds::RGB8;

use crate::{EFFECT_DISABLED_NAME, EffectId, MAX_EFFECT_SPEED};

#[derive(Clone, Copy)]
pub struct LightState {
    pub on: bool,
    pub brightness: u8,
    pub effect: Option<EffectId>,
    pub speed: u8,
    pub color: RGB8,
}

/// Parses a light command, which has the same shape as the published state.
pub fn parse_light_command(payload: &[u8]) -> Option<LightUpdate> {
    let Ok((command, _)) = serde_json_core::from_slice::<LightCommand<'_>>(payload) else {
        warn!("Ignoring invalid light command JSON");
        return None;
    };
    let on = match command.state {
        Some(state) if state.eq_ignore_ascii_case("ON") => Some(true),
        Some(state) if state.eq_ignore_ascii_case("OFF") => Some(false),
        _ => None,
    };
    let effect = command.effect.and_then(|name| {
        if name.eq_ignore_ascii_case(EFFECT_DISABLED_NAME) {
            Some(None)
        } else if let Some(id) = EffectId::from_name(name) {
            Some(Some(id))
        } else {
            warn!("Ignoring unknown light effect '{}'", name);
            None
        }
    });

    Some(LightUpdate {
        on,
        brightness: command.brightness,
        color: command.color.map(|LightColor { r, g, b }| RGB8 { r, g, b }),
        effect,
    })
}

pub struct LightUpdate {
    on: Option<bool>,
    brightness: Option<u8>,
    color: Option<RGB8>,
    effect: Option<Option<EffectId>>,
}

impl LightUpdate {
    pub fn apply(&self, state: &mut LightState) {
        if let Some(on) = self.on {
            state.on = on;
        }
        if let Some(brightness) = self.brightness {
            state.brightness = brightness;
        }
        if let Some(color) = self.color {
            state.color = color;
        }
        if let Some(effect) = self.effect {
            state.effect = effect;
        }
    }
}

pub fn parse_speed(payload: &[u8]) -> Option<u8> {
    core::str::from_utf8(payload)
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .filter(|speed| (1..=MAX_EFFECT_SPEED).contains(speed))
}

#[derive(Deserialize)]
struct LightCommand<'a> {
    #[serde(default)]
    state: Option<&'a str>,
    #[serde(default)]
    brightness: Option<u8>,
    #[serde(default)]
    color: Option<LightColor>,
    #[serde(default)]
    effect: Option<&'a str>,
}

#[derive(Clone, Copy, Deserialize)]
struct LightColor {
    r: u8,
    g: u8,
    b: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> LightState {
        LightState {
            on: false,
            brightness: 8,
            effect: Some(EffectId::Rainbow),
            speed: 64,
            color: RGB8 { r: 1, g: 2, b: 3 },
        }
    }

    fn applied(payload: &[u8]) -> LightState {
        let mut state = state();
        parse_light_command(payload).unwrap().apply(&mut state);
        state
    }

    #[test]
    fn missing_fields_keep_their_values() {
        let result = applied(br#"{"state":"on"}"#);
        assert!(result.on);
        assert_eq!(result.brightness, 8);
        assert_eq!(result.effect, Some(EffectId::Rainbow));
        assert_eq!(result.color, RGB8 { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn every_field_applies() {
        let result = applied(
            br#"{"state":"ON","brightness":128,"color":{"r":255,"g":0,"b":64},"effect":"pacifica"}"#,
        );
        assert!(result.on);
        assert_eq!(result.brightness, 128);
        assert_eq!(
            result.color,
            RGB8 {
                r: 255,
                g: 0,
                b: 64
            }
        );
        assert_eq!(result.effect, Some(EffectId::Pacifica));
    }

    #[test]
    fn none_effect_selects_the_solid_color() {
        assert_eq!(applied(br#"{"effect":"None"}"#).effect, None);
    }

    #[test]
    fn unknown_effect_still_applies_other_fields() {
        let result = applied(br#"{"brightness":9,"effect":"nope"}"#);
        assert_eq!(result.brightness, 9);
        assert_eq!(result.effect, Some(EffectId::Rainbow));
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(parse_light_command(b"{").is_none());
    }

    #[test]
    fn speed_must_be_in_range() {
        assert_eq!(parse_speed(b" 20\n"), Some(20));
        assert_eq!(parse_speed(b"128"), Some(128));
        for bad in [&b"0"[..], b"129", b"fast", b"", &[0xff]] {
            assert_eq!(parse_speed(bad), None);
        }
    }
}

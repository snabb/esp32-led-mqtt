use core::cell::RefCell;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use esp32_led_mqtt::{DEFAULT_BRIGHTNESS, EFFECT_DEFINITIONS, EffectId, EffectParams};
use smart_leds::RGB8;

pub(crate) static LIGHT_STATE: critical_section::Mutex<RefCell<LightState>> =
    critical_section::Mutex::new(RefCell::new(DEFAULT_LIGHT_STATE));
pub(crate) static LIGHT_STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub(crate) const MAX_EFFECT_SPEED: u8 = 128;
const DEFAULT_EFFECT_SPEED: u8 = 64;
const DEFAULT_LIGHT_STATE: LightState = LightState {
    on: true,
    brightness: DEFAULT_BRIGHTNESS,
    effect_code: EFFECT_DEFINITIONS[0].code,
    speed: DEFAULT_EFFECT_SPEED,
    color: RGB8 {
        r: 255,
        g: 96,
        b: 24,
    },
};

#[derive(Clone, Copy)]
pub(crate) struct LightState {
    pub(crate) on: bool,
    pub(crate) brightness: u8,
    pub(crate) effect_code: u8,
    pub(crate) speed: u8,
    pub(crate) color: RGB8,
}

pub(crate) fn mark_dirty() {
    LIGHT_STATE_CHANGED.signal(());
}

pub(crate) fn get() -> LightState {
    critical_section::with(|cs| *LIGHT_STATE.borrow(cs).borrow())
}

pub(crate) fn update(update: impl FnOnce(&mut LightState)) {
    critical_section::with(|cs| update(&mut LIGHT_STATE.borrow(cs).borrow_mut()));
}

pub(crate) fn current_effect_params(id: EffectId) -> EffectParams {
    effect_params(id, get())
}

pub(crate) fn effect_params(id: EffectId, state: LightState) -> EffectParams {
    EffectParams {
        id,
        primary: state.color,
        speed: effect_speed_value(state.speed),
        intensity: 128,
    }
}

fn effect_speed_value(slider: u8) -> u8 {
    let slider = slider.clamp(1, MAX_EFFECT_SPEED);
    let offset = u64::from(slider.saturating_sub(1));
    let max_offset = u64::from(MAX_EFFECT_SPEED - 1);
    1 + ((offset * offset * offset * offset * offset)
        / (max_offset * max_offset * max_offset * max_offset)) as u8
}

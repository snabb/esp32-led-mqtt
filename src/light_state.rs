use core::cell::RefCell;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
pub(crate) use esp32_led_mqtt::MAX_EFFECT_SPEED;
use esp32_led_mqtt::{EffectId, EffectParams, NORMAL_EFFECT_SPEED};
use smart_leds::RGB8;

static LIGHT_STATE: critical_section::Mutex<RefCell<LightState>> =
    critical_section::Mutex::new(RefCell::new(DEFAULT_LIGHT_STATE));
/// Raised by every `update`; MQTT publishes the new state when it fires.
pub(crate) static LIGHT_STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

const DEFAULT_BRIGHTNESS: u8 = 8;
const DEFAULT_LIGHT_STATE: LightState = LightState {
    on: true,
    brightness: DEFAULT_BRIGHTNESS,
    effect: Some(EffectId::Rainbow),
    speed: NORMAL_EFFECT_SPEED,
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
    pub(crate) effect: Option<EffectId>,
    pub(crate) speed: u8,
    pub(crate) color: RGB8,
}

pub(crate) fn get() -> LightState {
    critical_section::with(|cs| *LIGHT_STATE.borrow(cs).borrow())
}

pub(crate) fn update(update: impl FnOnce(&mut LightState)) {
    critical_section::with(|cs| update(&mut LIGHT_STATE.borrow(cs).borrow_mut()));
    LIGHT_STATE_CHANGED.signal(());
}

pub(crate) fn effect_params(id: EffectId, state: LightState) -> EffectParams {
    EffectParams {
        id,
        primary: state.color,
        speed: state.speed,
        intensity: 128,
    }
}
